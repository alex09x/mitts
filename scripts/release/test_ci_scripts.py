# Mitts Build — isolated CI runner controls without compiling Rust.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class IsolationRunnerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        commands = self.root / "bin"
        commands.mkdir()
        cargo = commands / "cargo"
        cargo.write_text(
            f"#!{sys.executable}\n"
            "import os,pathlib,sys\n"
            "if '--no-run' in sys.argv: sys.exit(0)\n"
            "if os.environ.get('FAKE_MODE')=='writer':\n"
            " p=pathlib.Path(os.environ['XDG_CACHE_HOME'])/'mitts'; p.mkdir(parents=True)\n"
            " (p/'entry').write_text('unexpected store')\n"
            "sys.exit(41 if os.environ.get('FAKE_MODE')=='fail' else 0)\n"
        )
        cargo.chmod(0o755)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("MITTS_")}
        self.env.update(
            PATH=f"{commands}:{os.environ['PATH']}",
            HOME=str(self.root / "home"),
            XDG_CACHE_HOME=str(self.root / "real-xdg"),
            TMPDIR=str(self.root),
        )

    def run_script(self, name):
        return subprocess.run(
            ["/bin/bash", str(ROOT / "scripts" / name), "fixture"],
            env=self.env,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_probe_accepts_empty_mitts_environment_on_system_bash(self):
        result = self.run_script("probe_store_isolation.sh")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0 target(s) wrote", result.stdout)

    def test_probe_rejects_a_target_writing_the_default_store(self):
        self.env["FAKE_MODE"] = "writer"
        result = self.run_script("probe_store_isolation.sh")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("WRITES fixture", result.stdout)

    def test_isolated_runner_preserves_test_failure_status(self):
        self.env["FAKE_MODE"] = "fail"
        result = self.run_script("run_tests_isolated.sh")
        self.assertEqual(result.returncode, 41, result.stdout + result.stderr)
