# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Diagnostics parity keys: one generated file, two build-script output locations."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from benchmark_diagnostics import _rel_file  # noqa: E402


class RelFileTests(unittest.TestCase):
    def test_workspace_files_are_workspace_relative(self):
        ws = Path("/w/proj")
        self.assertEqual(_rel_file("/w/proj/src/lib.rs", ws), "src/lib.rs")
        self.assertEqual(_rel_file("src/lib.rs", ws), "src/lib.rs")

    def test_a_build_script_output_keys_the_same_for_cargo_and_mitts(self):
        """tokei includes `$OUT_DIR/language_type.rs`: Cargo prints its target dir, Mitts
        its remapped output prefix; the diagnostic inside must compare equal."""
        ws = Path("/w/proj")
        cargo = _rel_file("/w/proj/target/debug/build/tokei-0123abcd/out/language_type.rs", ws)
        mitts = _rel_file("/mitts/out/tokei/language_type.rs", ws)
        self.assertEqual(cargo, "OUT_DIR/language_type.rs")
        self.assertEqual(mitts, "OUT_DIR/language_type.rs")
        cache = _rel_file("/cache/cas/build_scripts/abc/def/debug/build/tokei/out/sub/gen.rs", ws)
        self.assertEqual(cache, "OUT_DIR/sub/gen.rs")

    def test_an_out_directory_in_the_source_tree_stays_a_source_path(self):
        ws = Path("/w/proj")
        self.assertEqual(_rel_file("src/out/mod.rs", ws), "src/out/mod.rs")


if __name__ == "__main__":
    unittest.main()
