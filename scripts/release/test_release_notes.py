# Mitts Build — mocked release body generation regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import release_notes as notes

SHA = "a1" * 20
PREVIOUS = "v0.3.19"
COMPARE = f"compare/{PREVIOUS}...{SHA}?per_page=100&page=1"
HISTORY = f"commits?sha={SHA}&per_page=100&page=1"
NATIVE = (
    "## What's Changed\n\n### Fixes\n"
    "* Restore cache tracking by @alice in https://github.com/alex09x/mitts/pull/42\n\n"
    "## New Contributors\n* @alice made their first contribution in #42\n\n"
    "**Full Changelog**: https://github.com/alex09x/mitts/compare/v0.3.19...v0.3.20"
)


def commit(number, login="alice", name="Alice", message="Direct fix\n\nDetails"):
    return {
        "sha": f"{number:040x}",
        "author": {"login": login} if login else None,
        "commit": {"author": {"name": name} if name else None, "message": message},
    }


class ReleaseNotesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.calls = []
        self.responses = {
            "releases/latest": {"tag_name": PREVIOUS},
            "releases/generate-notes": {"body": NATIVE},
            COMPARE: {"commits": [], "total_commits": 0},
            HISTORY: [],
        }

    def api(self, path, data=None, optional=False):
        self.calls.append((path, data, optional))
        result = self.responses[path]
        if isinstance(result, Exception):
            raise result
        return result

    def build(self, **kwargs):
        return notes.build_release_body(
            "v0.3.20", SHA, self.api, root=self.root, **kwargs
        )

    def summary(self, content):
        directory = self.root / "docs" / "releases"
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "0.3.20.md").write_text(content, encoding="utf-8")

    def test_prior_release_combines_summary_categories_contributors_and_commits(self):
        self.summary(
            "# Mitts 0.3.20\n\nA human-reviewed overview.\n\n- Verified change.\n"
        )
        commits = [
            commit(1, message="Merge pull request #42\nMore text"),
            commit(2, login="ALICE"),
            commit(3, login=None, name="Public Author"),
            commit(4, login=None, name="Public Author"),
            commit(5, login=None, name=None),
        ]
        self.responses[COMPARE] = {"commits": commits, "total_commits": 5}
        body = self.build()
        self.assertIn("A human-reviewed overview.", body)
        self.assertIn("### Fixes\n* Restore cache tracking", body)
        self.assertIn("/pull/42", body)
        self.assertIn("## Contributors\n\n@alice, Public Author", body)
        self.assertIn("<summary>Commits (5 of 5)</summary>", body)
        self.assertIn(f"/commit/{commits[2]['sha']}) Direct fix", body)
        self.assertNotIn("More text", body)
        self.assertNotIn("## New Contributors", body)
        self.assertEqual(body.count("**Full Changelog**:"), 1)
        self.assertIn(f"/compare/{PREVIOUS}...{SHA}", body)
        self.assertNotIn(f"{PREVIOUS}...v0.3.20", body)
        self.assertEqual(sum(line.startswith("# ") for line in body.splitlines()), 1)
        self.assertEqual(
            self.calls,
            [
                ("releases/latest", None, True),
                (
                    "releases/generate-notes",
                    {
                        "tag_name": "v0.3.20",
                        "target_commitish": SHA,
                        "previous_tag_name": PREVIOUS,
                    },
                    False,
                ),
                (COMPARE, None, False),
            ],
        )

    def test_first_release_without_previous_tag(self):
        self.responses["releases/latest"] = None
        self.responses[HISTORY] = [commit(1)]
        body = self.build()
        self.assertEqual(
            self.calls[1][1], {"tag_name": "v0.3.20", "target_commitish": SHA}
        )
        self.assertIn(f"**Full Changelog**: {notes._REPO}/commits/{SHA}", body)
        self.assertIn("## Contributors\n\n@alice", body)
        self.assertNotIn("truncated", body)
        self.assertNotIn(COMPARE, [call[0] for call in self.calls])

    def test_explicit_previous_override_and_normalization(self):
        body = notes.build_release_body(
            " 0.3.20 ", SHA.upper(), self.api, previous_tag="0.3.19", root=self.root
        )
        self.assertEqual(self.calls[0][0], "releases/generate-notes")
        self.assertEqual(self.calls[0][1]["previous_tag_name"], PREVIOUS)
        self.assertEqual(self.calls[0][1]["target_commitish"], SHA)
        self.assertIn(f"<!-- mitts-release-source:{SHA} -->", body)

    def test_absent_summary_keeps_native_notes_without_inventing_features(self):
        body = self.build()
        self.assertIn("## What's Changed", body)
        self.assertNotIn("## Summary", body)
        self.assertNotIn("## Contributors", body)
        self.assertNotIn("<details>", body)
        self.assertNotIn("Native archives", body)

    def test_invalid_tag_sha_and_previous_fail_before_api(self):
        cases = [
            ("v0.3.20-rc.1", SHA, None),
            ("01.3.20", SHA, None),
            ("v0.3.20/../../file", SHA, None),
            (None, SHA, None),
            ("0.3.20", "a" * 39, None),
            ("0.3.20", "z" * 40, None),
            ("0.3.20", None, None),
            ("0.3.20", SHA, "0.3.19-beta"),
        ]
        for tag, sha, previous in cases:
            with self.subTest(tag=tag, sha=sha, previous=previous):
                api = Mock()
                with self.assertRaises((TypeError, ValueError)):
                    notes.build_release_body(
                        tag, sha, api, previous_tag=previous, root=self.root
                    )
                api.assert_not_called()

    def test_required_api_errors_propagate_without_mutating_release_or_tag(self):
        for path in ("releases/latest", "releases/generate-notes", COMPARE, HISTORY):
            with self.subTest(path=path):
                self.calls.clear()
                original = self.responses.copy()
                failure = RuntimeError("API unavailable")
                self.responses[path] = failure
                if path == HISTORY:
                    self.responses["releases/latest"] = None
                with self.assertRaises(RuntimeError) as caught:
                    self.build()
                self.assertIs(caught.exception, failure)
                self.assertEqual(self.calls[-1][0], path)
                self.assertFalse(
                    any(
                        call[0] in {"git/tags", "git/refs", "releases"}
                        for call in self.calls
                    )
                )
                self.responses = original

    def test_large_comparison_labels_incomplete_contributors_and_links_all_commits(
        self,
    ):
        commits = [commit(i) for i in range(1, 101)]
        self.responses[COMPARE] = {"commits": commits, "total_commits": 135}
        body = self.build()
        self.assertIn("Contributors (shown commits)", body)
        self.assertIn("Commits (50 of 135)", body)
        self.assertIn("Commit list is truncated", body)
        self.assertIn(
            f"[full history/comparison]({notes._REPO}/compare/{PREVIOUS}...{SHA})", body
        )
        self.assertEqual(body.count("- [`"), 50)
        self.assertEqual(len(self.calls), 3)

    def test_full_first_page_of_initial_history_does_not_claim_completeness(self):
        self.responses["releases/latest"] = None
        self.responses[HISTORY] = [commit(i) for i in range(100)]
        body = self.build()
        self.assertIn("Commit list is truncated", body)
        self.assertIn("contributors cover only the fetched commits", body)

    def test_display_limit_is_honest_even_when_all_authors_were_fetched(self):
        self.responses[COMPARE] = {
            "commits": [commit(i) for i in range(51)],
            "total_commits": 51,
        }
        body = self.build()
        self.assertIn("## Contributors\n", body)
        self.assertIn("Commits (50 of 51)", body)
        self.assertIn("Commit list is truncated", body)

    def test_markers_and_generated_owned_sections_are_not_duplicated_on_retry(self):
        marker = f"<!-- mitts-release-source:{SHA} -->"
        stale = f"<!-- mitts-release-source:{'b' * 40} -->"
        self.summary(f"# Mitts 0.3.20\n\nOverview\n{marker}\n{stale}")
        self.responses["releases/generate-notes"]["body"] = (
            "# Mitts v0.3.20\n\n## Fixes\n* PR #42\n\n"
            "## Contributors\n@native\n\n## Full Changelog\nold comparison\n\n"
            f"## Other\n* PR #43\n{marker}\n{stale}"
        )
        body = self.build()
        self.assertEqual(body.count("<!-- mitts-release-source:"), 1)
        self.assertTrue(body.endswith(marker + "\n"))
        self.assertNotIn("@native", body)
        self.assertNotIn("old comparison", body)
        self.assertIn("## Other\n* PR #43", body)
        self.assertEqual(body, self.build())

    def test_duplicate_commits_and_markdown_content(self):
        item = commit(
            1,
            login=None,
            name="A [public] <name>",
            message="fix *markup*\nprivate body",
        )
        self.responses[COMPARE] = {"commits": [item, item], "total_commits": 1}
        body = self.build()
        self.assertEqual(body.count("- [`"), 1)
        self.assertIn(r"A \[public\] &lt;name&gt;", body)
        self.assertIn(r"fix \*markup\*", body)
        self.assertNotIn("private body", body)

    def test_default_root_is_repository_containing_module(self):
        self.summary("# Mitts 0.3.20\n\nDefault-root summary")
        fake_module = self.root / "scripts" / "release" / "release_notes.py"
        with patch.object(notes, "__file__", str(fake_module)):
            body = notes.build_release_body("v0.3.20", SHA, self.api)
        self.assertIn("Default-root summary", body)


if __name__ == "__main__":
    unittest.main()
