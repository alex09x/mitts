# Mitts Build — Apple signing/notary policy controls without private credentials.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import io
import json
import os
import tarfile
import tempfile
import unittest
import zipfile
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import apple_keychain
import apple_sign
import apple_verify

TEAM = "TEAM123456"
SHA = "a" * 40
CDHASH = "c" * 40
JOB = "12345678-1234-1234-1234-123456789abc"
ZIP_SHA = "b" * 64


def receipt():
    return {
        "status": "Accepted",
        "jobId": JOB,
        "sha256": ZIP_SHA,
        "issues": None,
        "ticketContents": [
            {
                "cdhash": CDHASH,
                "arch": arch,
                "path": "submission.zip/" + target + "/mitts",
            }
            for target, arch in zip(apple_verify.MAC_TARGETS, ["arm64", "x86_64"])
        ],
    }


class ApplePolicyTests(unittest.TestCase):
    def test_temporary_keychain_is_searchable_during_signing_and_originals_restored(
        self,
    ):
        import base64

        identity = "A" * 40
        original = ["/fixture/login.keychain-db", "/fixture/System.keychain"]
        for fail in (False, True):
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as directory:
                search = list(original)
                values = {
                    name: "fixture-private" for name in apple_keychain.SECRET_NAMES
                }
                values.update(
                    {
                        "RUNNER_TEMP": directory,
                        "APPLE_TEAM_ID": TEAM,
                        "APPLE_SIGNING_IDENTITY": identity,
                        "APPLE_CERTIFICATE_P12_BASE64": base64.b64encode(
                            b"fixture"
                        ).decode(),
                    }
                )

                def run(args, **kwargs):
                    nonlocal search
                    if args[:4] == ["security", "list-keychains", "-d", "user"]:
                        if len(args) > 4:
                            self.assertEqual(args[4], "-s")
                            search = [str(path) for path in args[5:]]
                        return SimpleNamespace(
                            stdout="\n".join(json.dumps(path) for path in search)
                        )
                    if "find-identity" in args:
                        return SimpleNamespace(
                            stdout=f'1) {identity} "Developer ID Application: Fixture ({TEAM})"'
                        )
                    return SimpleNamespace(stdout="", stderr="", returncode=0)

                with (
                    patch.object(apple_keychain, "hosted_main"),
                    patch.object(apple_keychain, "run", side_effect=run),
                    patch.dict(os.environ, values),
                ):
                    try:
                        with apple_keychain.credentials() as auth:
                            self.assertEqual(search, [str(auth["keychain"]), *original])
                            if fail:
                                raise RuntimeError("fixture signing failure")
                    except RuntimeError as error:
                        self.assertTrue(fail)
                        self.assertEqual(str(error), "fixture signing failure")
                self.assertEqual(search, original)
                self.assertEqual(list(Path(directory).iterdir()), [])

    def test_private_keys_are_restricted_to_canonical_hosted_main(self):
        env = {
            "GITHUB_ACTIONS": "true",
            "GITHUB_REPOSITORY": "alex09x/mitts",
            "GITHUB_REF": "refs/heads/feature",
        }
        with (
            patch.dict(os.environ, env),
            patch.object(apple_keychain.sys, "platform", "darwin"),
        ):
            with self.assertRaises(ValueError):
                apple_keychain.hosted_main()

    def test_missing_secret_does_not_fall_back_to_ad_hoc(self):
        with (
            patch.object(apple_keychain, "hosted_main"),
            patch.dict(os.environ, {}, clear=True),
            patch.object(apple_keychain, "run") as run,
        ):
            with self.assertRaises(ValueError), apple_keychain.credentials():
                pass
            run.assert_not_called()

    def test_ad_hoc_signature_is_rejected(self):
        with patch.object(
            apple_verify,
            "run",
            return_value=SimpleNamespace(
                stderr="Signature=adhoc\nTeamIdentifier=" + TEAM
            ),
        ):
            with self.assertRaises(ValueError):
                apple_verify.signature(Path("fixture"), TEAM)

    def test_hardened_runtime_and_timestamp_are_required(self):
        valid = f"TeamIdentifier={TEAM}\nCodeDirectory flags=0x10000(runtime)\nTimestamp=fixture\nCDHash={CDHASH}\n"
        for detail in [
            valid.replace("Timestamp=fixture\n", ""),
            valid.replace("10000", "0"),
        ]:
            with (
                self.subTest(detail=detail),
                patch.object(
                    apple_verify, "run", return_value=SimpleNamespace(stderr=detail)
                ),
            ):
                with self.assertRaises(ValueError):
                    apple_verify.signature(Path("fixture"), TEAM)

    def test_developer_id_requirement_is_literal_not_a_filename(self):
        detail = (
            f"TeamIdentifier={TEAM}\nCodeDirectory flags=0x10000(runtime)\n"
            f"Timestamp=fixture\nCDHash={CDHASH}\n"
        )
        with patch.object(
            apple_verify, "run", return_value=SimpleNamespace(stderr=detail)
        ) as run:
            self.assertEqual(apple_verify.signature(Path("fixture"), TEAM), CDHASH)
        requirement = run.call_args_list[0].args[0]
        self.assertEqual(requirement[:4], ["codesign", "--verify", "--strict", "-R"])
        self.assertEqual(
            requirement[4],
            f'=anchor apple generic and certificate leaf[subject.OU] = "{TEAM}" '
            "and certificate leaf[field.1.2.840.113635.100.6.1.13] exists",
        )

    def test_receipt_matches_actual_code_and_supported_architectures(self):
        for target in apple_verify.MAC_TARGETS:
            apple_verify.receipt_matches(receipt(), JOB, ZIP_SHA, target, CDHASH)

    def test_rejected_submission_or_mismatched_ticket_cannot_pass(self):
        cases = [
            {**receipt(), "status": "Invalid"},
            {**receipt(), "jobId": "wrong"},
            {**receipt(), "sha256": "d" * 64},
            {**receipt(), "issues": [{"message": "error"}]},
            {**receipt(), "ticketContents": []},
        ]
        for value in cases:
            with self.subTest(value=value), self.assertRaises(ValueError):
                apple_verify.receipt_matches(
                    value, JOB, ZIP_SHA, apple_verify.MAC_TARGETS[0], CDHASH
                )

    def test_submission_contains_only_mac_executables_and_rejection_produces_no_archives(
        self,
    ):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "raw"
            source.mkdir()
            for target in apple_verify.MAC_TARGETS:
                with tarfile.open(
                    source / f"mitts-v0.3.18-{target}.tar.gz", "w:gz"
                ) as archive:
                    files = {
                        "mitts": b"fixture",
                        "LICENSE-MIT": b"license",
                        "LICENSE-APACHE": b"license",
                        "BUILD.txt": f"Mitts Build v0.3.18\nTarget: {target}\nSource: https://github.com/alex09x/mitts/tree/{SHA}\n".encode(),
                    }
                    for name, data in files.items():
                        member = tarfile.TarInfo(name)
                        member.size = len(data)
                        archive.addfile(member, io.BytesIO(data))

            @contextmanager
            def credentials():
                yield {"identity": "fixture", "keychain": "fixture", "auth": []}

            def run(args, **kwargs):
                if "submit" in args:
                    archive = Path(args[3])
                    with zipfile.ZipFile(archive) as content:
                        self.assertEqual(
                            set(content.namelist()),
                            {target + "/mitts" for target in apple_verify.MAC_TARGETS},
                        )
                    return SimpleNamespace(
                        stdout=json.dumps({"id": JOB, "status": "Invalid"})
                    )
                return SimpleNamespace(stdout="", stderr="", returncode=0)

            with (
                patch.object(apple_sign, "validate"),
                patch.object(apple_sign, "credentials", credentials),
                patch.object(apple_sign, "expected_team", return_value=TEAM),
                patch.object(apple_sign, "signature", return_value=CDHASH),
                patch.object(apple_sign, "run", side_effect=run),
            ):
                with self.assertRaises(ValueError):
                    apple_sign.sign_release(
                        source, root / "out", root / "evidence", "v0.3.18", SHA
                    )
            self.assertEqual(list((root / "out").iterdir()), [])

    def test_redaction_never_returns_secret_values(self):
        with patch.object(apple_keychain, "_redactions", ["fixture-private-value"]):
            self.assertEqual(
                apple_keychain.redact("error fixture-private-value"), "error [REDACTED]"
            )

    def test_timeout_does_not_expose_private_command_arguments(self):
        import subprocess

        with patch.object(
            apple_keychain.subprocess,
            "run",
            side_effect=subprocess.TimeoutExpired(
                ["security", "-P", "fixture-private-value"], 1
            ),
        ):
            with self.assertRaises(RuntimeError) as failure:
                apple_keychain.run(["security", "-P", "fixture-private-value"])
            self.assertNotIn("fixture-private-value", str(failure.exception))

    def test_only_actual_release_uses_the_signing_environment(self):
        workflows = Path(__file__).resolve().parents[2] / ".github/workflows"
        release = (workflows / "release.yml").read_text()
        signing = release.split("  apple-sign:\n", 1)[1].split("  apple-verify:\n", 1)[
            0
        ]
        self.assertIn("inputs.mode != 'verify'", signing)
        self.assertIn("github.ref == 'refs/heads/main'", signing)
        self.assertIn("environment: release-signing", signing)
        for path in workflows.glob("*.yml"):
            if path.name not in {"release.yml", "apple-credentials-check.yml"}:
                self.assertNotIn("secrets.APPLE_", path.read_text(), path.name)
        diagnostic = (workflows / "apple-credentials-check.yml").read_text()
        self.assertIn("workflow_dispatch:", diagnostic)
        self.assertNotIn("apple_sign.py", diagnostic)
        self.assertNotIn("notarytool submit", diagnostic)
