# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Injection-target selection: the defect must land in a test root every arm builds."""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from t5_parsers import select_workspace_targets  # noqa: E402


class InjectionTargetTests(unittest.TestCase):
    def test_a_test_target_behind_required_features_is_not_an_injection_root(self):
        """reqwest's tests/blocking.rs needs `blocking`: with default features it is never
        built, so a defect injected there was caught by no tool (INVALID_T5)."""
        with tempfile.TemporaryDirectory() as tmp:
            ws = Path(tmp)
            (ws / "src").mkdir()
            (ws / "tests").mkdir()
            (ws / "Cargo.toml").write_text(
                '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
                '[features]\nblocking = []\n\n'
                '[[test]]\nname = "a_gated"\npath = "tests/a_gated.rs"\nrequired-features = ["blocking"]\n'
            )
            (ws / "src/lib.rs").write_text("pub fn f() {}\n")
            (ws / "tests/a_gated.rs").write_text("#[test]\nfn gated() {}\n")
            (ws / "tests/b_plain.rs").write_text("#[test]\nfn plain() {}\n")
            info = select_workspace_targets(ws)
            self.assertEqual(info["test_src"], (ws / "tests/b_plain.rs").resolve())
            self.assertEqual(info["lib_or_bin_src"], (ws / "src/lib.rs").resolve())


if __name__ == "__main__":
    unittest.main()
