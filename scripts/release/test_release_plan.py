# Mitts Build — release source and version guard regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from plan import plan, stable_tag


class ReleasePlanTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        before = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, before)
        self.git("init", "-q", "-b", "main")
        self.write_version("0.3.18")
        self.commit()

    def git(self, *args):
        return subprocess.check_output(["git", *args], text=True).strip()

    def write_version(self, version, locked=None):
        Path("Cargo.toml").write_text(f'[package]\nname="mitts"\nversion="{version}"\n')
        Path("Cargo.lock").write_text(
            f'[[package]]\nname="mitts"\nversion="{locked or version}"\n'
        )

    def commit(self):
        self.git("add", ".")
        self.git(
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-qm",
            "version",
        )
        sha = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/remotes/origin/main", sha)
        return sha

    def test_existing_tag_keeps_its_exact_commit(self):
        sha = self.git("rev-parse", "HEAD")
        self.git("tag", "v0.3.18")
        self.write_version("0.4.0")
        self.commit()
        with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/main"}):
            self.assertEqual(plan("0.3.18", "draft")["sha"], sha)

    def test_verify_checks_current_candidate_even_if_version_is_already_tagged(self):
        self.git("tag", "v0.3.18")
        Path("candidate.txt").write_text("unreleased candidate")
        sha = self.commit()
        self.assertEqual(plan("0.3.18", "verify")["sha"], sha)

    def test_future_verify_does_not_create_any_tag(self):
        self.assertEqual(plan("0.3.18", "verify")["tag"], "v0.3.18")
        self.assertEqual(self.git("tag", "--list"), "")

    def test_version_mismatch_is_rejected(self):
        with self.assertRaises(ValueError):
            plan("0.4.0", "verify")

    def test_lock_mismatch_is_rejected(self):
        self.write_version("0.3.18", "0.3.17")
        self.commit()
        with self.assertRaises(ValueError):
            plan("0.3.18", "verify")

    def test_release_writes_require_main(self):
        with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/feature"}):
            with self.assertRaises(ValueError):
                plan("0.3.18", "publish")

    def test_orphan_source_is_not_a_reviewed_main_commit(self):
        self.git("checkout", "--orphan", "unreviewed")
        self.write_version("0.4.0")
        self.git("add", ".")
        self.git(
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-qm",
            "orphan",
        )
        self.git("tag", "v0.4.0")
        with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/main"}):
            with self.assertRaises(subprocess.CalledProcessError):
                plan("0.4.0", "draft")

    def test_unsafe_or_prerelease_tags_are_rejected(self):
        for tag in ["../../main", "1.2.3;false", "v1.2.3-rc1", "v01.2.3", ""]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                stable_tag(tag)
