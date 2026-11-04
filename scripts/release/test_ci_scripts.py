# Mitts Build — tests for Mitts-only CI isolation scripts.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def write_executable(path: Path, body: str) -> None:
    path.write_text(f"#!{sys.executable}\n" + textwrap.dedent(body))
    path.chmod(0o755)


class MittsOnlyIsolationScriptsTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.tmp_dir = self.root / "tmp with spaces"
        self.tmp_dir.mkdir()
        self.log_file = self.root / "events.jsonl"
        commands = self.root / "bin"
        commands.mkdir()
        self.fake_mitts = commands / "mitts"
        write_executable(
            self.fake_mitts,
            """\
            import json, os, pathlib, sys
            args = sys.argv[1:]
            if not args:
                sys.exit(2)
            mode = args[0]
            target = args[args.index("--test") + 1] if "--test" in args else "unit"
            leaked = [key for key in os.environ if key.startswith("MITTS_")]
            with pathlib.Path(os.environ["FAKE_LOG"]).open("a") as log:
                log.write(json.dumps({
                    "command": mode,
                    "args": args,
                    "home": os.environ.get("HOME"),
                    "xdg": os.environ.get("XDG_CACHE_HOME"),
                    "leaked": leaked,
                    "target": target,
                }) + "\\n")
            if mode == "metadata":
                if os.environ.get("FAKE_MODE") == "readonly_cache":
                    cache = pathlib.Path(args[args.index("--cache-dir") + 1])
                    sealed = cache / "sealed"
                    sealed.mkdir(parents=True)
                    (sealed / "object").write_text("fixture")
                    sealed.chmod(0o555)
                if os.environ.get("FAKE_MODE") == "controller_writer":
                    store = pathlib.Path(os.environ["XDG_CACHE_HOME"]) / "mitts"
                    store.mkdir(parents=True, exist_ok=True)
                    (store / "controller-entry").write_text("unexpected")
                root = pathlib.Path(os.environ["FAKE_REPO"])
                print(json.dumps({"packages": [{
                    "manifest_path": str(root / "Cargo.toml"),
                    "targets": [] if os.environ.get("FAKE_MODE") == "empty_metadata" else [
                        {"name": "alpha", "kind": ["test"]},
                        {"name": "beta", "kind": ["test"]},
                    ],
                }]}))
                sys.exit(0)
            if mode != "test":
                sys.exit(3)
            if os.environ.get("FAKE_MODE") == "cargo_probe":
                import subprocess
                sys.exit(subprocess.run(["cargo", "--version"]).returncode)
            if os.environ.get("FAKE_MODE") == "writer" and target == "alpha":
                store = pathlib.Path(os.environ["XDG_CACHE_HOME"]) / "mitts"
                store.mkdir(parents=True, exist_ok=True)
                (store / "target-entry").write_text("unexpected")
            if os.environ.get("FAKE_MODE") == "fail" and target == "beta":
                sys.exit(41)
            sys.exit(0)
            """,
        )

        self.env = {key: value for key, value in os.environ.items() if not key.startswith("MITTS_")}
        self.env.update(
            PATH=f"{commands}:{os.environ.get('PATH', '')}",
            HOME=str(self.root / "caller-home"),
            XDG_CACHE_HOME=str(self.root / "caller-xdg"),
            TMPDIR=str(self.tmp_dir),
            FAKE_LOG=str(self.log_file),
            FAKE_REPO=str(ROOT),
            MITTS_CI_BIN=str(self.fake_mitts),
            MITTS_CI_CACHE_DIR=str(self.root / "shared-cache"),
        )

    def run_script(self, name, *targets):
        return subprocess.run(
            ["/bin/bash", str(ROOT / "scripts" / name), *targets],
            env=self.env,
            text=True,
            capture_output=True,
            check=False,
        )

    def read_events(self):
        if not self.log_file.exists():
            return []
        return [json.loads(line) for line in self.log_file.read_text().splitlines()]

    def test_probe_uses_mitts_and_separate_sandboxes(self):
        self.env["MITTS_CACHE_DIR"] = str(self.root / "inherited-store")
        result = self.run_script("probe_store_isolation.sh", "alpha", "beta")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0 target(s) wrote", result.stdout)
        events = self.read_events()
        self.assertEqual(sum(event["command"] == "metadata" for event in events), 1)
        tests = [event for event in events if event["command"] == "test"]
        self.assertEqual(len(tests), 3)  # library/binary tests plus alpha and beta
        self.assertEqual({event["target"] for event in tests}, {"unit", "alpha", "beta"})
        self.assertEqual(len({event["home"] for event in tests}), 3)
        caches = [event["args"][event["args"].index("--cache-dir") + 1] for event in tests]
        self.assertEqual(len(set(caches)), 1)
        self.assertTrue(all(cache not in event["home"] for cache, event in zip(caches, tests)))
        self.assertTrue(all(not event["leaked"] for event in events))
        self.assertEqual(list(self.tmp_dir.iterdir()), [])

    def probe_targets(self, shard):
        self.log_file.unlink(missing_ok=True)
        if shard is None:
            self.env.pop("MITTS_PROBE_SHARD", None)
        else:
            self.env["MITTS_PROBE_SHARD"] = shard
        result = self.run_script("probe_store_isolation.sh", "delta", "alpha", "gamma", "beta", "epsilon")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        tests = [event["target"] for event in self.read_events() if event["command"] == "test"]
        return tests, result

    def test_probe_shards_partition_sorted_targets_and_run_unit_tests_once(self):
        first, first_result = self.probe_targets("1/2")
        second, _ = self.probe_targets("2/2")
        # Sorted: alpha beta delta epsilon gamma, dealt round-robin.
        self.assertEqual(first, ["unit", "alpha", "delta", "gamma"])
        self.assertEqual(second, ["beta", "epsilon"])
        self.assertIn("shard 1/2 runs 3 integration target(s)", first_result.stdout)
        whole, _ = self.probe_targets(None)
        self.assertEqual(sorted(first + second), sorted(whole))

    def test_probe_single_shard_matches_unsharded_run(self):
        whole, _ = self.probe_targets(None)
        single, _ = self.probe_targets("1/1")
        self.assertEqual(single, whole)

    def test_probe_more_shards_than_targets_leaves_empty_shards_green(self):
        seen = []
        for index in range(1, 8):
            tests, _ = self.probe_targets(f"{index}/7")
            seen.extend(tests)
        self.assertEqual(sorted(seen), sorted(["unit", "alpha", "beta", "delta", "epsilon", "gamma"]))

    def test_probe_rejects_malformed_shard_selectors(self):
        for bad in ("3", "0/2", "3/2", "1/0", "a/2", "1/2/3", "/2", "1/", "-1/2"):
            with self.subTest(bad=bad):
                self.log_file.unlink(missing_ok=True)
                self.env["MITTS_PROBE_SHARD"] = bad
                result = self.run_script("probe_store_isolation.sh", "alpha", "beta")
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn("MITTS_PROBE_SHARD", result.stderr)
                self.assertFalse(any(event["command"] == "test" for event in self.read_events()))

    def test_probe_rejects_a_target_writing_the_default_store(self):
        self.env["FAKE_MODE"] = "writer"
        result = self.run_script("probe_store_isolation.sh", "alpha", "beta")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("WRITES alpha", result.stdout)
        self.assertIn("clean  beta", result.stdout)

    def test_probe_rejects_controller_metadata_writing_the_default_store(self):
        self.env["FAKE_MODE"] = "controller_writer"
        result = self.run_script("probe_store_isolation.sh", "alpha")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("metadata touched a default store", result.stderr)

    def test_probe_rejects_missing_integration_targets(self):
        self.env["FAKE_MODE"] = "empty_metadata"
        result = self.run_script("probe_store_isolation.sh")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("metadata omitted integration test targets", result.stderr)
        self.assertFalse(any(event["command"] == "test" for event in self.read_events()))

    def test_probe_rejects_partially_missing_integration_targets(self):
        result = self.run_script("probe_store_isolation.sh")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("metadata omitted integration test targets", result.stderr)
        self.assertFalse(any(event["command"] == "test" for event in self.read_events()))

    def test_probe_rejects_cargo_cli_invocation(self):
        self.env["FAKE_MODE"] = "cargo_probe"
        result = self.run_script("probe_store_isolation.sh", "alpha")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Cargo CLI is forbidden", result.stdout)

    def test_probe_cleans_read_only_cache_directories(self):
        self.env["FAKE_MODE"] = "readonly_cache"
        result = self.run_script("probe_store_isolation.sh", "alpha")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(list(self.tmp_dir.iterdir()), [])

    def test_isolated_runner_uses_mitts_and_preserves_test_failure_status(self):
        self.env["FAKE_MODE"] = "fail"
        result = self.run_script("run_tests_isolated.sh", "--test", "beta")
        self.assertEqual(result.returncode, 41, result.stdout + result.stderr)
        events = self.read_events()
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]["command"], "test")
        self.assertIn("--cache-dir", events[0]["args"])
        self.assertFalse(events[0]["leaked"])


if __name__ == "__main__":
    unittest.main()
