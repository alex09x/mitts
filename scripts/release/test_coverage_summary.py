# Mitts Build — coverage summary scope and counter regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import tempfile
import unittest
from pathlib import Path

from coverage_summary import summarize


class CoverageSummaryTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("first-party fixture\n")
        (self.root / "tests").mkdir()
        (self.root / "tests/fixture.rs").write_text("test fixture\n")
        self.lcov = self.root / "lcov.info"

    def test_first_party_only_and_duplicate_records_are_merged(self):
        self.lcov.write_text(
            "SF:src/lib.rs\nDA:1,0\nDA:2,3\nend_of_record\n"
            "SF:src/lib.rs\nDA:1,2\nDA:2,0\nDA:3,0\nend_of_record\n"
            "SF:tests/fixture.rs\nDA:1,1\nend_of_record\n"
            "SF:/external/dependency/src/lib.rs\nDA:1,0\nend_of_record\n"
        )
        result, filtered = summarize(self.lcov, self.root)
        self.assertEqual(
            (result["files"], result["covered"], result["lines"]), (1, 2, 3)
        )
        self.assertAlmostEqual(result["percent"], 200 / 3)
        self.assertNotIn("external", filtered)
        self.assertNotIn("tests/", filtered)

    def test_absolute_source_paths_are_supported(self):
        self.lcov.write_text(f"SF:{self.root}/src/lib.rs\nDA:1,2\nend_of_record\n")
        self.assertEqual(summarize(self.lcov, self.root)[0]["percent"], 100)

    def test_empty_or_external_only_data_cannot_claim_passing_coverage(self):
        self.lcov.write_text("SF:/external/src/lib.rs\nDA:1,2\nend_of_record\n")
        with self.assertRaises(ValueError):
            summarize(self.lcov, self.root)
