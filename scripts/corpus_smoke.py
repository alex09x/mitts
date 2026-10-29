#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Corpus smoke pass: does Mitts build what Cargo builds?

For every corpus project (or a shard of it) the script clones HEAD, locks and fetches the
dependencies once, then runs the same three commands the benchmark harness runs — a
workspace `check`, a workspace `clippy` and a workspace `test --tests --no-run` — under Cargo
and under Mitts, each from an empty Mitts cache, and records both exit codes with the tail
of each stderr. A project where the two tools disagree is a defect to look at before a
benchmark series is started: Mitts failing where Cargo succeeds is a Mitts bug, Mitts
succeeding where Cargo fails is a missing diagnostic. No timing is recorded; this is not a
benchmark and may run on any build node.

    corpus_smoke.py --mitts-bin target/release/mitts --work /tmp/smoke --out smoke.json
    corpus_smoke.py ... --shard 2/3            # the second third of the corpus
    corpus_smoke.py ... --projects chrono,bat  # named projects only
"""
import argparse
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import time
from pathlib import Path

COMMANDS = {
    "check": (["cargo", "check", "--workspace", "--offline"], ["check", "--workspace"]),
    "clippy": (["cargo", "clippy", "--workspace", "--offline"], ["clippy", "--workspace"]),
    "test_no_run": (
        ["cargo", "test", "--workspace", "--tests", "--no-run", "--offline"],
        ["test", "--workspace", "--tests", "--no-run"],
    ),
}


def rmtree_force(path: Path) -> None:
    if not path.exists():
        return
    for root, dirs, files in os.walk(path):
        for name in dirs + files:
            try:
                os.chmod(os.path.join(root, name), stat.S_IRWXU)
            except OSError:
                pass
    shutil.rmtree(path, ignore_errors=True)


def run(cmd: list[str], cwd: Path, env: dict, timeout: int = 1800) -> dict:
    t0 = time.perf_counter()
    try:
        r = subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace", timeout=timeout)
        code, out = r.returncode, r.stdout
    except subprocess.TimeoutExpired as exc:
        code, out = -1, (exc.stdout or "") + "\n[timeout]"
    return {"exit_code": code, "wall_s": round(time.perf_counter() - t0, 1), "tail": "\n".join(out.strip().splitlines()[-40:])}


def count_cargo_test_binaries(proj: Path, env: dict) -> int | None:
    """Number of test executables Cargo's `test --tests --no-run` produces (warm, from its JSON messages)."""
    r = subprocess.run(
        ["cargo", "test", "--workspace", "--tests", "--no-run", "--offline", "--message-format=json"],
        cwd=proj, env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, errors="replace",
    )
    if r.returncode != 0:
        return None
    n = 0
    for line in r.stdout.splitlines():
        try:
            m = json.loads(line)
        except ValueError:
            continue
        # Only test harnesses count: a bin Cargo builds so integration tests can run it
        # (bat's `bat`, clap's `stdio-fixture`) is an executable without the test profile.
        if m.get("reason") == "compiler-artifact" and m.get("executable") and m.get("profile", {}).get("test"):
            n += 1
    return n


def count_mitts_test_binaries(tail: str) -> int | None:
    """Mitts prints `(N test binaries ready)` at the end of a successful `test --no-run`."""
    m = re.search(r"(\d+) test binar(?:y|ies) ready", tail)
    return int(m.group(1)) if m else None


