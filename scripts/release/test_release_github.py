# Mitts Build — release mutation and retry safety regressions, without network.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import os
import unittest
from pathlib import Path
from unittest.mock import patch

import github_release as release

TAG = "v0.4.0"
SHA = "a" * 40
MANIFEST = {"archive": {"sha256": "b" * 64, "size": 123}}
ASSET = {"name": "archive", "digest": "sha256:" + "b" * 64, "size": 123}


class GitHubReleaseTests(unittest.TestCase):
    @patch.object(release, "api")
    def test_retagging_is_never_attempted(self, api):
        api.side_effect = [None, {"object": {"type": "commit", "sha": "c" * 40}}]
        with self.assertRaises(ValueError):
            release.draft(TAG, SHA)
        self.assertTrue(all(len(call.args) == 1 for call in api.call_args_list))

    @patch.object(release, "api")
    def test_new_release_starts_as_source_locked_draft(self, api):
        api.side_effect = [None, None, {"sha": "d" * 40}, {}, None, {}]
        release.draft(TAG, SHA)
        payload = api.call_args_list[-1].args[1]
        self.assertTrue(payload["draft"])
        self.assertIn(SHA, payload["body"])
        self.assertEqual(payload["target_commitish"], SHA)

    def test_conflicting_existing_asset_is_rejected(self):
        with self.assertRaises(ValueError):
            release.verify_remote({"assets": [{**ASSET, "size": 0}]}, MANIFEST, True)

    @patch.object(release.subprocess, "run")
    @patch.object(release, "validate", return_value=MANIFEST)
    @patch.object(release, "release_for", return_value={"draft": True, "assets": []})
    def test_missing_asset_prevents_publish(self, source, validate, run):
        with self.assertRaises(ValueError):
            release.publish(TAG, SHA, Path("unused"))
        run.assert_not_called()

    @patch.object(release.subprocess, "run")
    @patch.object(release, "validate", return_value=MANIFEST)
    @patch.object(
        release, "release_for", return_value={"draft": True, "assets": [ASSET]}
    )
    def test_retry_does_not_overwrite_identical_assets(self, source, validate, run):
        release.upload(TAG, SHA, Path("unused"))
        run.assert_not_called()

    @patch.object(release.subprocess, "run")
    @patch.object(release, "validate", return_value=MANIFEST)
    @patch.object(
        release, "release_for", return_value={"draft": False, "assets": [ASSET]}
    )
    def test_repeat_publish_of_identical_owned_release_is_noop(
        self, source, validate, run
    ):
        release.publish(TAG, SHA, Path("unused"))
        run.assert_not_called()

    @patch.object(release, "api", return_value={"tag_name": "v0.4.0"})
    def test_old_or_equal_versions_cannot_replace_latest(self, api):
        for tag in ["v0.3.18", "v0.4.0"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.require_newer(tag)

    @patch.dict(
        os.environ,
        {"GITHUB_REF": "refs/heads/feature", "GITHUB_REPOSITORY": release.REPO},
    )
    @patch("sys.argv", ["github_release.py", "draft", TAG, SHA])
    def test_cli_does_not_allow_writes_from_feature_branch(self):
        with self.assertRaises(ValueError):
            release.main()
