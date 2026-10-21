#!/usr/bin/env python3
# Mitts Build — benchmark snapshot isolation and hardlink topology regressions
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

import os
import tempfile
import unittest
from pathlib import Path

from benchmark_lib import restore_dir, snapshot_dir


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.live = self.root / "live"
        self.snapshot = self.root / "snapshot"
        self.restored = self.root / "restored"
        obj = self.live / "cas/objects/hash"
        obj.parent.mkdir(parents=True)
        obj.write_bytes(b"immutable artifact")
        alias = self.live / "cas/artifacts/libcrate.rlib"
        alias.parent.mkdir()
        os.link(obj, alias)
        (self.live / "sources").mkdir()
        (self.live / "sources/package").write_bytes(b"source")

    def assert_topology(self, root):
        obj = root / "cas/objects/hash"
        alias = root / "cas/artifacts/libcrate.rlib"
        self.assertTrue(os.path.samefile(obj, alias))
        self.assertEqual(obj.read_bytes(), b"immutable artifact")

    def test_snapshot_preserves_internal_links_without_linking_live_store(self):
        snapshot_dir(self.live, self.snapshot)
        self.assert_topology(self.snapshot)
        self.assertFalse(os.path.samefile(
            self.live / "cas/objects/hash", self.snapshot / "cas/objects/hash"))
        (self.live / "cas/objects/hash").write_bytes(b"changed live store")
        self.assertEqual((self.snapshot / "cas/objects/hash").read_bytes(), b"immutable artifact")

    def test_restore_preserves_links_and_does_not_mutate_snapshot(self):
        snapshot_dir(self.live, self.snapshot)
        restore_dir(self.snapshot, self.restored)
        self.assert_topology(self.restored)
        self.assertFalse(os.path.samefile(
            self.snapshot / "cas/objects/hash", self.restored / "cas/objects/hash"))
        (self.restored / "cas/objects/hash").write_bytes(b"changed restored store")
        self.assertEqual((self.snapshot / "cas/objects/hash").read_bytes(), b"immutable artifact")

    def test_keep_sources_preserves_topology_filters_and_symlinks(self):
        (self.live / "cas/tmp").mkdir()
        (self.live / "cas/tmp/stale").write_bytes(b"temporary")
        (self.live / "jobserver").mkdir()
        (self.live / "jobserver/pool.cap").write_text("24")
        (self.live / "cas/stale.lock").write_text("")
        os.symlink("objects/hash", self.live / "cas/object-alias")
        snapshot_dir(self.live, self.snapshot)
        (self.restored / "sources").mkdir(parents=True)
        (self.restored / "sources/retained").write_bytes(b"retained")
        (self.restored / "stale").write_bytes(b"remove")
        restore_dir(self.snapshot, self.restored, keep_sources=True)
        self.assert_topology(self.restored)
        self.assertEqual((self.restored / "sources/retained").read_bytes(), b"retained")
        self.assertFalse((self.restored / "sources/package").exists())
        self.assertFalse((self.restored / "stale").exists())
        for path in ["cas/tmp", "jobserver", "cas/stale.lock"]:
            self.assertFalse((self.restored / path).exists())
        self.assertTrue((self.restored / "cas/object-alias").is_symlink())
        self.assertEqual(os.readlink(self.restored / "cas/object-alias"), "objects/hash")


if __name__ == "__main__":
    unittest.main()
