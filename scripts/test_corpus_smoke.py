# Mitts Build — corpus smoke command selection
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import corpus_smoke


class CorpusSmokeTests(unittest.TestCase):
    def test_project_uses_mitts_and_records_no_comparison(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            calls = []

            def fake_subprocess(args, **kwargs):
                calls.append(args)
                if args[:2] == ["git", "clone"]:
                    Path(args[-1]).mkdir()
                return SimpleNamespace(returncode=0, stdout="fixture-sha")

            def fake_run(args, cwd, env):
                calls.append(args)
                return {"exit_code": 0, "wall_s": 0.1, "tail": ""}

            with patch.object(corpus_smoke.subprocess, "run", side_effect=fake_subprocess), patch.object(corpus_smoke, "run", side_effect=fake_run):
                result = corpus_smoke.smoke_project("demo", "example", root, Path("/bin/mitts"), {})
            self.assertEqual(result["status"], "OK")
            self.assertTrue(all(Path(args[0]).name != "cargo" for args in calls))
            self.assertEqual(set(result["commands"]), {"check", "clippy", "test_no_run"})
            self.assertTrue(all(record["verdict"] == "pass" and "cargo" not in record for record in result["commands"].values()))
