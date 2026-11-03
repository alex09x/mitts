# Mitts Build — immutable-tag draft/upload/publish operations for GitHub Actions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
import json
import os
import re
import subprocess
from pathlib import Path

from assets import validate
from plan import stable_tag

REPO = "alex09x/mitts"


def api(path, data=None, optional=False):
    command = ["gh", "api", f"repos/{REPO}/{path}"]
    if data is not None:
        command += ["--method", "POST", "--input", "-"]
    result = subprocess.run(
        command,
        input=json.dumps(data) if data is not None else None,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        if optional and "HTTP 404" in result.stderr:
            return None
        raise RuntimeError(f"GitHub API failed for {path}: {result.stderr}")
    return json.loads(result.stdout)


def tag_commit(tag):
    ref = api("git/ref/tags/" + tag, optional=True)
    if ref is None:
        return None
    obj = ref["object"]
    for _ in range(5):
        if obj["type"] == "commit":
            return obj["sha"]
        if obj["type"] != "tag":
            break
        obj = api("git/tags/" + obj["sha"])["object"]
    raise ValueError("Invalid release tag object")


def require_newer(tag):
    latest = api("releases/latest", optional=True)
    if latest:
        current = stable_tag(latest["tag_name"])
        if tuple(map(int, tag[1:].split("."))) <= tuple(
            map(int, current[1:].split("."))
        ):
            raise ValueError("A release must be newer than the current stable release")


def find_release(tag):
    # The authenticated listing explicitly includes drafts, unlike assumptions
    # about a published-release lookup. Never create duplicate drafts on retry.
    for page in range(1, 101):
        releases = api(f"releases?per_page=100&page={page}")
        for candidate in releases:
            if candidate["tag_name"] == tag:
                return candidate
        if len(releases) < 100:
            return None
    raise ValueError("Release history exceeds the bounded lookup")


def draft(tag, sha):
    existing = tag_commit(tag)
    if existing and existing != sha:
        raise ValueError("Published tags are immutable: source commit differs")
    marker = f"<!-- mitts-release-source:{sha} -->"
    release = find_release(tag)
    if release:
        if existing != sha or marker not in (release.get("body") or ""):
            raise ValueError("Will not adopt an existing unowned release")
        if not release["draft"]:
            print(
                "Existing source-owned publication; assets must still match before retry succeeds"
            )
            return
    require_newer(tag)
    if existing is None:
        obj = api(
            "git/tags",
            {"tag": tag, "message": "Mitts " + tag, "object": sha, "type": "commit"},
        )
        api("git/refs", {"ref": "refs/tags/" + tag, "sha": obj["sha"]})
    if release:
        return
    body = (
        f"Mitts {tag}\n\nSource commit: `{sha}`.\n\n"
        "Native archives for Linux/macOS, ARM64/x86-64 include BUILD.txt and both licenses. "
        "Verify downloads using SHA256SUMS.\n\n" + marker
    )
    api(
        "releases",
        {
            "tag_name": tag,
            "target_commitish": sha,
            "name": "Mitts " + tag,
            "body": body,
            "draft": True,
            "prerelease": False,
        },
    )


def release_for(tag, sha):
    if tag_commit(tag) != sha:
        raise ValueError("Remote release tag does not match tested source")
    release = find_release(tag)
    if release is None:
        raise ValueError("Missing release draft")
    if f"<!-- mitts-release-source:{sha} -->" not in (release.get("body") or ""):
        raise ValueError("Release is not owned by this workflow/source")
    return release


def verify_remote(release, manifest, complete):
    remote = {asset["name"]: asset for asset in release["assets"]}
    if set(remote) - set(manifest):
        raise ValueError("Unexpected assets already attached to release")
    if complete and set(remote) != set(manifest):
        raise ValueError("Release is missing required artifacts")
    for name, asset in remote.items():
        expected = manifest[name]
        if (
            asset.get("digest") != "sha256:" + expected["sha256"]
            or asset["size"] != expected["size"]
        ):
            raise ValueError("Existing asset differs; refusing to overwrite: " + name)
    return remote


def upload(tag, sha, folder):
    manifest = validate(folder, tag, sha)
    release = release_for(tag, sha)
    if not release["draft"]:
        verify_remote(release, manifest, complete=True)
        print("Published assets already match; no upload attempted")
        return
    remote = verify_remote(release, manifest, complete=False)
    for name in manifest:
        if name not in remote:
            subprocess.run(
                ["gh", "release", "upload", tag, str(folder / name), "--repo", REPO],
                check=True,
            )
    verify_remote(release_for(tag, sha), manifest, complete=True)


def publish(tag, sha, folder):
    manifest = validate(folder, tag, sha)
    release = release_for(tag, sha)
    verify_remote(release, manifest, complete=True)
    if not release["draft"]:
        print("Already published with identical tested assets; no changes made")
        return
    require_newer(tag)
    subprocess.run(
        ["gh", "release", "edit", tag, "--repo", REPO, "--draft=false", "--latest"],
        check=True,
    )
    if release_for(tag, sha)["draft"]:
        raise ValueError("Publication did not complete")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=["draft", "upload", "publish"])
    parser.add_argument("tag")
    parser.add_argument("sha")
    parser.add_argument("--folder", type=Path)
    args = parser.parse_args()
    if (
        os.environ.get("GITHUB_REF") != "refs/heads/main"
        or os.environ.get("GITHUB_REPOSITORY") != REPO
    ):
        raise ValueError("Release writes require the canonical main workflow")
    tag = stable_tag(args.tag)
    if not re.fullmatch(r"[0-9a-f]{40}", args.sha):
        raise ValueError("Expected a full tested source commit")
    if args.operation == "draft":
        draft(tag, args.sha)
    elif args.folder is None:
        raise ValueError("Artifact folder required")
    elif args.operation == "upload":
        upload(tag, args.sha, args.folder)
    else:
        publish(tag, args.sha, args.folder)


if __name__ == "__main__":
    main()
