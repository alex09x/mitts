# Mitts Build — optional provenance inputs must not force every Cargo invocation.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class BuildMetadataTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.project = self.root / "project"
        (self.project / "src").mkdir(parents=True)
        self.counter = self.root / "counter"
        (self.project / "Cargo.toml").write_text(
            '[package]\nname="metadata-fixture"\nversion="0.1.0"\nedition="2021"\n'
        )
        (self.project / "Cargo.lock").write_text(
            'version=4\n[[package]]\nname="metadata-fixture"\nversion="0.1.0"\n'
        )
        (self.project / "src/lib.rs").write_text(
            '#[test]\nfn provenance() { assert_eq!(env!("MITTS_GIT_SHA"), '
            'std::env::var("EXPECTED_SOURCE_SHA").unwrap()); }\n'
        )
        code = (ROOT / "build.rs").read_text()
        instrumentation = """
    let counter = std::env::var("BUILD_METADATA_COUNTER").unwrap();
    let count = std::fs::read_to_string(&counter).unwrap_or_default().parse::<u32>().unwrap_or(0);
    std::fs::write(&counter, (count + 1).to_string()).unwrap();
"""
        self.assertEqual(code.count("fn main() {"), 1)
        (self.project / "build.rs").write_text(
            code.replace("fn main() {", "fn main() {" + instrumentation)
        )
        self.env = {
            **os.environ,
            "BUILD_METADATA_COUNTER": str(self.counter),
            "CARGO_TARGET_DIR": str(self.root / "target"),
        }
        subprocess.run(
            ["cargo", "generate-lockfile", "--quiet"],
            cwd=self.project,
            env=self.env,
            check=True,
        )

    def git(self, *args):
        return subprocess.check_output(
            [
                "git",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.com",
                "-c",
                "commit.gpgsign=false",
                *args,
            ],
            cwd=self.project,
            text=True,
        ).strip()

    def cargo(self, expected):
        result = subprocess.run(
            ["cargo", "test", "--quiet", "--lib"],
            cwd=self.project,
            env={**self.env, "EXPECTED_SOURCE_SHA": expected},
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_git_checkout_caches_metadata_and_detects_new_commit(self):
        self.git("init", "-q")
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")
        sha = self.git("rev-parse", "HEAD")
        self.cargo(sha)
        self.cargo(sha)
        self.assertEqual(self.counter.read_text(), "1")
        self.git("commit", "--allow-empty", "-qm", "next")
        self.cargo(self.git("rev-parse", "HEAD"))
        self.assertEqual(self.counter.read_text(), "2")

    def test_packaged_source_caches_metadata_and_detects_vcs_change(self):
        path = self.project / ".cargo_vcs_info.json"
        path.write_text(json.dumps({"git": {"sha1": "1" * 40}}))
        self.cargo("1" * 40)
        self.cargo("1" * 40)
        self.assertEqual(self.counter.read_text(), "1")
        path.write_text(json.dumps({"git": {"sha1": "2" * 40}}))
        self.cargo("2" * 40)
        self.assertEqual(self.counter.read_text(), "2")

    def test_worktree_tracks_its_actual_git_metadata(self):
        self.git("init", "-q")
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")
        worktree = self.root / "worktree"
        self.git("worktree", "add", "--quiet", "--detach", str(worktree), "HEAD")
        self.project = worktree
        sha = self.git("rev-parse", "HEAD")
        self.cargo(sha)
        self.cargo(sha)
        self.assertEqual(self.counter.read_text(), "1")
        self.git("commit", "--allow-empty", "-qm", "next")
        self.cargo(self.git("rev-parse", "HEAD"))
        self.assertEqual(self.counter.read_text(), "2")

    def test_unknown_provenance_remains_conservatively_uncached(self):
        self.cargo("unknown")
        self.cargo("unknown")
        self.assertEqual(self.counter.read_text(), "2")
        (self.project / ".cargo_vcs_info.json").write_text("invalid metadata")
        self.cargo("unknown")
        self.cargo("unknown")
        self.assertEqual(self.counter.read_text(), "4")

    def test_dirty_tracked_non_rust_input_updates_the_suffix(self):
        (self.project / "README.md").write_text("original\n")
        self.git("init", "-q")
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")
        sha = self.git("rev-parse", "HEAD")
        self.cargo(sha)
        (self.project / "README.md").write_text("modified\n")
        self.cargo(sha + "-dirty")
        self.git("checkout", "--", "README.md")
        self.cargo(sha)
