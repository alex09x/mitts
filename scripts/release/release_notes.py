# Mitts Build — deterministic release bodies from human and GitHub evidence.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Read-only release preparation; the injected publisher API owns authentication."""

import html
import re
from pathlib import Path

_REPO = "https://github.com/alex09x/mitts"
_PAGE_SIZE = 100
_DISPLAY_LIMIT = 50
_MARKER = re.compile(r"<!--\s*mitts-release-source:.*?-->", re.DOTALL)


def _stable_tag(value):
    if not isinstance(value, str):
        raise TypeError("Tag must be a string")
    tag = value.strip()
    tag = tag if tag.startswith("v") else "v" + tag
    if not re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", tag):
        raise ValueError("Use a stable vX.Y.Z version")
    return tag


def _source_sha(value):
    if not isinstance(value, str):
        raise TypeError("Source SHA must be a string")
    if not re.fullmatch(r"[0-9a-fA-F]{40}", value.strip()):
        raise ValueError("Source SHA must be a full 40-character hexadecimal commit")
    return value.strip().lower()


def _clean_notes(body, *, generated=False):
    """Keep native PR categories; replace native contributor/changelog sections."""
    body = _MARKER.sub("", body)
    body = re.sub(r"\A\s*#(?!#)[ \t]+[^\n]*(?:\n|$)", "", body)
    if not generated:
        return body.strip()
    lines = []
    skip_level = None
    for line in body.splitlines():
        heading = re.match(r"^(#{1,6})\s+(.+?)\s*#*\s*$", line)
        if heading:
            level, title = len(heading[1]), heading[2].casefold()
            if skip_level is not None and level <= skip_level:
                skip_level = None
            if title in {"new contributors", "contributors", "full changelog"}:
                skip_level = level
        # GitHub emits this as a bold paragraph rather than a heading by default.
        if re.match(r"^\s*(?:\*\*)?Full Changelog(?:\*\*)?\s*:", line, re.IGNORECASE):
            continue
        if skip_level is None:
            lines.append(line)
    return "\n".join(lines).strip()


def _text(value):
    """Keep public names and subjects on one Markdown-safe line."""
    value = html.escape(" ".join(value.split()), quote=False)
    return re.sub(r"([\\`*\[\]_])", r"\\\1", value)


def _commit_sections(commits, total, full_url, *, incomplete):
    # Preserve API commit order. Login identity is case insensitive on GitHub.
    contributors = {}
    unique = {}
    for commit in commits:
        commit_sha = _source_sha(commit["sha"])
        unique.setdefault(commit_sha, commit)
        login = (commit.get("author") or {}).get("login")
        name = (commit["commit"].get("author") or {}).get("name")
        if login:
            contributors.setdefault(("login", login.casefold()), "@" + login)
        elif name:
            contributors.setdefault(("name", name.casefold()), _text(name))
    sections = []
    if contributors:
        label = "Contributors (shown commits)" if incomplete else "Contributors"
        sections.append(f"## {label}\n\n" + ", ".join(contributors.values()))
    displayed = list(unique.items())[:_DISPLAY_LIMIT]
    if displayed:
        count = (
            f"{len(displayed)} of {total}" if total is not None else str(len(displayed))
        )
        lines = ["<details>", f"<summary>Commits ({count})</summary>", ""]
        for commit_sha, commit in displayed:
            # Use the first subject only; commit message bodies are not release notes.
            message = commit["commit"]["message"].splitlines()
            subject = _text(message[0]) if message else ""
            lines.append(
                f"- [`{commit_sha[:7]}`]({_REPO}/commit/{commit_sha}) {subject}"
            )
        lines += ["", "</details>"]
        sections.append("\n".join(lines))
    if incomplete or len(unique) > len(displayed):
        sections.append(
            "Commit list is truncated; contributors cover only the fetched commits. "
            f"See the [full history/comparison]({full_url}) for all commits and authors."
        )
    return sections


def build_release_body(tag, sha, api, *, previous_tag=None, root=None):
    """Build Markdown before publishing anything; required API errors propagate.

    Fetch one explicit 100-commit page. Larger histories are labelled as truncated,
    since the publisher API returns JSON rather than pagination response headers.
    First releases use the source commit's history when no comparison base exists.
    """
    tag, sha = _stable_tag(tag), _source_sha(sha)
    if previous_tag is not None:
        previous_tag = _stable_tag(previous_tag)
    else:
        latest = api("releases/latest", optional=True)
        if latest is not None:
            previous_tag = _stable_tag(latest["tag_name"])

    request = {"tag_name": tag, "target_commitish": sha}
    if previous_tag is not None:
        request["previous_tag_name"] = previous_tag
    native = api("releases/generate-notes", request)
    generated = _clean_notes(native["body"], generated=True)

    if previous_tag is not None:
        comparison = f"{previous_tag}...{sha}"
        full_url = f"{_REPO}/compare/{comparison}"
        result = api(f"compare/{comparison}?per_page={_PAGE_SIZE}&page=1")
        commits, total = result["commits"], result["total_commits"]
        incomplete = len({commit["sha"] for commit in commits}) < total
    else:
        full_url = f"{_REPO}/commits/{sha}"
        commits = api(f"commits?sha={sha}&per_page={_PAGE_SIZE}&page=1")
        # A full page may have a successor; never claim that the history ends here.
        incomplete = len(commits) >= _PAGE_SIZE
        total = None if incomplete else len(commits)

    repository = Path(root) if root is not None else Path(__file__).resolve().parents[2]
    summary_path = repository / "docs" / "releases" / f"{tag[1:]}.md"
    summary = (
        _clean_notes(summary_path.read_text(encoding="utf-8"))
        if summary_path.is_file()
        else ""
    )
    parts = [f"# Mitts {tag}", f"Source commit: [`{sha}`]({_REPO}/commit/{sha})."]
    if summary:
        parts.append(summary)
    if generated:
        parts.append(generated)
    parts.extend(_commit_sections(commits, total, full_url, incomplete=incomplete))
    parts.append(f"**Full Changelog**: {full_url}")
    parts.append(f"<!-- mitts-release-source:{sha} -->")
    return "\n\n".join(parts) + "\n"
