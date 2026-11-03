# Mitts Build — validate a release source without publishing anything.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
import os
import re
import subprocess
from pathlib import Path

import tomllib


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def stable_tag(value):
    tag = value if value.startswith("v") else "v" + value
    if not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag):
        raise ValueError("Use a stable vX.Y.Z version")
    return tag


def plan(value, mode):
    tag = stable_tag(value)
    if mode not in {"verify", "draft", "publish"}:
        raise ValueError("Unknown release mode")
    if mode != "verify" and os.environ.get("GITHUB_REF") != "refs/heads/main":
        raise ValueError("Release mutations are allowed only from main")
    exists = (
        subprocess.run(
            ["git", "show-ref", "--verify", "--quiet", "refs/tags/" + tag], check=False
        ).returncode
        == 0
    )
    sha = git("rev-parse", tag + "^{commit}" if exists else "HEAD")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Source must be a full commit SHA")
    subprocess.run(
        ["git", "merge-base", "--is-ancestor", sha, "refs/remotes/origin/main"],
        check=True,
    )
    manifest = tomllib.loads(git("show", sha + ":Cargo.toml"))
    lock = tomllib.loads(git("show", sha + ":Cargo.lock"))
    package = manifest["package"]
    locked = [p for p in lock["package"] if p["name"] == "mitts" and "source" not in p]
    if package["name"] != "mitts" or package["version"] != tag[1:]:
        raise ValueError("Tag must match the selected source's Cargo.toml version")
    if len(locked) != 1 or locked[0]["version"] != tag[1:]:
        raise ValueError("Cargo.lock package version does not match")
    return {"tag": tag, "sha": sha, "mode": mode}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("tag")
    parser.add_argument("mode", choices=["verify", "draft", "publish"])
    args = parser.parse_args()
    result = plan(args.tag, args.mode)
    for key, value in result.items():
        print(f"{key}={value}")
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with Path(output).open("a") as stream:
            stream.write("".join(f"{k}={v}\n" for k, v in result.items()))


if __name__ == "__main__":
    main()
