# Mitts Build — benchmark command selection
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

import unittest
from pathlib import Path

from benchmark_lib import run_timed, tool_cmd


class BenchmarkCommandTests(unittest.TestCase):
    def test_mitts_command_and_reject_other_tools(self):
        ctx = {"mitts_bin": Path("/tmp/mitts"), "mitts_cas": Path("/tmp/cache")}
        self.assertEqual(tool_cmd("mitts_cas", "build", ctx)[:2], ["/tmp/mitts", "build"])
        for arm in ("cargo_bare", "cargo_sccache", "cargo_shared_target"):
            with self.subTest(arm=arm), self.assertRaisesRegex(ValueError, "Mitts-only"):
                tool_cmd(arm, "build", ctx)

    def test_timing_runner_rejects_direct_cargo_command(self):
        with self.assertRaisesRegex(ValueError, "Mitts-only"):
            run_timed(["cargo", "--version"], Path("."), {})
