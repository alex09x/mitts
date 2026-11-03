# Mitts Build — exact-commit CI proof reuse regressions, without network.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import unittest
from unittest.mock import patch

import reuse_ci

SHA = "a" * 40
RUN = {
    "id": 1,
    "head_sha": SHA,
    "head_branch": "main",
    "event": "push",
    "status": "completed",
    "conclusion": "success",
    "html_url": "https://github.com/example/run/1",
}


class ReuseCITests(unittest.TestCase):
    @patch.object(reuse_ci, "api")
    def test_different_controller_never_reuses_old_policy(self, api):
        self.assertFalse(reuse_ci.wait_for_ci(SHA, "b" * 40))
        api.assert_not_called()

    @patch.object(reuse_ci, "api", return_value={"workflow_runs": [RUN]})
    def test_exact_successful_main_run_is_reused(self, api):
        self.assertTrue(reuse_ci.wait_for_ci(SHA, SHA))

    @patch.object(
        reuse_ci,
        "api",
        return_value={"workflow_runs": [{**RUN, "conclusion": "failure"}]},
    )
    def test_failed_main_ci_is_not_replaced_by_fresh_optimistic_ci(self, api):
        with self.assertRaises(ValueError):
            reuse_ci.wait_for_ci(SHA, SHA)

    @patch.object(reuse_ci.time, "sleep")
    @patch.object(reuse_ci.time, "monotonic", side_effect=[0, 31])
    @patch.object(
        reuse_ci, "api", return_value={"workflow_runs": [{**RUN, "head_sha": "b" * 40}]}
    )
    def test_other_commit_is_not_accepted(self, api, clock, sleep):
        self.assertFalse(reuse_ci.wait_for_ci(SHA, SHA))

    @patch.object(reuse_ci.time, "sleep")
    @patch.object(reuse_ci, "api")
    def test_running_main_ci_is_waited_for_instead_of_duplicated(self, api, sleep):
        api.side_effect = [
            {"workflow_runs": [{**RUN, "status": "in_progress", "conclusion": None}]},
            {"workflow_runs": [RUN]},
        ]
        self.assertTrue(reuse_ci.wait_for_ci(SHA, SHA))
        sleep.assert_called_once_with(10)
