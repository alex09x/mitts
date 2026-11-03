# Mitts Build — reuse successful main CI only for the exact source/controller SHA.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
import os
import re
import time
from pathlib import Path

from github_release import api


def wait_for_ci(source, controller, timeout=5400):
    if source != controller:
        return False
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("Expected an exact source commit")
    start = time.monotonic()
    while True:
        data = api(
            "actions/workflows/ci.yml/runs?event=push&branch=main&per_page=30&head_sha="
            + source,
            optional=True,
        )
        runs = [
            run
            for run in (data or {}).get("workflow_runs", [])
            if run["head_sha"] == source
            and run["event"] == "push"
            and run["head_branch"] == "main"
        ]
        if runs:
            run = max(runs, key=lambda item: item["id"])
            if run["status"] == "completed":
                if run["conclusion"] != "success":
                    raise ValueError("Exact main CI did not pass: " + run["html_url"])
                print("Reusing successful exact main CI: " + run["html_url"])
                return True
            if time.monotonic() - start >= timeout:
                raise TimeoutError("Timed out waiting for exact main CI")
        elif time.monotonic() - start >= 30:
            return False
        print(
            "Waiting for exact main CI; no duplicate native tests started", flush=True
        )
        time.sleep(10)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("sha")
    args = parser.parse_args()
    result = wait_for_ci(args.sha, os.environ.get("GITHUB_SHA"))
    value = "true" if result else "false"
    print("reused=" + value)
    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
            output.write("reused=" + value + "\n")


if __name__ == "__main__":
    main()
