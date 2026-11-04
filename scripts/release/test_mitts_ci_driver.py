# Mitts Build — native CI driver selection and isolation regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import unittest

import test_ci_scripts as fixtures


class MittsDriverTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.MittsOnlyIsolationScriptsTests(
            "test_isolated_runner_uses_mitts_and_preserves_test_failure_status"
        )
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        binary = self.fixture.root / "native mitts"
        fixtures.write_executable(
            binary,
            """\
            import json, os, pathlib, sys
            args = sys.argv[1:]
            if args[0] not in ("test", "coverage") or "--workspace" not in args or "--cache-dir" not in args:
                sys.exit(98)
            if any(k.startswith("MITTS_") for k in os.environ):
                sys.exit(97)
            if any(os.environ.get(k) != "" for k in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")):
                sys.exit(95)
            home = pathlib.Path(os.environ["HOME"])
            if not home.is_dir() or pathlib.Path(os.environ["XDG_CACHE_HOME"]).exists():
                sys.exit(96)
            with pathlib.Path(os.environ["FAKE_LOG"]).open("a") as log:
                log.write(json.dumps({"proc":"mitts", "args":args, "home":str(home)}) + "\\n")
            sys.exit(41 if os.environ.get("FAKE_MODE") == "fail" else 0)
        """,
        )
        self.fixture.env.update(MITTS_TEST_DRIVER="mitts", MITTS_CI_BIN=str(binary), RUSTC_WRAPPER="ambient-wrapper")

    def test_native_driver_runs_directly_in_clean_home(self):
        result = self.fixture.run_script("run_tests_isolated.sh", "--no-fail-fast")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        events = self.fixture.read_events()
        self.assertEqual([event["proc"] for event in events], ["mitts"])
        self.assertIn("--no-fail-fast", events[0]["args"])
        self.assertIn("real store", result.stdout)

    def test_native_failure_is_not_hidden_by_cargo_fallback(self):
        self.fixture.env["FAKE_MODE"] = "fail"
        result = self.fixture.run_script("run_tests_isolated.sh", "--no-fail-fast")
        self.assertEqual(result.returncode, 41, result.stdout + result.stderr)
        self.assertEqual(
            [event["proc"] for event in self.fixture.read_events()], ["mitts"]
        )

    def test_coverage_uses_the_same_native_driver_and_isolation(self):
        self.fixture.env["MITTS_TEST_COMMAND"] = "coverage"
        result = self.fixture.run_script("run_tests_isolated.sh", "--lcov")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.fixture.read_events()[0]["args"][0], "coverage")
