# Mitts Build — isolated CI runner controls and Cargo runner regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FAKE_HOST = "fake-unix-host"


def write_executable(path: Path, body: str) -> None:
    path.write_text(f"#!{sys.executable}\n" + textwrap.dedent(body))
    path.chmod(0o755)


class IsolationRunnerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.tmp_dir = self.root / "tmp with spaces"
        self.tmp_dir.mkdir()
        self.log_file = self.root / "events.jsonl"
        commands = self.root / "bin"
        commands.mkdir()

        write_executable(
            commands / "rustc",
            f"""\
            import json, os, pathlib, sys
            if "-vV" not in sys.argv:
                sys.exit(2)
            with pathlib.Path(os.environ["FAKE_LOG"]).open("a") as fh:
                fh.write(json.dumps({{"proc": "rustc", "home": os.environ["HOME"]}}) + "\\n")
            print("rustc 1.98.0\\nbinary: rustc\\nhost: {FAKE_HOST}\\nrelease: 1.98.0")
            """,
        )
        fake_test = commands / "fake-test-bin"
        write_executable(
            fake_test,
            """\
            import json, os, pathlib, sys
            mitts = [k for k in os.environ if k.startswith("MITTS_")]
            runners = [k for k in os.environ if k.startswith("CARGO_TARGET_") and k.endswith("_RUNNER")]
            if mitts or runners or "CARGO_MANIFEST_DIR" not in os.environ:
                sys.stderr.write(f"leaked={mitts} runners={runners}\\n")
                sys.exit(97)
            with pathlib.Path(os.environ["FAKE_LOG"]).open("a") as fh:
                fh.write(json.dumps({"proc": "test", "home": os.environ["HOME"], "xdg": os.environ["XDG_CACHE_HOME"]}) + "\\n")
            mode = os.environ.get("FAKE_MODE", "")
            if mode == "writer":
                store = pathlib.Path(os.environ["XDG_CACHE_HOME"]) / "mitts"
                store.mkdir(parents=True, exist_ok=True)
                (store / "entry").write_text("unexpected store")
            sys.exit(41 if mode == "fail" else 0)
            """,
        )
        write_executable(
            commands / "cargo",
            f"""\
            import json, os, pathlib, subprocess, sys
            args, runner = sys.argv[1:], None
            if "--config" in args:
                lines = [s.strip() for s in pathlib.Path(args[args.index("--config") + 1]).read_text().splitlines() if s.strip()]
                if len(lines) != 2 or lines[0] not in ('[target."{FAKE_HOST}"]', '[target.{FAKE_HOST}]'):
                    sys.exit(95)
                if not lines[1].startswith("runner = "):
                    sys.exit(96)
                runner = json.loads(lines[1][len("runner = "):])
                if not isinstance(runner, list) or runner[0] != "/bin/bash":
                    sys.exit(96)
            mode = os.environ.get("FAKE_MODE", "")
            with pathlib.Path(os.environ["FAKE_LOG"]).open("a") as fh:
                fh.write(json.dumps({{"proc": "cargo", "home": os.environ["HOME"], "no_run": "--no-run" in args}}) + "\\n")
            if mode == "controller_writer":
                store = pathlib.Path(os.environ["XDG_CACHE_HOME"]) / "mitts"
                store.mkdir(parents=True, exist_ok=True)
                (store / "ctl").write_text("controller store")
            if "--no-run" in args:
                sys.exit(0)
            if mode == "controller_late_writer":
                store = pathlib.Path(os.environ["HOME"]) / ".cache" / "mitts"
                store.mkdir(parents=True, exist_ok=True)
                (store / "ctl").write_text("controller late store")
            child_env = {{**os.environ, "CARGO_MANIFEST_DIR": os.getcwd()}}
            cmd = [*runner, {str(fake_test)!r}] if runner is not None else [{str(fake_test)!r}]
            sys.exit(subprocess.run(cmd, env=child_env).returncode)
            """,
        )

        self.env = {k: v for k, v in os.environ.items() if not k.startswith("MITTS_")}
        self.env.update(
            PATH=f"{commands}:{os.environ['PATH']}",
            HOME=str(self.root / "home"),
            XDG_CACHE_HOME=str(self.root / "real-xdg"),
            TMPDIR=str(self.tmp_dir),
            FAKE_LOG=str(self.log_file),
        )

    def run_script(self, name, *targets):
        return subprocess.run(
            ["/bin/bash", str(ROOT / "scripts" / name), *(targets or ("fixture",))],
            env=self.env,
            text=True,
            capture_output=True,
            check=False,
        )

    def read_events(self):
        return [
            json.loads(line)
            for line in self.log_file.read_text().splitlines()
            if line.strip()
        ]

    def test_probe_accepts_empty_mitts_environment_on_system_bash(self):
        result = self.run_script("probe_store_isolation.sh", "alpha", "beta")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0 target(s) wrote", result.stdout)
        events = self.read_events()
        rustc_events = [e for e in events if e["proc"] == "rustc"]
        cargo_events = [e for e in events if e["proc"] == "cargo"]
        test_events = [e for e in events if e["proc"] == "test"]
        self.assertEqual(len(rustc_events), 1)
        self.assertEqual(len(cargo_events), 3)
        self.assertEqual(len({e["home"] for e in cargo_events}), 1)
        self.assertEqual(len(test_events), 2)
        self.assertNotEqual(test_events[0]["home"], test_events[1]["home"])
        self.assertNotEqual(test_events[0]["home"], cargo_events[0]["home"])
        self.assertEqual(list(self.tmp_dir.iterdir()), [])

    def test_probe_strips_inherited_mitts_variables(self):
        self.env["MITTS_CACHE_DIR"] = str(self.root / "inherited-store")
        self.env["MITTS_PROBE_SANDBOX"] = str(self.root / "caller-sandbox")
        result = self.run_script("probe_store_isolation.sh")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("clean  fixture", result.stdout)

    def test_probe_rejects_a_target_writing_the_default_store(self):
        self.env["FAKE_MODE"] = "writer"
        result = self.run_script("probe_store_isolation.sh")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("WRITES fixture", result.stdout)
        self.assertEqual(list(self.tmp_dir.iterdir()), [])

    def test_probe_rejects_controller_phase_writing_default_store(self):
        for mode in ("controller_writer", "controller_late_writer"):
            with self.subTest(mode=mode):
                self.env["FAKE_MODE"] = mode
                result = self.run_script("probe_store_isolation.sh")
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("WRITES controller", result.stdout)
                self.assertEqual(list(self.tmp_dir.iterdir()), [])

    def test_probe_reports_target_failure_and_cleans_tempdirs(self):
        self.env["FAKE_MODE"] = "fail"
        result = self.run_script("probe_store_isolation.sh")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FAIL fixture rc=41", result.stdout)
        self.assertIn("1 failed", result.stdout)
        self.assertEqual(list(self.tmp_dir.iterdir()), [])

    def test_isolated_runner_preserves_test_failure_status(self):
        self.env["FAKE_MODE"] = "fail"
        result = self.run_script("run_tests_isolated.sh")
        self.assertEqual(result.returncode, 41, result.stdout + result.stderr)


class RealCargoProbeIsolationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.tmp_dir = self.root / "tmp with spaces"
        self.caller_home = self.root / "caller-home"
        self.caller_xdg = self.root / "caller-xdg"
        self.cargo_home = self.root / "cargo-home"
        self.records_dir = self.root / "records"
        for directory in (
            self.tmp_dir,
            self.caller_home,
            self.caller_xdg,
            self.cargo_home,
            self.records_dir,
        ):
            directory.mkdir()
        self.counter_file = self.root / "build-count.txt"
        self.fixture = self.root / "fixture"
        for sub in ("src", "tests", ".ci-tools"):
            (self.fixture / sub).mkdir(parents=True)
        self.probe = self.fixture / ".ci-tools" / "probe.sh"
        shutil.copy2(ROOT / "scripts" / "probe_store_isolation.sh", self.probe)

        (self.fixture / "Cargo.toml").write_text(
            '[package]\nname = "probe-fixture"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        (self.fixture / "build.rs").write_text(
            textwrap.dedent(
                """\
                use std::{env, fs};

                fn main() {
                    println!("cargo:rerun-if-env-changed=HOME");
                    let path = env::var("FIXTURE_BUILD_COUNTER").expect("FIXTURE_BUILD_COUNTER");
                    let count = fs::read_to_string(&path)
                        .ok()
                        .and_then(|value| value.trim().parse::<u32>().ok())
                        .unwrap_or(0);
                    fs::write(&path, format!("{}\\n", count + 1)).expect("write build counter");
                }
                """
            )
        )
        (self.fixture / "src" / "lib.rs").write_text(
            textwrap.dedent(
                """\
                use std::path::Path;
                use std::{env, fs};

                pub fn verify_and_record(target: &str) {
                    let home = env::var("HOME").expect("HOME");
                    let xdg = env::var("XDG_CACHE_HOME").expect("XDG_CACHE_HOME");
                    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");

                    let home_path = Path::new(&home);
                    assert!(home_path.is_dir(), "HOME directory missing");
                    assert_eq!(fs::read_dir(home_path).expect("read HOME").count(), 0);
                    assert!(!Path::new(&xdg).exists(), "XDG_CACHE_HOME pre-existed");
                    fs::write(home_path.join("used-marker"), target).expect("write marker");

                    let mitts: Vec<_> = env::vars().map(|(k, _)| k).filter(|k| k.starts_with("MITTS_")).collect();
                    let runners: Vec<_> = env::vars()
                        .map(|(k, _)| k)
                        .filter(|k| k.starts_with("CARGO_TARGET_") && k.ends_with("_RUNNER"))
                        .collect();
                    assert!(mitts.is_empty(), "leaked MITTS vars: {mitts:?}");
                    assert!(runners.is_empty(), "exposed runner vars: {runners:?}");

                    if env::var("FIXTURE_WRITER_TARGET").as_deref() == Ok(target) {
                        for store in [home_path.join(".cache/mitts"), Path::new(&xdg).join("mitts")] {
                            fs::create_dir_all(&store).expect("create store");
                            fs::write(store.join("leak"), "1").expect("write leak");
                        }
                    }

                    let records = env::var("FIXTURE_RECORDS_DIR").expect("FIXTURE_RECORDS_DIR");
                    let body = format!("home={home}\\nxdg={xdg}\\nmanifest_dir={manifest_dir}\\nmitts_count={}\\n", mitts.len());
                    fs::write(Path::new(&records).join(format!("{target}.env")), body).expect("write record");
                }

                #[cfg(test)]
                mod tests {
                    #[test]
                    fn unit_target_env() {
                        super::verify_and_record("unit");
                    }
                }
                """
            )
        )
        for target in ("alpha", "beta"):
            (self.fixture / "tests" / f"{target}.rs").write_text(
                f'#[test]\nfn {target}_env() {{ probe_fixture::verify_and_record("{target}"); }}\n'
            )

        real_rustup = os.environ.get("RUSTUP_HOME") or str(Path.home() / ".rustup")
        cargo_bin = Path.home() / ".cargo" / "bin"
        path_entries = [os.environ.get("PATH", "")]
        if cargo_bin.is_dir():
            path_entries.append(str(cargo_bin))
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("MITTS_")}
        self.env.update(
            PATH=":".join(entry for entry in path_entries if entry),
            HOME=str(self.caller_home),
            XDG_CACHE_HOME=str(self.caller_xdg),
            CARGO_HOME=str(self.cargo_home),
            RUSTUP_HOME=real_rustup,
            TMPDIR=str(self.tmp_dir),
            MITTS_CACHE_DIR=str(self.root / "inherited-mitts-store"),
            MITTS_PROBE_SANDBOX=str(self.root / "inherited-probe-sandbox"),
            FIXTURE_BUILD_COUNTER=str(self.counter_file),
            FIXTURE_RECORDS_DIR=str(self.records_dir),
        )

    def read_record(self, target):
        return dict(
            line.split("=", 1)
            for line in (self.records_dir / f"{target}.env").read_text().splitlines()
        )

    def test_real_cargo_compiles_once_with_fresh_homes_and_manifest_env(self):
        result = subprocess.run(
            ["/bin/bash", str(self.probe)],
            env=self.env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(int(self.counter_file.read_text().strip()), 1)
        for line in ("clean  unit tests", "clean  alpha", "clean  beta"):
            self.assertIn(line, result.stdout)
        self.assertIn("0 target(s) wrote to the default store, 0 failed", result.stdout)

        records = {name: self.read_record(name) for name in ("unit", "alpha", "beta")}
        self.assertEqual(len({rec["home"] for rec in records.values()}), 3)
        self.assertEqual(len({rec["xdg"] for rec in records.values()}), 3)
        for rec in records.values():
            self.assertNotEqual(rec["home"], str(self.caller_home))
            self.assertFalse(Path(rec["home"]).exists())
            self.assertEqual(rec["mitts_count"], "0")
            self.assertEqual(
                Path(rec["manifest_dir"]).resolve(), self.fixture.resolve()
            )
        self.assertEqual(list(self.tmp_dir.iterdir()), [])
        self.assertFalse((self.caller_home / ".cache" / "mitts").exists())
        self.assertFalse((self.caller_xdg / "mitts").exists())

    def test_real_cargo_rejects_writer_target_without_recompiling(self):
        self.env["FIXTURE_WRITER_TARGET"] = "alpha"
        result = subprocess.run(
            ["/bin/bash", str(self.probe), "alpha", "beta"],
            env=self.env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(int(self.counter_file.read_text().strip()), 1)
        for token in (
            "WRITES alpha ->",
            ".cache/mitts[leak,]",
            "xdg/mitts[leak,]",
            "clean  beta",
            "1 target(s) wrote to the default store, 0 failed",
        ):
            self.assertIn(token, result.stdout)
        self.assertNotEqual(
            self.read_record("alpha")["home"], self.read_record("beta")["home"]
        )
        self.assertEqual(list(self.tmp_dir.iterdir()), [])
