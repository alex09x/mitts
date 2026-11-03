# Mitts Build — registry publication and immutable retry regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import hashlib
import io
import json
import os
import tarfile
import tempfile
import unittest
import urllib.error
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import registry

TAG = "v0.3.19"
SHA = "a" * 40
TOKEN = "fixture-private-upload-credential"
MANIFEST = '[package]\nname="mitts"\nversion="0.3.19"\n'
ENV = {
    "GITHUB_ACTIONS": "true",
    "GITHUB_REPOSITORY": "alex09x/mitts",
    "GITHUB_REF": "refs/heads/main",
    "CARGO_REGISTRY_TOKEN": TOKEN,
}


def crate(missing=None, vcs=None, manifest=MANIFEST, duplicate=None):
    files = {
        "Cargo.toml": manifest.encode(),
        ".cargo_vcs_info.json": json.dumps(
            vcs or {"git": {"sha1": SHA}, "path_in_vcs": ""}
        ).encode(),
    }
    files.pop(missing, None)
    result = io.BytesIO()
    with tarfile.open(fileobj=result, mode="w:gz") as archive:
        for name, value in files.items():
            member = tarfile.TarInfo("mitts-0.3.19/" + name)
            member.size = len(value)
            archive.addfile(member, io.BytesIO(value))
            if duplicate == name:
                archive.addfile(member, io.BytesIO(value))
    return result.getvalue()


class RegistryReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.folder = self.root / "verified"
        self.folder.mkdir()
        self.raw = crate()
        self.archive = self.folder / "mitts-0.3.19.crate"
        self.archive.write_bytes(self.raw)
        self.manifest = self.root / "Cargo.toml"
        self.manifest.write_text(MANIFEST)
        self.receipt = registry.prepare(self.folder, TAG, SHA)
        self.entry = {
            "name": "mitts",
            "vers": "0.3.19",
            "cksum": self.receipt["sha256"],
            "yanked": False,
        }

    def test_prepare_binds_actual_bytes_version_and_source(self):
        self.assertEqual(self.receipt["sha256"], hashlib.sha256(self.raw).hexdigest())
        self.assertEqual(self.receipt["source"], SHA)
        self.assertEqual(
            json.loads((self.folder / registry.RECEIPT_NAME).read_text()), self.receipt
        )

    def test_wrong_missing_dirty_and_duplicate_provenance_are_rejected(self):
        cases = [
            crate(missing=".cargo_vcs_info.json"),
            crate(vcs={"git": {"sha1": "b" * 40}}),
            crate(vcs={"git": {"sha1": SHA, "dirty": True}}),
            crate(manifest=MANIFEST.replace('name="mitts"', 'name="other"')),
            crate(manifest=MANIFEST.replace("0.3.19", "0.3.18")),
            crate(duplicate=".cargo_vcs_info.json"),
        ]
        for raw in cases:
            with (
                self.subTest(raw_sha=hashlib.sha256(raw).hexdigest()),
                self.assertRaises(ValueError),
            ):
                registry.validate_crate_bytes(raw, TAG, SHA)

    def test_modified_receipt_is_rejected(self):
        path = self.folder / registry.RECEIPT_NAME
        path.write_text(json.dumps({**self.receipt, "source": "b" * 40}))
        with self.assertRaises(ValueError):
            registry.verify(self.folder, TAG, SHA)

    def test_public_download_index_and_prepared_archive_must_match(self):
        with (
            patch.object(registry, "fetch_index_version", return_value=self.entry),
            patch.object(registry, "http_get", return_value=self.raw),
        ):
            self.assertEqual(registry.verify(self.folder, TAG, SHA), self.receipt)
        for entry, raw in [
            ({**self.entry, "cksum": "0" * 64}, self.raw),
            (self.entry, b"corrupted"),
        ]:
            with (
                patch.object(registry, "fetch_index_version", return_value=entry),
                patch.object(registry, "http_get", return_value=raw),
            ):
                with self.assertRaises(ValueError):
                    registry.verify(self.folder, TAG, SHA)

    def test_matching_retry_is_read_only_and_conflicting_version_cannot_upload(self):
        with (
            patch.dict(os.environ, ENV),
            patch.object(registry, "fetch_index_version", return_value=self.entry),
            patch.object(registry, "http_get", return_value=self.raw),
            patch.object(registry, "_run_cargo_publish") as upload,
        ):
            registry.publish(self.folder, TAG, SHA, self.manifest)
            upload.assert_not_called()
        with (
            patch.dict(os.environ, ENV),
            patch.object(
                registry,
                "fetch_index_version",
                return_value={**self.entry, "cksum": "0" * 64},
            ),
            patch.object(registry, "_run_cargo_publish") as upload,
        ):
            with self.assertRaises(ValueError):
                registry.publish(self.folder, TAG, SHA, self.manifest)
            upload.assert_not_called()

    def test_absent_version_uploads_once_then_requires_public_readback(self):
        with (
            patch.dict(os.environ, ENV),
            patch.object(
                registry, "fetch_index_version", side_effect=[None, None, self.entry]
            ),
            patch.object(registry, "http_get", return_value=self.raw),
            patch.object(registry, "_require_clean_source") as source,
            patch.object(registry, "_run_cargo_publish") as upload,
        ):
            self.assertEqual(
                registry.publish(self.folder, TAG, SHA, self.manifest, interval=0),
                self.receipt,
            )
            source.assert_called_once_with(self.manifest, SHA)
            upload.assert_called_once()

    def test_wrong_checkout_stops_before_upload_and_git_does_not_receive_token(self):
        for output in [("b" * 40, ""), (SHA, " M Cargo.toml")]:
            results = [SimpleNamespace(returncode=0, stdout=value) for value in output]
            with (
                patch.dict(os.environ, ENV),
                patch.object(registry, "fetch_index_version", return_value=None),
                patch.object(registry.subprocess, "run", side_effect=results) as run,
                patch.object(registry, "_run_cargo_publish") as upload,
            ):
                with self.assertRaises(ValueError):
                    registry.publish(self.folder, TAG, SHA, self.manifest)
                upload.assert_not_called()
                for call in run.call_args_list:
                    self.assertNotIn("CARGO_REGISTRY_TOKEN", call.kwargs["env"])

    def test_untrusted_branch_and_missing_credentials_do_not_publish(self):
        for extra in [
            {"GITHUB_REF": "refs/heads/feature"},
            {"CARGO_REGISTRY_TOKEN": ""},
        ]:
            with (
                patch.dict(os.environ, {**ENV, **extra}),
                patch.object(registry, "_run_cargo_publish") as upload,
            ):
                with self.assertRaises(ValueError):
                    registry.publish(self.folder, TAG, SHA, self.manifest)
                upload.assert_not_called()

    def test_failed_upload_redacts_the_ephemeral_token(self):
        with (
            patch.dict(os.environ, ENV),
            patch.object(registry, "fetch_index_version", return_value=None),
            patch.object(registry, "_require_clean_source"),
            patch.object(
                registry,
                "_run_cargo_publish",
                side_effect=RuntimeError("failed " + TOKEN),
            ),
        ):
            with self.assertRaises(RuntimeError) as failure:
                registry.publish(self.folder, TAG, SHA, self.manifest)
            self.assertNotIn(TOKEN, str(failure.exception))

    def test_absence_is_distinguished_from_malformed_or_denied_index(self):
        record = json.dumps(self.entry).encode() + b"\n"
        with patch.object(registry, "http_get", return_value=record):
            self.assertIsNone(registry.fetch_index_version("0.3.20"))
            self.assertEqual(registry.fetch_index_version("0.3.19"), self.entry)
        with patch.object(registry, "http_get", return_value=b"not json"):
            with self.assertRaises(ValueError):
                registry.fetch_index_version("0.3.19")
        denied = urllib.error.HTTPError(registry.INDEX_URL, 403, "denied", {}, None)
        with patch.object(registry.urllib.request, "urlopen", side_effect=denied):
            with self.assertRaises(RuntimeError):
                registry.fetch_index_version("0.3.19")
