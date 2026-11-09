#!/usr/bin/env python3
# Mitts Build — README translation checks
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Check translated READMEs without changing files or accessing the network."""

import argparse
from collections import Counter
import re
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parent.parent
LOCALES = ("ru", "zh-CN", "es", "pt-BR", "ja", "de", "fr", "ko", "ar")
REPO = "https://github.com/alex09x/mitts/blob/main/"
FENCES = re.compile(r"^```[^\n]*\n.*?^```[ \t]*$", re.MULTILINE | re.DOTALL)


def headings(text):
    return re.findall(r"^## (.+)$", text, re.MULTILINE)


def inline_commands(text):
    prose = FENCES.sub("", text)
    return Counter(re.findall(r"`((?:mitts|cargo|rustup|curl)\s+[^`]+)`", prose))


def external_urls(text):
    return set(re.findall(r'''https?://[^\s<>"')`]+''', FENCES.sub("", text)))


def srcset_urls(prose):
    urls = []
    for raw in re.findall(r"\bsrcset=[\"']([^\"']+)[\"']", prose):
        for candidate in raw.split(","):
            tokens = candidate.strip().split()
            if tokens:
                urls.append(tokens[0])
    return urls


def image_filenames(text):
    prose = FENCES.sub("", text)
    raw = re.findall(r"!\[[^\]]*\]\(([^\s)]+)\)", prose)
    raw += re.findall(r"\bsrc=[\"']([^\"']+)[\"']", prose)
    raw += srcset_urls(prose)
    names = []
    for item in raw:
        parts = urlsplit(item)
        if not parts.scheme and not parts.netloc and parts.path:
            names.append(Path(unquote(parts.path)).name)
    return names


def local_link_errors(base_dir, prose, check_anchors=True):
    errors = []
    targets = re.findall(r"\]\(([^\s)]+)\)", prose)
    targets += re.findall(r"\b(?:href|src)=[\"']([^\"']+)[\"']", prose)
    targets += srcset_urls(prose)
    anchors = set(re.findall(r"<a\s+(?:name|id)=[\"']([^\"']+)[\"']", prose))
    for target in targets:
        parts = urlsplit(target)
        if parts.scheme or parts.netloc:
            continue
        if parts.path:
            local = (base_dir / unquote(parts.path)).resolve()
            if not local.is_relative_to(ROOT) or not local.exists():
                errors.append("broken local link/image: " + target)
        elif check_anchors and parts.fragment and unquote(parts.fragment) not in anchors:
            errors.append("missing explicit navigation anchor: " + parts.fragment)
    return errors


def nested_alerts(text):
    prose = re.sub(r"`[^`\n]*`", "", FENCES.sub("", text))
    tokens = re.finditer(
        r"<(?P<close>/)?div\b[^>]*>|^\s*>\s*\[!(?P<alert>NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]",
        prose, re.IGNORECASE | re.MULTILINE,
    )
    depth = 0
    alerts = []
    for token in tokens:
        if token.group("alert"):
            if depth:
                alerts.append(token.group("alert").upper())
        elif token.group("close"):
            depth = max(0, depth - 1)
        elif not token.group(0).endswith("/>"):
            depth += 1
    return alerts


def check(path, canonical):
    text = path.read_text(encoding="utf-8")
    errors = []
    if FENCES.findall(text) != FENCES.findall(canonical):
        errors.append("fenced code, CLI output or diagrams differ from English")
    if len(headings(text)) != len(headings(canonical)):
        errors.append("top-level sections differ from English")
    if inline_commands(text) != inline_commands(canonical):
        errors.append("inline CLI commands differ from English")
    source = re.search(r"<!-- translation-source: ([0-9a-f]{40}) -->", text)
    if not source:
        errors.append("missing English source commit marker")
    if path.name == "README.ar.md" and ('dir="rtl"' not in text or 'dir="ltr"' not in text):
        errors.append("Arabic needs RTL prose and LTR code regions")
    for target in ("README.md", *(f"readme/README.{loc}.md" for loc in LOCALES)):
        if REPO + target not in text:
            errors.append("language selector missing " + target)

    expected = external_urls(canonical)
    allowed = expected | {REPO + "README.md"}
    allowed |= {REPO + f"readme/README.{loc}.md" for loc in LOCALES}
    if source:
        allowed.add(f"https://github.com/alex09x/mitts/blob/{source.group(1)}/README.md")
    actual = external_urls(text)
    errors += ["unexpected external URL: " + url for url in sorted(actual - allowed)]
    errors += ["missing English external URL: " + url for url in sorted(expected - actual)]
    errors += ["GitHub alert inside HTML div: " + kind for kind in nested_alerts(text)]

    prose = FENCES.sub("", text)
    if image_filenames(text) != image_filenames(canonical):
        errors.append("local image filenames differ from English")
    errors += local_link_errors(path.parent, prose)
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", help="specific translated files; default: all nine")
    parser.add_argument("--source-only", action="store_true", help="inspect the English input only")
    args = parser.parse_args()
    canonical = (ROOT / "README.md").read_text(encoding="utf-8")
    if not FENCES.findall(canonical) or not headings(canonical):
        parser.error("English README has no fenced examples or sections")
    source_errors = local_link_errors(ROOT, FENCES.sub("", canonical), check_anchors=False)
    for error in source_errors:
        print(f"README.md: {error}", file=sys.stderr)
    if args.source_only:
        if not source_errors:
            print(f"English source: {len(headings(canonical))} sections, {len(FENCES.findall(canonical))} fenced blocks")
        return int(bool(source_errors))

    paths = [Path(p).resolve() for p in args.paths] if args.paths else [
        ROOT / "readme" / f"README.{loc}.md" for loc in LOCALES
    ]
    failed = bool(source_errors)
    for path in paths:
        try:
            errors = check(path, canonical)
        except (OSError, UnicodeError) as error:
            errors = [str(error)]
        for error in errors:
            print(f"{path.name}: {error}", file=sys.stderr)
        failed |= bool(errors)
    if not failed:
        print(f"Verified {len(paths)} translations: verbatim examples, sections, links and alert placement")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