def smoke_project(name: str, url: str, work: Path, mitts_bin: Path, env: dict) -> dict:
    proj = work / name
    rmtree_force(proj)
    res: dict = {"name": name, "url": url, "commands": {}}
    r = subprocess.run(["git", "clone", "-q", "--depth", "60", url, str(proj)], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if r.returncode != 0:
        res["status"] = "CLONE_FAILED"
        res["error"] = r.stdout[-2000:]
        return res
    res["commit"] = subprocess.run(["git", "rev-parse", "HEAD"], cwd=proj, stdout=subprocess.PIPE, text=True).stdout.strip()
    if (proj / ".gitmodules").exists():
        subprocess.run(["git", "submodule", "update", "--init", "--recursive", "--force"], cwd=proj, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if not (proj / "Cargo.lock").exists():
        lk = subprocess.run(["cargo", "generate-lockfile"], cwd=proj, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        if lk.returncode != 0:
            res["status"] = "LOCK_FAILED"
            res["error"] = lk.stdout[-2000:]
            return res
    fetch = subprocess.run(["cargo", "fetch"], cwd=proj, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if fetch.returncode != 0:
        res["status"] = "FETCH_FAILED"
        res["error"] = fetch.stdout[-2000:]
        return res
    cache = work / f"{name}.mitts-cache"
    for key, (cargo_cmd, mitts_args) in COMMANDS.items():
        rmtree_force(proj / "target")
        rc = run(cargo_cmd, proj, env)
        rmtree_force(proj / "target")
        rmtree_force(cache)
        rm = run([str(mitts_bin), *mitts_args, "--cache-dir", str(cache)], proj, env)
        verdict = "agree" if (rc["exit_code"] == 0) == (rm["exit_code"] == 0) else ("MITTS_FAILS" if rc["exit_code"] == 0 else "MITTS_PASSES_CARGO_FAILS")
        rec = {"cargo": rc, "mitts": rm, "verdict": verdict}
        if key == "test_no_run" and verdict == "agree" and rc["exit_code"] == 0:
            # Exit codes agree; the executables must too. clap's examples with `test = true`
            # gave Cargo 24 test binaries and Mitts 22 while both exited 0.
            rec["cargo_test_binaries"] = count_cargo_test_binaries(proj, env)
            rec["mitts_test_binaries"] = count_mitts_test_binaries(rm["tail"])
            if rec["cargo_test_binaries"] is not None and rec["mitts_test_binaries"] is not None and rec["cargo_test_binaries"] != rec["mitts_test_binaries"]:
                rec["verdict"] = verdict = "TEST_BINARY_COUNT_MISMATCH"
        res["commands"][key] = rec
        bins = f", test binaries cargo {rec.get('cargo_test_binaries')} / mitts {rec.get('mitts_test_binaries')}" if "cargo_test_binaries" in rec else ""
        print(f"  [{name}] {key}: cargo exit {rc['exit_code']} ({rc['wall_s']} s), mitts exit {rm['exit_code']} ({rm['wall_s']} s){bins} -> {verdict}", flush=True)
    rmtree_force(cache)
    rmtree_force(proj / "target")
    bad = [k for k, v in res["commands"].items() if v["verdict"] != "agree"]
    res["status"] = "OK" if not bad else "MISMATCH:" + ",".join(bad)
    return res


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--corpus", default=str(Path(__file__).resolve().parent / "corpus_100.json"))
    ap.add_argument("--mitts-bin", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--shard", default="1/1", help="i/n: run the i-th of n equal slices of the corpus")
    ap.add_argument("--projects", default="", help="comma-separated project names (overrides --shard)")
    args = ap.parse_args()
    projects = json.loads(Path(args.corpus).read_text())["projects"]
    if args.projects:
        wanted = {p.strip() for p in args.projects.split(",") if p.strip()}
        projects = [p for p in projects if p["name"] in wanted]
    else:
        i, n = (int(x) for x in args.shard.split("/"))
        projects = projects[i - 1::n]
    work = Path(args.work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, "CARGO_TERM_COLOR": "never", "CI": "true"}
    env.pop("MITTS_CACHE_DIR", None)
    mitts_bin = Path(args.mitts_bin).resolve()
    version = subprocess.run([str(mitts_bin), "--version"], stdout=subprocess.PIPE, text=True).stdout.strip()
    out_path = Path(args.out)
    report = {"mitts_version": version, "host": os.uname().nodename, "projects": {}}
    if out_path.exists():
        report["projects"] = json.loads(out_path.read_text()).get("projects", {})
    print(f"=== smoke {version} on {report['host']}: {len(projects)} projects", flush=True)
    for idx, p in enumerate(projects, 1):
        if p["name"] in report["projects"] and report["projects"][p["name"]].get("status") not in (None, "CLONE_FAILED", "FETCH_FAILED"):
            continue
        print(f"[{idx}/{len(projects)}] {p['name']}", flush=True)
        try:
            report["projects"][p["name"]] = smoke_project(p["name"], p["url"], work, mitts_bin, env)
        except Exception as exc:  # noqa: BLE001 - one project must not end the pass
            report["projects"][p["name"]] = {"name": p["name"], "status": "ERROR", "error": repr(exc)}
        out_path.write_text(json.dumps(report, indent=2))
        print(f"  -> {p['name']}: {report['projects'][p['name']]['status']}", flush=True)
    summary = {}
    for r in report["projects"].values():
        summary[r["status"].split(":")[0]] = summary.get(r["status"].split(":")[0], 0) + 1
    print("=== smoke done:", json.dumps(summary), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
