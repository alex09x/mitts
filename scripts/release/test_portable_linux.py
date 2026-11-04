#!/usr/bin/env python3
# Mitts Build — portable Linux release checks.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import shutil


ROOT = Path(__file__).resolve().parents[2]


class PortableLinuxTests(unittest.TestCase):
    def test_glibc_floor_rejects_newer_and_missing_versions(self):
        with tempfile.TemporaryDirectory() as tmp:
            fake = Path(tmp) / "readelf"
            fake.write_text('#!/bin/sh\nprintf "%s\\n" "$FAKE_READELF"\n')
            fake.chmod(0o755)
            for output, expected in [("GLIBC_2.17 GLIBC_2.34", 0),
                                     ("GLIBC_2.9 GLIBC_2.35", 1),
                                     ("no symbol versions", 1)]:
                env = {**os.environ, "PATH": f"{tmp}:{os.environ['PATH']}", "FAKE_READELF": output}
                result = subprocess.run(["bash", str(ROOT / "scripts/release/check-glibc.sh"), "binary"],
                                        env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stderr)

    @unittest.skipUnless(os.uname().sysname == "Linux", "Linux loader required")
    def test_missing_bootstrap_library_stops_before_docker(self):
        target = {"aarch64": "aarch64-unknown-linux-gnu", "x86_64": "x86_64-unknown-linux-gnu"}.get(os.uname().machine)
        if target is None:
            self.skipTest("unsupported architecture")
        with tempfile.TemporaryDirectory(prefix="mitts-loader-test-") as tmp:
            root = Path(tmp)
            commands = root / "commands"
            commands.mkdir()
            fake_ldd = commands / "ldd"
            fake_ldd.write_text('#!/bin/sh\necho "libmissing.so => not found"\n')
            fake_ldd.chmod(0o755)
            fake_docker = commands / "docker"
            fake_docker.write_text('#!/bin/sh\ntouch "$DOCKER_CALLED"\nexit 0\n')
            fake_docker.chmod(0o755)
            env = {**os.environ, "PATH": f"{commands}:{os.environ['PATH']}", "RUNNER_TEMP": tmp,
                   "MITTS_CI_BIN": str(root / "binary"), "MITTS_CI_CACHE_DIR": str(root / "cache"),
                   "DOCKER_CALLED": str(root / "docker-called")}
            result = subprocess.run(["bash", str(ROOT / "scripts/release/package-linux-portable.sh"), tmp,
                                     "v0.1.0", target, str(root / "dist")], cwd=ROOT, env=env,
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("bootstrap library is missing", result.stderr)
            self.assertFalse((root / "docker-called").exists())

    @unittest.skipUnless(os.environ.get("MITTS_PORTABLE_INTEGRATION") == "1", "requires Docker and a Mitts bootstrap")
    def test_container_packages_and_installs_a_real_gnu_binary(self):
        bootstrap = shutil.which("mitts")
        self.assertIsNotNone(bootstrap)
        target = {"aarch64": "aarch64-unknown-linux-gnu", "x86_64": "x86_64-unknown-linux-gnu"}.get(os.uname().machine)
        self.assertIsNotNone(target)
        with tempfile.TemporaryDirectory(prefix="mitts-portable-test-") as tmp:
            root = Path(tmp)
            workspace = root / "workspace"
            source = root / "source"
            runner = root / "runner"
            runner.mkdir()
            source.joinpath("src").mkdir(parents=True)
            workspace.joinpath("scripts/release").mkdir(parents=True)
            workspace.joinpath("site").mkdir()
            for name in ["package.sh", "package-linux-portable.sh", "check-glibc.sh", "test_installer.py", "amazonlinux.Dockerfile"]:
                shutil.copy(ROOT / "scripts/release" / name, workspace / "scripts/release" / name)
            shutil.copy(ROOT / "site/install.sh", workspace / "site/install.sh")
            source.joinpath("scripts/release").mkdir(parents=True)
            shutil.copy(ROOT / "scripts/release/check-glibc.sh", source / "scripts/release/check-glibc.sh")
            source.joinpath("Cargo.toml").write_text('[package]\nname = "mitts"\nversion = "0.1.0"\nedition = "2021"\n')
            source.joinpath("Cargo.lock").write_text('version = 3\n\n[[package]]\nname = "mitts"\nversion = "0.1.0"\n')
            source.joinpath("build.rs").write_text('fn main() { let o = std::process::Command::new("git").args(["rev-parse", "HEAD"]).output().unwrap(); println!("cargo:rustc-env=TEST_SHA={}", String::from_utf8(o.stdout).unwrap().trim()); }\n')
            source.joinpath("src/main.rs").write_text('fn main() { if std::env::args().any(|a| a == "--version") { println!("mitts 0.1.0 ({})", env!("TEST_SHA")); } }\n')
            for name in ["LICENSE-MIT", "LICENSE-APACHE"]:
                source.joinpath(name).write_text("fixture license\n")
            env = {**os.environ, "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.test", "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.test"}
            for argv in [["init", "-q"], ["add", "."], ["commit", "-qm", "fixture"], ["tag", "v0.1.0"]]:
                subprocess.run(["git", "-C", str(source), *argv], env=env, check=True)
            shutil.copy(bootstrap, runner / "mitts")
            env.update(RUNNER_TEMP=str(runner), MITTS_CI_BIN=str(runner / "mitts"), MITTS_CI_CACHE_DIR=str(runner / "cache"))
            result = subprocess.run(["bash", "scripts/release/package-linux-portable.sh", str(source), "v0.1.0", target, str(root / "dist")], cwd=workspace, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, (result.stdout + result.stderr)[-5000:])
            self.assertIn("Native archive install passed", result.stdout)


if __name__ == "__main__":
    unittest.main()
