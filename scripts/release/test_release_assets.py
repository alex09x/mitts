# Mitts Build — complete archive and checksum manifest regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from assets import (
    DARWIN_ARCHES,
    SIGNING_MARKERS,
    TARGETS,
    digest,
    main,
    validate,
    validate_notarized,
)

TAG = "v0.4.0"
SHA = "a" * 40
TEAM = "TEAM123456"
JOB_ID = "12345678-1234-4321-8765-123456789abc"
ASSETS_SCRIPT = Path(__file__).with_name("assets.py")


class ReleaseAssetTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.folder = Path(temporary.name)
        for target in TARGETS:
            self.archive(target)

    def archive(self, target, sha=SHA, symlink=False, duplicate=False):
        name = f"mitts-{TAG}-{target}.tar.gz"
        archive = self.folder / name
        contents = {
            "mitts": b"fixture binary",
            "LICENSE-MIT": b"fixture license",
            "LICENSE-APACHE": b"fixture license",
            "BUILD.txt": (
                f"Mitts Build {TAG}\n"
                f"Target: {target}\n"
                f"Source: https://github.com/alex09x/mitts/tree/{sha}\n"
            ).encode(),
        }
        with tarfile.open(archive, "w:gz") as tar:
            for entry, data in contents.items():
                info = tarfile.TarInfo(entry)
                info.size = len(data)
                if symlink and entry == "mitts":
                    info.type = tarfile.SYMTYPE
                    info.linkname = "/unexpected"
                    info.size = 0
                tar.addfile(info, io.BytesIO(data))
            if duplicate:
                info = tarfile.TarInfo("mitts")
                tar.addfile(info, io.BytesIO())
        (self.folder / (name + ".sha256")).write_text(f"{digest(archive)}  {name}\n")
        return archive

    def test_complete_set_has_nine_verified_assets(self):
        manifest = validate(self.folder, TAG, SHA)
        self.assertEqual(len(manifest), 9)
        self.assertEqual(len((self.folder / "SHA256SUMS").read_text().splitlines()), 4)

    def test_missing_native_target_prevents_publication(self):
        self.archive(TARGETS[0]).unlink()
        with self.assertRaises(FileNotFoundError):
            validate(self.folder, TAG, SHA)

    def test_wrong_source_provenance_is_rejected(self):
        self.archive(TARGETS[0], sha="b" * 40)
        with self.assertRaises(ValueError):
            validate(self.folder, TAG, SHA)

    def test_symlink_and_duplicate_members_are_rejected(self):
        for mode in ["symlink", "duplicate"]:
            with self.subTest(mode=mode):
                self.archive(TARGETS[0], **{mode: True})
                with self.assertRaises(ValueError):
                    validate(self.folder, TAG, SHA)

    def test_wrong_checksum_is_rejected(self):
        checksum = next(self.folder.glob("*.sha256"))
        checksum.write_text("0" * 64 + "  " + checksum.name.removesuffix(".sha256"))
        with self.assertRaises(ValueError):
            validate(self.folder, TAG, SHA)

    def test_existing_manifest_is_not_silently_replaced(self):
        (self.folder / "SHA256SUMS").write_text("stale manifest")
        with self.assertRaises(ValueError):
            validate(self.folder, TAG, SHA)


class NotarizedReleaseAssetTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.folder = Path(temporary.name)
        self.cdhashes = {
            "aarch64-apple-darwin": "c" * 40,
            "x86_64-apple-darwin": "d" * 40,
        }
        self.build_set()

    def ticket(self, path, arch="arm64", cdhash=None):
        return {
            "path": path,
            "digestAlgorithm": "SHA-256",
            "cdhash": cdhash or self.cdhashes["aarch64-apple-darwin"],
            "arch": arch,
        }

    def receipt_dict(
        self, cdhashes=None, status="Accepted", job_id=JOB_ID, issues=None, tickets=None
    ):
        hashes = cdhashes or self.cdhashes
        return {
            "logFormatVersion": 1,
            "jobId": job_id,
            "status": status,
            "issues": issues,
            "ticketContents": (
                tickets
                if tickets is not None
                else [
                    self.ticket(f"notarize.zip/{t}/mitts", arch, hashes[t])
                    for t, arch in DARWIN_ARCHES.items()
                ]
            ),
        }

    def write_archive(self, target, binary=None, extra_lines=()):
        name = f"mitts-{TAG}-{target}.tar.gz"
        archive = self.folder / name
        payload = binary if binary is not None else f"binary-{target}".encode()
        base_lines = [
            f"Mitts Build {TAG}",
            f"Target: {target}",
            f"Source: https://github.com/alex09x/mitts/tree/{SHA}",
        ]
        metadata = "\n".join([*base_lines, *extra_lines]) + "\n"
        contents = {
            "mitts": payload,
            "LICENSE-MIT": b"fixture license",
            "LICENSE-APACHE": b"fixture license",
            "BUILD.txt": metadata.encode(),
        }
        with tarfile.open(archive, "w:gz") as tar:
            for entry, data in contents.items():
                info = tarfile.TarInfo(entry)
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
        (self.folder / (name + ".sha256")).write_text(f"{digest(archive)}  {name}\n")
        (self.folder / "SHA256SUMS").unlink(missing_ok=True)
        return archive

    def darwin_lines(self, target, binary=None, overrides=None, receipt=None):
        payload = binary if binary is not None else f"binary-{target}".encode()
        values = {
            "Signing-Team": TEAM,
            "Signed-Binary-SHA256": hashlib.sha256(payload).hexdigest(),
            "CodeDirectory-Hash": self.cdhashes[target],
            "Notarization-ID": JOB_ID,
            "Notarization-Status": "Accepted",
            "Notarization-Receipt": json.dumps(
                receipt if receipt is not None else self.receipt_dict(),
                separators=(",", ":"),
            ),
        }
        if overrides:
            values.update(overrides)
        return [f"{key}: {val}" for key, val in values.items() if val is not None]

    def build_set(self, notarized=True):
        for target in TARGETS:
            lines = (
                self.darwin_lines(target)
                if notarized and target in DARWIN_ARCHES
                else ()
            )
            self.write_archive(target, extra_lines=lines)

    def run_cli(self, *extra_args, team=TEAM):
        env = {k: v for k, v in os.environ.items() if k != "APPLE_TEAM_ID"}
        if team is not None:
            env["APPLE_TEAM_ID"] = team
        return subprocess.run(
            [
                sys.executable,
                str(ASSETS_SCRIPT),
                *extra_args,
                str(self.folder),
                TAG,
                SHA,
            ],
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def assert_rejected(self, team=TEAM):
        with (
            patch.dict(os.environ, {"APPLE_TEAM_ID": team}),
            self.assertRaises(ValueError),
        ):
            validate_notarized(self.folder, TAG, SHA)

    def test_raw_set_passes_raw_mode_and_fails_notarized_without_side_effects(self):
        self.build_set(notarized=False)
        self.assert_rejected()
        self.assertFalse((self.folder / "SHA256SUMS").exists())
        self.assertNotEqual(self.run_cli("--require-notarized").returncode, 0)
        self.assertFalse((self.folder / "SHA256SUMS").exists())
        raw = self.run_cli()
        self.assertEqual(raw.returncode, 0, raw.stderr)
        self.assertEqual(len(json.loads(raw.stdout)), 9)

    def test_complete_notarized_set_passes_validators_cli_and_cdhash_variants(self):
        with patch.dict(os.environ, {"APPLE_TEAM_ID": TEAM}):
            manifest = validate_notarized(self.folder, "0.4.0", SHA)
            self.assertEqual(len(manifest), 9)
            self.assertEqual(validate(self.folder, TAG, SHA), manifest)
            with io.StringIO() as buf, patch("sys.stdout", buf):
                main(["--require-notarized", str(self.folder), TAG, SHA])
                self.assertEqual(json.loads(buf.getvalue()), manifest)
        cli = self.run_cli("--require-notarized")
        self.assertEqual(cli.returncode, 0, cli.stderr)
        for target in TARGETS:
            with tarfile.open(self.folder / f"mitts-{TAG}-{target}.tar.gz") as tar:
                self.assertEqual(
                    {m.name for m in tar.getmembers()},
                    {"mitts", "BUILD.txt", "LICENSE-MIT", "LICENSE-APACHE"},
                )
                body = tar.extractfile("BUILD.txt").read().decode()
                if target not in DARWIN_ARCHES:
                    self.assertNotIn("Signing-Team", body)
        cd64 = {t: h[0] * 64 for t, h in self.cdhashes.items()}
        alt_receipt = {
            "jobId": JOB_ID,
            "status": "Accepted",
            "issues": [],
            "ticketContents": [
                {"path": f"{t}/mitts", "cdhash": cd64[t], "arch": arch}
                for t, arch in DARWIN_ARCHES.items()
            ],
        }
        for target in DARWIN_ARCHES:
            lines = self.darwin_lines(
                target,
                overrides={"CodeDirectory-Hash": cd64[target]},
                receipt=alt_receipt,
            )
            self.write_archive(target, extra_lines=lines)
        with patch.dict(os.environ, {"APPLE_TEAM_ID": TEAM}):
            self.assertEqual(len(validate_notarized(self.folder, TAG, SHA)), 9)

    def test_missing_or_invalid_team_env_and_team_mismatch_are_rejected(self):
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("APPLE_TEAM_ID", None)
            with self.assertRaises(ValueError):
                validate_notarized(self.folder, TAG, SHA)
        for bad_env in ["", "team123456", "TEAM12345", "TEAM1234567", "TEAM12345-"]:
            with self.subTest(env=bad_env):
                self.assert_rejected(team=bad_env)
        for target in DARWIN_ARCHES:
            with self.subTest(target=target):
                self.build_set()
                lines = self.darwin_lines(
                    target, overrides={"Signing-Team": "OTHER12345"}
                )
                self.write_archive(target, extra_lines=lines)
                self.assert_rejected()

    def test_missing_duplicate_and_malformed_markers_are_rejected(self):
        for target in DARWIN_ARCHES:
            for marker in SIGNING_MARKERS:
                with self.subTest(target=target, missing=marker):
                    self.build_set()
                    lines = self.darwin_lines(target, overrides={marker: None})
                    self.write_archive(target, extra_lines=lines)
                    self.assert_rejected()
                with self.subTest(target=target, duplicate=marker):
                    self.build_set()
                    base = self.darwin_lines(target)
                    dup = next(line for line in base if line.startswith(marker + ": "))
                    self.write_archive(target, extra_lines=[*base, dup])
                    self.assert_rejected()
        target = "aarch64-apple-darwin"
        base = self.darwin_lines(target)
        dup_json = (
            f'{{"status":"Invalid","status":"Accepted","jobId":"{JOB_ID}",'
            '"issues":null,"ticketContents":[]}'
        )
        malformed_cases = [
            [s.replace("Signing-Team: ", "Signing-Team:") for s in base],
            [" " + s if s.startswith("Signing-Team:") else s for s in base],
            [s.replace("Signing-Team:", "signing-team:") for s in base],
            [*base, "Signing-Team EXTRA"],
            [s + "\r" for s in base],
            self.darwin_lines(target, overrides={"Signed-Binary-SHA256": "z" * 64}),
            self.darwin_lines(target, overrides={"CodeDirectory-Hash": "C" * 40}),
            self.darwin_lines(target, overrides={"CodeDirectory-Hash": "c" * 48}),
            self.darwin_lines(target, overrides={"Notarization-ID": "not-a-uuid"}),
            self.darwin_lines(target, overrides={"Notarization-Receipt": "not-json"}),
            self.darwin_lines(target, overrides={"Notarization-Receipt": "[]"}),
            self.darwin_lines(target, overrides={"Notarization-Receipt": dup_json}),
        ]
        for index, lines in enumerate(malformed_cases):
            with self.subTest(malformed=index):
                self.build_set()
                self.write_archive(target, extra_lines=lines)
                self.assert_rejected()

    def test_rejected_notarization_status_job_id_and_issues_are_rejected(self):
        target = "x86_64-apple-darwin"
        no_issues = {k: v for k, v in self.receipt_dict().items() if k != "issues"}
        cases = [
            self.darwin_lines(target, overrides={"Notarization-Status": "Invalid"}),
            self.darwin_lines(target, overrides={"Notarization-Status": "In Progress"}),
            self.darwin_lines(target, receipt=self.receipt_dict(status="Invalid")),
            self.darwin_lines(
                target,
                receipt=self.receipt_dict(
                    job_id="00000000-0000-0000-0000-000000000000"
                ),
            ),
            self.darwin_lines(
                target, receipt=self.receipt_dict(issues=[{"severity": "error"}])
            ),
            self.darwin_lines(target, receipt=self.receipt_dict(issues="none")),
            self.darwin_lines(target, receipt=no_issues),
        ]
        for index, lines in enumerate(cases):
            with self.subTest(case=index):
                self.build_set()
                self.write_archive(target, extra_lines=lines)
                self.assert_rejected()

    def test_binary_hash_cdhash_arch_and_ticket_path_mismatches_are_rejected(self):
        target = "aarch64-apple-darwin"
        tampered = self.darwin_lines(target, binary=b"signed-before-tamper")
        self.write_archive(target, binary=b"different-binary", extra_lines=tampered)
        self.assert_rejected()
        bad_receipts = [
            self.receipt_dict(cdhashes={**self.cdhashes, target: "e" * 40}),
            self.receipt_dict(
                tickets=[self.ticket(f"notarize.zip/{target}/mitts", "x86_64")]
            ),
            self.receipt_dict(tickets=[]),
            self.receipt_dict(tickets=[self.ticket("notarize.zip/mitts")]),
            self.receipt_dict(tickets=[self.ticket(f"evil-{target}/mitts")]),
            self.receipt_dict(tickets=[self.ticket(f"../{target}/mitts")]),
            self.receipt_dict(tickets=[self.ticket(f"/{target}/mitts")]),
            self.receipt_dict(
                tickets=[
                    *self.receipt_dict()["ticketContents"],
                    self.ticket(f"second.zip/{target}/mitts"),
                ]
            ),
        ]
        for index, bad_receipt in enumerate(bad_receipts):
            with self.subTest(receipt_case=index):
                self.build_set()
                lines = self.darwin_lines(target, receipt=bad_receipt)
                self.write_archive(target, extra_lines=lines)
                self.assert_rejected()


if __name__ == "__main__":
    unittest.main()
