# Mitts Build — historical formatter baseline safety regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import hashlib
import tempfile
import unittest
from pathlib import Path

from check_format import BASELINE_SHA, historical_differences


class FormattingBaselineTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.file = self.root / "historical.rs"
        self.original = b"fn fixture( ) {}\n"
        self.file.write_bytes(self.original)
        self.baseline = {
            "source_sha": BASELINE_SHA,
            "files": {self.file.name: hashlib.sha256(self.original).hexdigest()},
        }
        self.output = f"Diff in {self.file}:1:\n formatting diff\n"

    def check(self, blob=None):
        return historical_differences(
            self.root,
            self.baseline,
            self.output,
            lambda path: self.original if blob is None else blob,
        )

    def test_identical_released_file_is_allowed(self):
        self.assertEqual(self.check(), {"historical.rs"})

    def test_changed_file_cannot_use_the_waiver(self):
        self.file.write_bytes(b"fn changed( ) {}\n")
        with self.assertRaises(ValueError):
            self.check()

    def test_new_file_cannot_use_the_waiver(self):
        self.baseline["files"] = {}
        with self.assertRaises(ValueError):
            self.check()

    def test_manifest_cannot_replace_the_published_blob(self):
        with self.assertRaises(ValueError):
            self.check(blob=b"different released content")

    def test_other_formatter_errors_are_not_accepted(self):
        self.output = "parse error"
        with self.assertRaises(ValueError):
            self.check()
