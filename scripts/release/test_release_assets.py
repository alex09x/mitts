# Mitts Build — complete archive and checksum manifest regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import io
import tarfile
import tempfile
import unittest
from pathlib import Path

from assets import TARGETS, digest, validate

TAG = "v0.4.0"
SHA = "a" * 40


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
            "BUILD.txt": f"Mitts Build {TAG}\nTarget: {target}\nSource: https://github.com/alex09x/mitts/tree/{sha}\n".encode(),
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
