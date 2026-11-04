# Mitts Build — tests for the setup and save GitHub Actions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SETUP = ROOT / "action.yml"
SAVE = ROOT / "save" / "action.yml"


def run_blocks(text):
    """Yield the lines of every `run: |` block, to guard against expression injection."""
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        match = re.match(r"^(\s*)run:\s*\|\s*$", lines[index])
        index += 1
        if not match:
            continue
        base = len(match.group(1))
        while index < len(lines):
            line = lines[index]
            if line.strip() and len(line) - len(line.lstrip()) <= base:
                break
            yield line
            index += 1


class ActionMetadataTests(unittest.TestCase):
    def test_both_actions_exist_and_are_composite(self):
        for path in (SETUP, SAVE):
            text = path.read_text()
            self.assertIn("using: composite", text, path)
            self.assertIn("SPDX-License-Identifier: MIT OR Apache-2.0", text, path)

    def test_every_external_action_is_pinned_to_a_commit(self):
        for path in (SETUP, SAVE):
            for use in re.findall(r"^\s*(?:-\s*)?uses:\s*(\S+)", path.read_text(), re.M):
                self.assertRegex(use, r"^[\w./-]+@[0-9a-f]{40}$", f"{path.name}: {use}")

    def test_run_scripts_never_interpolate_expressions(self):
        for path in (SETUP, SAVE):
            for line in run_blocks(path.read_text()):
                self.assertNotIn("${{", line, f"{path.name}: pass values through env: {line.strip()}")

    def test_setup_installs_with_the_installer_shipped_next_to_it(self):
        text = SETUP.read_text()
        self.assertIn('"$GITHUB_ACTION_PATH/site/install.sh"', text)
        self.assertTrue((ROOT / "site" / "install.sh").is_file())

    def test_cache_root_precedence_matches_mitts(self):
        text = SETUP.read_text()
        self.assertIn("${INPUT_CACHE_DIR:-${MITTS_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/mitts}}", text)
        self.assertIn("MITTS_CACHE_DIR=%s", text)

    def test_save_only_reads_what_setup_exports(self):
        setup = SETUP.read_text()
        save = SAVE.read_text()
        exported = set(re.findall(r"printf '([A-Z_]+)=%s", setup))
        used = set(re.findall(r"\b((?:SETUP_MITTS|MITTS)_[A-Z_]+)\b", save))
        self.assertTrue(used, "save action must reference the setup state")
        self.assertLessEqual(used, exported, f"save uses {sorted(used - exported)} that setup never exports")

    def test_save_prunes_before_it_saves(self):
        text = SAVE.read_text()
        self.assertLess(text.index("mitts gc"), text.index("actions/cache/save@"))
        self.assertIn("--max-bytes", text)

    def test_cache_key_covers_platform_version_and_lockfiles(self):
        text = SETUP.read_text()
        for part in ("runner.os", "runner.arch", "steps.install.outputs.version", "hashFiles('**/Cargo.lock')"):
            self.assertIn(part, text)

    def test_documentation_describes_both_actions(self):
        docs = (ROOT / "docs" / "github-action.md").read_text()
        for needle in ("alex09x/mitts@", "alex09x/mitts/save@", "cache-key", "max-bytes"):
            self.assertIn(needle, docs)


if __name__ == "__main__":
    unittest.main()
