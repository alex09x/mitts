#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Execution logic for Stages T0 (Explain), T1 (Source Store Fetch & Cross-Clone CAS),
T2 (S1 Cold Build, S2 FastBuildLock No-Op, L3b/L3c Mutation & Reuse), and
T3 (S3 Git History Replay & S4 Branch Hopping) in `benchmark_project.py`.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import os
import time
from pathlib import Path

from benchmark_common import get_disk_usage_mb, wait_for_quiet_rig
from benchmark_lib import (
    TOOLS, assert_dir_empty_or_absent, chmod_rmtree, collect_sccache_stats_and_stop, copy_repo_tree,
    git_checkout_clean, parse_explain_metrics, prepare_swarm_worktrees, purge_mitts_cas, reset_sccache,
    run_timed, snapshot_dir, start_sccache_zero_stats, stop_sccache, tool_cmd, tool_disk_paths,
)
from benchmark_pressure import (
    SystemPressureSampler, get_current_dirty_mb, get_device_io_bytes, get_df_used_mb, sync_and_wait_dirty,
)
from t5_parsers import (
    compute_stats, parse_build_output, parse_mitts_io, parse_mitts_stats, resource_stats, select_workspace_targets,
    swarm_resources, timed_resources,
)


def _mb(before: int | None, after: int | None) -> float | None:
    return round((after - before) / (1024.0 * 1024.0), 1) if (before is not None and after is not None) else None


def measure_swarm_arm(
    arm: str, subcmd: str, wts: list[tuple[int, Path, str]], wt_root: Path,
    proj_work: Path, proj_cache: Path, ctx: dict, extra_args: list[str], max_load: float,
) -> dict:
    """One swarm run of `arm`: 12 worktrees in parallel, with I/O, disk, pressure and per-worktree resources."""
    dirty_start = sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=60.0)
    dev_id, bytes_before, read_before = get_device_io_bytes([wt_root, proj_cache])
    df_before = get_df_used_mb([proj_work, proj_cache])
    sampler = SystemPressureSampler(interval_s=0.5)
    sampler.start()
    cmd = tool_cmd(arm, subcmd, ctx, extra_args)
    t0 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=len(wts)) as ex:
        runs = [f.result() for f in [ex.submit(run_timed, cmd, wt_d, ctx["envs"][arm]) for _, wt_d, _ in wts]]
    wall_s = round(time.perf_counter() - t0, 3)
    pressure = sampler.stop()
    dirty_end = get_current_dirty_mb()
    sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=60.0)
    _, bytes_after, read_after = get_device_io_bytes([wt_root, proj_cache])
    df_after = get_df_used_mb([proj_work, proj_cache])
    scc_st = collect_sccache_stats_and_stop(ctx["sccache_bin"], ctx["envs"]["cargo_sccache"]) if arm == "cargo_sccache" else None
    # Recorded for the provenance only; the next arm's gate does the real check.
    try:
        load_after = round(os.getloadavg()[0], 2)
    except OSError:
        load_after = 0.0
    df_delta = round(df_after - df_before, 1) if (df_before is not None and df_after is not None) else None
    d_mb = get_disk_usage_mb(tool_disk_paths(arm, [w[1] for w in wts], ctx))
    p_info = {**pressure, "dirty_start_mb": dirty_start, "dirty_end_mb": dirty_end, "device": dev_id}
    return {
        "runs": runs, "wall_s": wall_s, "device_mb_written": _mb(bytes_before, bytes_after), "device_mb_read": _mb(read_before, read_after),
        "df_used_delta_mb": df_delta, "disk_mb": d_mb, "sccache_stats": scc_st, "loadavg_after": load_after, "pressure": p_info,
        "resources": swarm_resources(runs, arm),
    }


def run_warmups_and_snapshot(
    subcmd: str,
    src_dir: Path,
    wt_root: Path,
    swarm_commits: list[str],
    commit_locks: dict[str, Path] | None,
    ctx: dict,
    extra_args: list[str],
    snap_cas: Path,
    snap_scc: Path,
    snap_sh: Path,
) -> dict:
    init_wts = prepare_swarm_worktrees(src_dir, wt_root, swarm_commits, commit_locks=commit_locks)
    wt01 = init_wts[0][1]

    purge_mitts_cas(ctx["mitts_cas"], keep_sources=True)
    # `fast_lock_noop` below is a second warm run of the same command: for `test --no-run`
    # the fast build lock never applies (builder: Build | Check only), so this measures the
    # plan + action-cache no-op path; the fast lock itself is measured in T2 S2.
    r_wm = run_timed(tool_cmd("mitts_cas", subcmd, ctx, extra_args), cwd=wt01, env=ctx["envs"]["mitts_cas"])
    if r_wm["exit_code"] != 0:
        raise RuntimeError(f"Mitts warmup ({subcmd}) failed on WT-01: {r_wm['stderr_tail']}")
    r_fl = run_timed(tool_cmd("mitts_cas", subcmd, ctx, extra_args), cwd=wt01, env=ctx["envs"]["mitts_cas"])
    fast_lock_noop = {"exit_code": r_fl["exit_code"], "wall_s": r_fl["wall_s"], **parse_build_output(r_fl["output"], "mitts_cas")}
    snapshot_dir(ctx["mitts_cas"], snap_cas)

    chmod_rmtree(wt01 / "target")
    reset_sccache(ctx["sccache_bin"], ctx["sccache_dir"], ctx["envs"]["cargo_sccache"])
    r_ws = run_timed(tool_cmd("cargo_sccache", subcmd, ctx, extra_args), cwd=wt01, env=ctx["envs"]["cargo_sccache"])
    stop_sccache(ctx["sccache_bin"], ctx["sccache_dir"])
    if r_ws["exit_code"] != 0:
        raise RuntimeError(f"sccache warmup ({subcmd}) failed on WT-01: {r_ws['stderr_tail']}")
    snapshot_dir(ctx["sccache_dir"], snap_scc)

    chmod_rmtree(ctx["shared_target"])
    ctx["shared_target"].mkdir(parents=True, exist_ok=True)
    r_wsh = run_timed(tool_cmd("cargo_shared", subcmd, ctx, extra_args), cwd=wt01, env=ctx["envs"]["cargo_shared"])
    if r_wsh["exit_code"] != 0:
        raise RuntimeError(f"cargo_shared warmup ({subcmd}) failed on WT-01: {r_wsh['stderr_tail']}")
    snapshot_dir(ctx["shared_target"], snap_sh)

    chmod_rmtree(wt_root)
    return fast_lock_noop


def run_stages_t0_t1_t2_t3(
    res: dict,
    stages: set[str],
    src_dir: Path,
    proj_work: Path,
    proj_cache: Path,
    ctx: dict,
    mitts_bin: Path,
    sccache_bin: Path,
    extra_args: list[str],
    repeats: int,
    max_disk_mb: float,
    max_disk_gb: float,
    max_load: float,
    head_commit: str,
    branch_hop_commit: str,
    replay_commits: list[str],
    commit_locks: dict[str, Path],
    prewarm_commits: list[str] | None = None,
    online_fetch: bool = False,
) -> bool:
    git_checkout_clean(src_dir, head_commit, commit_locks)

    # ---------------- T0: Explain & 8-Phase Planner Timings vs Cargo Metadata ----------------
    locked_flag = ["--locked"] if ctx.get("cargo_locked", True) else []
    if "t0" in stages:
        env_t0 = {**ctx["envs"]["mitts_cas"], "MITTS_TIMINGS": "1"}
        # One untimed run fills the toolchain probe cache; the timed runs and the parsed
        # metrics then describe the same warm state.
        run_timed([str(mitts_bin), "explain", "--json"], cwd=src_dir, env=env_t0)
        exp_runs = [run_timed([str(mitts_bin), "explain", "--json"], cwd=src_dir, env=env_t0) for _ in range(repeats)]
        ok_ms = [r["wall_s"] * 1000.0 for r in exp_runs if r["exit_code"] == 0]
        c_meta_cmd = ["cargo", "metadata", "--offline", "--format-version", "1"] + locked_flag
        c_meta_runs = [run_timed(c_meta_cmd, cwd=src_dir, env=ctx["envs"]["cargo_bare"]) for _ in range(repeats)]
        c_ok_ms = [r["wall_s"] * 1000.0 for r in c_meta_runs if r["exit_code"] == 0]
        m_st, c_st = compute_stats(ok_ms), compute_stats(c_ok_ms)
        res["stages"]["t0_explain"] = {
            "exit_code": exp_runs[0]["exit_code"],
            "failures": sum(1 for r in exp_runs if r["exit_code"] != 0),
            "explain_ms": m_st,
            "cargo_metadata_ms": c_st,
            "cargo_failures": sum(1 for r in c_meta_runs if r["exit_code"] != 0),
            "ratio_vs_cargo_metadata": round(c_st["median"] / m_st["median"], 2) if (c_st["median"] and m_st["median"]) else None,
            "comparison_note": "non-equivalent work: mitts explain plans build units vs cargo metadata resolves full workspace+dev graph",
            "resources": resource_stats(exp_runs),
            "cargo_metadata_resources": resource_stats(c_meta_runs),
            "stderr_tail": exp_runs[-1]["stderr_tail"],
            **parse_explain_metrics(exp_runs[-1]["output"]),
        }
        print(f"  [t0] explain {m_st['median']} ms vs cargo metadata {c_st['median']} ms (n={repeats})", flush=True)

    # ---------------- T1: Source Store Fetch/Unpack (Mitts vs Cargo) & Cross-Clone Build (All 4 Arms) ----------------
    if "t1" in stages:
        net_fetch: dict | None = None
        if online_fetch:
            # Cold network: an empty, throw-away CARGO_HOME and Mitts store per tool, so neither
            # tool can reuse the other's download (Mitts unpacks .crate files it finds in
            # CARGO_HOME) and the multi-commit cache in ctx["cargo_home"] stays intact for the
            # offline stages. n=1; the order alternates between projects.
            net_cas = proj_work / "net_mitts_cas"
            net_ch_m, net_ch_c = proj_work / "net_ch_mitts", proj_work / "net_ch_cargo"
            for d in (net_cas, net_ch_m, net_ch_c):
                chmod_rmtree(d)
                d.mkdir(parents=True, exist_ok=True)
            env_m_on = {
                **{k: v for k, v in ctx["envs"]["mitts_cas"].items() if k != "CARGO_NET_OFFLINE"},
                "CARGO_HOME": str(net_ch_m), "MITTS_CACHE_DIR": str(net_cas),
            }
            env_c_on = {**ctx["fetch_env"], "CARGO_HOME": str(net_ch_c)}
            m_cmd_on = [str(mitts_bin), "fetch", "--cache-dir", str(net_cas)]
            c_cmd_on = ["cargo", "fetch"] + locked_flag
            first = "mitts_cas" if (sum(ord(ch) for ch in proj_work.name) % 2 == 0) else "cargo_bare"
            order = [first, "cargo_bare" if first == "mitts_cas" else "mitts_cas"]
            r_on: dict[str, dict] = {}
            for tool in order:
                if tool == "mitts_cas":
                    r_on[tool] = run_timed(m_cmd_on, cwd=src_dir, env=env_m_on)
                    r_on[tool]["store_mb"] = get_disk_usage_mb([net_cas])
                else:
                    r_on[tool] = run_timed(c_cmd_on, cwd=src_dir, env=env_c_on)
                    r_on[tool]["download_mb"] = get_disk_usage_mb([net_ch_c / "registry"])
                    # `cargo fetch` only downloads; the unpack Mitts already did happens at the
                    # first build or metadata call, timed here offline in the same CARGO_HOME.
                    r_on["cargo_unpack"] = run_timed(["cargo", "metadata", "--offline", "--format-version", "1"] + locked_flag,
                                                     cwd=src_dir, env={**env_c_on, "CARGO_NET_OFFLINE": "true"})
                    r_on[tool]["store_mb"] = get_disk_usage_mb([net_ch_c / "registry"])
            for d in (net_cas, net_ch_m, net_ch_c):
                chmod_rmtree(d)
            r_cu = r_on["cargo_unpack"]
            net_fetch = {
                "order": order,
                "mitts": {"wall_s": r_on["mitts_cas"]["wall_s"], "exit_code": r_on["mitts_cas"]["exit_code"], **timed_resources(r_on["mitts_cas"]),
                          "store_mb_after": r_on["mitts_cas"]["store_mb"], "stderr_tail": r_on["mitts_cas"]["stderr_tail"],
                          "note": "download and unpack into the Mitts source store"},
                "cargo": {"wall_s": r_on["cargo_bare"]["wall_s"], "exit_code": r_on["cargo_bare"]["exit_code"], **timed_resources(r_on["cargo_bare"]),
                          "download_mb": r_on["cargo_bare"]["download_mb"], "registry_mb_after": r_on["cargo_bare"]["store_mb"],
                          "unpack_wall_s": r_cu["wall_s"], "unpack_exit_code": r_cu["exit_code"], "unpack_resources": timed_resources(r_cu),
                          "download_plus_unpack_s": round(r_on["cargo_bare"]["wall_s"] + r_cu["wall_s"], 4),
                          "stderr_tail": r_on["cargo_bare"]["stderr_tail"], "note": "`cargo fetch` downloads .crate files; the unpack is the first offline `cargo metadata`"},
                "note": "one cold download per tool over the live network: registry index, .crate files and the Mitts store start empty",
            }
            print(f"  [t1/net] cold online fetch: mitts {net_fetch['mitts']['wall_s']} s (download+unpack), cargo {net_fetch['cargo']['wall_s']} s + unpack {r_cu['wall_s']} s (order {order})", flush=True)

        purge_mitts_cas(ctx["mitts_cas"], keep_sources=False)
        assert_dir_empty_or_absent(ctx["mitts_cas"])
        r_m_cold = run_timed([str(mitts_bin), "fetch", "--cache-dir", str(ctx["mitts_cas"]), "--offline"], cwd=src_dir, env=ctx["envs"]["mitts_cas"])
        m_warm = [run_timed([str(mitts_bin), "fetch", "--cache-dir", str(ctx["mitts_cas"]), "--offline"], cwd=src_dir, env=ctx["envs"]["mitts_cas"]) for _ in range(repeats)]
        m_warm_ms = [r["wall_s"] * 1000.0 for r in m_warm if r["exit_code"] == 0]

        reg_src = ctx["cargo_home"] / "registry" / "src"
        chmod_rmtree(reg_src)
        reg_src.mkdir(parents=True, exist_ok=True)
        c_fetch_cmd = ["cargo", "fetch", "--offline"] + locked_flag
        c_unpack_cmd = ["cargo", "metadata", "--offline", "--format-version", "1"] + locked_flag
        r_c_cold_fetch = run_timed(c_fetch_cmd, cwd=src_dir, env=ctx["envs"]["cargo_bare"])
        r_c_cold_unpack = run_timed(c_unpack_cmd, cwd=src_dir, env=ctx["envs"]["cargo_bare"])
        c_warm = [run_timed(c_fetch_cmd, cwd=src_dir, env=ctx["envs"]["cargo_bare"]) for _ in range(repeats)]
        c_warm_ms = [r["wall_s"] * 1000.0 for r in c_warm if r["exit_code"] == 0]

        clone_b = proj_work / "repo_clone_b"
        cross_arms: dict = {}
        for arm in TOOLS:
            stop_sccache(sccache_bin, ctx["sccache_dir"])
            chmod_rmtree(src_dir / "target")
            chmod_rmtree(src_dir / ".mitts")
            purge_mitts_cas(ctx["mitts_cas"], keep_sources=True)
            chmod_rmtree(ctx["shared_target"])
            ctx["shared_target"].mkdir(parents=True, exist_ok=True)
            if arm == "cargo_sccache":
                reset_sccache(sccache_bin, ctx["sccache_dir"], ctx["envs"]["cargo_sccache"])
            cmd = tool_cmd(arm, "build", ctx, extra_args)
            r_ca = run_timed(cmd, cwd=src_dir, env=ctx["envs"][arm])
            copy_repo_tree(src_dir, clone_b)
            r_cb = run_timed(cmd, cwd=clone_b, env=ctx["envs"][arm])
            chmod_rmtree(clone_b)
            scc_cb = collect_sccache_stats_and_stop(sccache_bin, ctx["envs"]["cargo_sccache"]) if arm == "cargo_sccache" else None
            p_ca, p_cb = parse_build_output(r_ca["output"], arm), parse_build_output(r_cb["output"], arm)
            cross_arms[arm] = {
                "clone_a_exit_code": r_ca["exit_code"], "clone_a_s": r_ca["wall_s"] if r_ca["exit_code"] == 0 else None,
                "clone_b_exit_code": r_cb["exit_code"], "clone_b_s": r_cb["wall_s"] if r_cb["exit_code"] == 0 else None,
                "units_compiled_a": p_ca["units_compiled"], "units_total_a": p_ca["units_total"],
                "units_compiled_b": p_cb["units_compiled"], "units_total_b": p_cb["units_total"], "sccache_stats": scc_cb,
                "clone_a_resources": timed_resources(r_ca), "clone_b_resources": timed_resources(r_cb),
            }
            if arm == "mitts_cas":
                st_cb = parse_mitts_stats(r_cb["output"])
                cross_arms[arm]["mitts_stats_a"] = parse_mitts_stats(r_ca["output"])
                cross_arms[arm]["mitts_stats_b"] = st_cb
                cross_arms[arm]["mitts_io_a"] = parse_mitts_io(r_ca["output"])
                cross_arms[arm]["mitts_io_b"] = parse_mitts_io(r_cb["output"])
                cross_arms[arm]["full_cas_hit"] = r_ca["exit_code"] == 0 and r_cb["exit_code"] == 0 and st_cb["compiled"] == 0 and st_cb["cached"] > 0
        m_cc = cross_arms["mitts_cas"]
        res["stages"]["t1_source_and_cross_clone"] = {
            "cold_network_fetch": net_fetch,
            "cold_fetch_exit_code": r_m_cold["exit_code"],
            "cold_fetch_ms": round(r_m_cold["wall_s"] * 1000.0, 2) if r_m_cold["exit_code"] == 0 else None,
            "warm_fetch_ms": compute_stats(m_warm_ms),
            "cargo_cold_fetch_ms": round(r_c_cold_fetch["wall_s"] * 1000.0, 2) if r_c_cold_fetch["exit_code"] == 0 else None,
            "cargo_cold_unpack_ms": round(r_c_cold_unpack["wall_s"] * 1000.0, 2) if r_c_cold_unpack["exit_code"] == 0 else None,
            "cargo_warm_fetch_ms": compute_stats(c_warm_ms),
            "cold_fetch_resources": timed_resources(r_m_cold),
            "warm_fetch_resources": resource_stats(m_warm),
            "cargo_cold_fetch_resources": timed_resources(r_c_cold_fetch),
            "cargo_cold_unpack_resources": timed_resources(r_c_cold_unpack),
            "cargo_warm_fetch_resources": resource_stats(c_warm),
            "cross_clone_arms": cross_arms,
            "cross_clone_a_exit_code": m_cc["clone_a_exit_code"],
            "cross_clone_a_s": m_cc["clone_a_s"],
            "cross_clone_b_exit_code": m_cc["clone_b_exit_code"],
            "cross_clone_b_s": m_cc["clone_b_s"],
            "cross_clone_b_stats": m_cc["mitts_stats_b"],
            "cross_clone_full_cas_hit": m_cc["full_cas_hit"],
        }
        print(f"  [t1] fetch cold {res['stages']['t1_source_and_cross_clone']['cold_fetch_ms']} ms; cross-clone full CAS hit={m_cc['full_cas_hit']}", flush=True)

    # Ensure both `<mitts_cas>/sources` and Cargo `registry/src` are populated symmetrically before T2,
    # for every commit the later stages check out: T1 emptied `registry/src`, and an unpack inside a
    # timed window would be charged to that build.
    for c in list(dict.fromkeys([head_commit, *(prewarm_commits or [])])):
        git_checkout_clean(src_dir, c, commit_locks)
        run_timed([str(mitts_bin), "fetch", "--cache-dir", str(ctx["mitts_cas"]), "--offline"], cwd=src_dir, env=ctx["envs"]["mitts_cas"])
        run_timed(["cargo", "metadata", "--offline", "--format-version", "1"] + locked_flag, cwd=src_dir, env=ctx["envs"]["cargo_bare"])
    git_checkout_clean(src_dir, head_commit, commit_locks)

    # ---------------- T2: S1 Cold Build (Interleaved Across Arms), S2 FastBuildLock No-Op, L3b/L3c Mutation ----------------
    if "t2" in stages:
        s1_arms = ["mitts_cas", "cargo_bare", "cargo_bare_no_incr", "cargo_sccache", "cargo_shared"]
        cold_by_arm: dict[str, list[dict]] = {a: [] for a in s1_arms}
        last_scc_by_arm: dict[str, dict | None] = {a: None for a in s1_arms}
        s1_cold, s2_noop, s2_fast_lock, l3_mutation = {}, {}, {}, {}
        tgt_info = select_workspace_targets(src_dir, env=ctx["envs"]["cargo_bare"])
        rs_file = tgt_info["lib_or_bin_src"]

        for rep_idx in range(repeats):
            shift = rep_idx % len(s1_arms)
            for arm in s1_arms[shift:] + s1_arms[:shift]:
                cmd = tool_cmd("mitts_cas" if arm == "mitts_cas" else "cargo_bare", "build", ctx, extra_args)
                stop_sccache(sccache_bin, ctx["sccache_dir"])
                g_load, _, g_waited, g_busy = wait_for_quiet_rig(timeout_s=120.0, max_load=max_load)
                chmod_rmtree(src_dir / "target")
                chmod_rmtree(src_dir / ".mitts")
                assert_dir_empty_or_absent(src_dir / "target")
                purge_mitts_cas(ctx["mitts_cas"], keep_sources=True)
                chmod_rmtree(ctx["shared_target"])
                assert_dir_empty_or_absent(ctx["shared_target"])
                ctx["shared_target"].mkdir(parents=True, exist_ok=True)
                if arm == "cargo_sccache":
                    reset_sccache(sccache_bin, ctx["sccache_dir"], ctx["envs"]["cargo_sccache"])

                # Settle the filesystem first: deletes and writes still in flight made deltas negative.
                sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=60.0)
                df_before = get_df_used_mb([proj_work, proj_cache])
                _, dev_before, rd_before = get_device_io_bytes([src_dir, proj_cache])
                r_c = run_timed(cmd, cwd=src_dir, env=ctx["envs"][arm])
                sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=60.0)
                _, dev_after, rd_after = get_device_io_bytes([src_dir, proj_cache])
                df_after = get_df_used_mb([proj_work, proj_cache])
                if arm == "cargo_sccache":
                    last_scc_by_arm[arm] = collect_sccache_stats_and_stop(sccache_bin, ctx["envs"]["cargo_sccache"])

                d_mb = get_disk_usage_mb(tool_disk_paths(arm, [src_dir], ctx))
                if d_mb > max_disk_mb:
                    res["status"] = f"SKIPPED_DISK_LIMIT_{max_disk_gb}GB"
                    res["disk_mb_observed"] = d_mb
                    return False

                parsed_c = parse_build_output(r_c["output"], "mitts_cas" if arm == "mitts_cas" else "cargo_bare")
                print(f"  [t2] rep {rep_idx + 1}/{repeats} {arm}: cold build {r_c['wall_s']} s, exit {r_c['exit_code']}", flush=True)
                cold_rec = {
                    "wall_s": r_c["wall_s"], "exit_code": r_c["exit_code"], **timed_resources(r_c),
                    "disk_mb": d_mb, "df_used_delta_mb": round(df_after - df_before, 1) if (df_before is not None and df_after is not None) else None,
                    "device_mb_written": _mb(dev_before, dev_after), "device_mb_read": _mb(rd_before, rd_after),
                    "units_compiled": parsed_c["units_compiled"], "units_cached": parsed_c["units_cached"], "units_total": parsed_c["units_total"],
                    "loadavg_before": g_load, "cpu_busy_before": g_busy, "preflight_waited_s": g_waited, "stderr_tail": r_c["stderr_tail"],
                }
                if arm == "mitts_cas":
                    cold_rec["units_tainted"] = parsed_c["units_tainted"]
                    cold_rec["mitts_io"] = parse_mitts_io(r_c["output"])
                cold_by_arm[arm].append(cold_rec)

                if rep_idx == repeats - 1 and r_c["exit_code"] == 0:
                    if arm in TOOLS:
                        if arm == "cargo_sccache":
                            start_sccache_zero_stats(sccache_bin, ctx["envs"]["cargo_sccache"])
                        noop_runs = [run_timed(cmd, cwd=src_dir, env=ctx["envs"][arm]) for _ in range(repeats)]
                        if arm == "cargo_sccache":
                            collect_sccache_stats_and_stop(sccache_bin, ctx["envs"]["cargo_sccache"])
                        s2_noop[arm] = {
                            **compute_stats([r["wall_s"] * 1000.0 for r in noop_runs if r["exit_code"] == 0]),
                            "failures": sum(1 for r in noop_runs if r["exit_code"] != 0), "resources": resource_stats(noop_runs),
                        }
                        if arm == "mitts_cas":
                            # The fast build lock is off with an explicit --cache-dir; one run
                            # records its state, the timed runs are fast-lock hits.
                            env_fl = {**ctx["envs"][arm], "MITTS_FAST_LOCK": "1"}
                            run_timed(cmd, cwd=src_dir, env=env_fl)
                            fl_runs = [run_timed(cmd, cwd=src_dir, env=env_fl) for _ in range(repeats)]
                            s2_fast_lock[arm] = {
                                **compute_stats([r["wall_s"] * 1000.0 for r in fl_runs if r["exit_code"] == 0]),
                                "fast_lock_hits": sum(1 for r in fl_runs if parse_build_output(r["output"], arm).get("fast_lock") == 1),
                                "failures": sum(1 for r in fl_runs if r["exit_code"] != 0), "resources": resource_stats(fl_runs),
                            }

                    if rs_file and rs_file.is_file():
                        orig_txt = rs_file.read_text(encoding="utf-8", errors="replace")
                        try:
                            if arm == "cargo_sccache":
                                start_sccache_zero_stats(sccache_bin, ctx["envs"]["cargo_sccache"])
                            rs_file.write_text(orig_txt + "\n#[allow(dead_code)]\npub const __MITTS_L3_MUTATION: u32 = 7;\n", encoding="utf-8")
                            r_l3b = run_timed(cmd, cwd=src_dir, env=ctx["envs"][arm])
                            wt2 = proj_work / "wt_l3c"
                            copy_repo_tree(src_dir, wt2)
                            r_l3c = run_timed(cmd, cwd=wt2, env=ctx["envs"][arm])
                            scc_l3 = collect_sccache_stats_and_stop(sccache_bin, ctx["envs"]["cargo_sccache"]) if arm == "cargo_sccache" else None
                            chmod_rmtree(wt2)
                            tool_l3 = "mitts_cas" if arm == "mitts_cas" else "cargo_bare"
                            p_l3b, p_l3c = parse_build_output(r_l3b["output"], tool_l3), parse_build_output(r_l3c["output"], tool_l3)
                            l3_mutation[arm] = {
                                "mutated_file": str(rs_file.relative_to(src_dir)),
                                "l3b_exit_code": r_l3b["exit_code"], "l3b_in_wt_edit_s": r_l3b["wall_s"] if r_l3b["exit_code"] == 0 else None,
                                "l3c_exit_code": r_l3c["exit_code"], "l3c_cross_wt_reuse_s": r_l3c["wall_s"] if r_l3c["exit_code"] == 0 else None,
                                "l3b_units_compiled": p_l3b["units_compiled"], "l3b_units_total": p_l3b["units_total"],
                                "l3c_units_compiled": p_l3c["units_compiled"], "l3c_units_total": p_l3c["units_total"],
                                "l3b_resources": timed_resources(r_l3b), "l3c_resources": timed_resources(r_l3c),
                                "sccache_stats": scc_l3,
                            }
                            if arm == "mitts_cas":
                                l3_mutation[arm]["l3b_stats"] = parse_mitts_stats(r_l3b["output"])
                                l3_mutation[arm]["l3c_stats"] = parse_mitts_stats(r_l3c["output"])
                                l3_mutation[arm]["l3b_io"] = parse_mitts_io(r_l3b["output"])
                                l3_mutation[arm]["l3c_io"] = parse_mitts_io(r_l3c["output"])
                        finally:
                            rs_file.write_text(orig_txt, encoding="utf-8")

        for arm in s1_arms:
            cold_runs = cold_by_arm[arm]
            ok_runs = [r for r in cold_runs if r["exit_code"] == 0]
            ok_cold = [r["wall_s"] for r in ok_runs]
            last_ok = ok_runs[-1] if ok_runs else (cold_runs[-1] if cold_runs else {})

            def stat_of(key: str) -> dict:
                return compute_stats([r[key] for r in ok_runs if r.get(key) is not None])

            s1_cold[arm] = {
                "wall_stats_s": compute_stats(ok_cold), "failures": sum(1 for r in cold_runs if r["exit_code"] != 0),
                "resources": resource_stats(cold_runs),
                "max_rss_mb": max((r["max_rss_mb"] for r in cold_runs), default=0.0),
                "disk_stats_mb": stat_of("disk_mb"), "disk_mb": stat_of("disk_mb")["median"] or 0.0,
                "df_used_delta_stats_mb": stat_of("df_used_delta_mb"), "df_used_delta_mb": stat_of("df_used_delta_mb")["median"],
                "device_mb_written": stat_of("device_mb_written")["median"], "device_mb_read": stat_of("device_mb_read")["median"],
                "units_compiled": last_ok.get("units_compiled", 0), "units_cached": last_ok.get("units_cached", 0),
                "units_total": last_ok.get("units_total", 0),
                "sccache_stats": last_scc_by_arm[arm], "stderr_tail": cold_runs[-1]["stderr_tail"] if cold_runs else "",
                "runs": [{k: v for k, v in r.items() if k != "stderr_tail"} for r in cold_runs],
            }
            if arm == "mitts_cas":
                s1_cold[arm]["units_tainted"] = last_ok.get("units_tainted", 0)

        res["stages"]["t2_cold_noop_and_l3"] = {
            "s1_cold_build": s1_cold, "s2_fast_noop_ms": s2_noop, "s2_fast_lock_noop_ms": s2_fast_lock, "l3_mutation_and_reuse": l3_mutation,
        }

    # ---------------- T3: S3 Git History Replay & S4 Branch Hopping (All 4 Arms) ----------------
    if "t3" in stages:
        s3_replay, s4_hop = {}, {}
        for arm in TOOLS:
            stop_sccache(sccache_bin, ctx["sccache_dir"])
            g_load, _, g_waited, g_busy = wait_for_quiet_rig(timeout_s=120.0, max_load=max_load)
            chmod_rmtree(src_dir / "target")
            purge_mitts_cas(ctx["mitts_cas"], keep_sources=True)
            chmod_rmtree(ctx["shared_target"])
            ctx["shared_target"].mkdir(parents=True, exist_ok=True)
            if arm == "cargo_sccache":
                reset_sccache(sccache_bin, ctx["sccache_dir"], ctx["envs"]["cargo_sccache"])

            cmd = tool_cmd(arm, "build", ctx, extra_args)
            tool_t3 = "mitts_cas" if arm == "mitts_cas" else "cargo_bare"
            print(f"  [t3] {arm}: replaying {len(replay_commits)} commits, then branch hop", flush=True)
            step_records = []
            for c in replay_commits:
                git_checkout_clean(src_dir, c, commit_locks)
                r_st = run_timed(cmd, cwd=src_dir, env=ctx["envs"][arm])
                p_st = parse_build_output(r_st["output"], tool_t3)
                step = {
                    "commit": c[:12],
                    "wall_s": r_st["wall_s"],
                    "exit_code": r_st["exit_code"],
                    **timed_resources(r_st),
                    "units_compiled": p_st["units_compiled"], "units_cached": p_st["units_cached"], "units_total": p_st["units_total"],
                    "stderr_tail": r_st["stderr_tail"],
                }
                if arm == "mitts_cas":
                    step["units_tainted"] = p_st["units_tainted"]
                    step["mitts_io"] = parse_mitts_io(r_st["output"])
                step_records.append(step)
            scc_s3 = collect_sccache_stats_and_stop(sccache_bin, ctx["envs"]["cargo_sccache"]) if arm == "cargo_sccache" else None
            ok_steps = [s["wall_s"] for s in step_records if s["exit_code"] == 0]
            d_mb = get_disk_usage_mb(tool_disk_paths(arm, [src_dir], ctx))
            s3_replay[arm] = {
                "commits_count": len(replay_commits),
                "failed_steps": sum(1 for s in step_records if s["exit_code"] != 0),
                "cumulative_wall_s": round(sum(ok_steps), 3) if len(ok_steps) == len(replay_commits) else None,
                "step_stats_s": compute_stats(ok_steps),
                "peak_rss_mb": max((s["max_rss_mb"] for s in step_records), default=0.0),
                "total_user_s": round(sum(s["user_s"] for s in step_records), 3),
                "total_sys_s": round(sum(s["sys_s"] for s in step_records), 3),
                "disk_mb": d_mb,
                "sccache_stats": scc_s3,
                "loadavg_before": g_load, "cpu_busy_before": g_busy, "preflight_waited_s": g_waited,
                "steps": step_records,
            }

            git_checkout_clean(src_dir, head_commit, commit_locks)
            if arm == "cargo_sccache":
                start_sccache_zero_stats(sccache_bin, ctx["envs"]["cargo_sccache"])
            run_timed(cmd, cwd=src_dir, env=ctx["envs"][arm])

            sw_b_times, rev_a_times, hop_failures, hops = [], [], 0, []
            for _ in range(repeats):
                git_checkout_clean(src_dir, branch_hop_commit, commit_locks)
                r_sw_b = run_timed(cmd, cwd=src_dir, env=ctx["envs"][arm])
                git_checkout_clean(src_dir, head_commit, commit_locks)
                r_rev_a = run_timed(cmd, cwd=src_dir, env=ctx["envs"][arm])
                for direction, r_hop in (("to_b", r_sw_b), ("to_a", r_rev_a)):
                    p_hop = parse_build_output(r_hop["output"], tool_t3)
                    hops.append({
                        "direction": direction, "wall_s": r_hop["wall_s"], "exit_code": r_hop["exit_code"], **timed_resources(r_hop),
                        "units_compiled": p_hop["units_compiled"], "units_cached": p_hop["units_cached"], "units_total": p_hop["units_total"],
                    })
                if r_sw_b["exit_code"] == 0:
                    sw_b_times.append(r_sw_b["wall_s"])
                else:
                    hop_failures += 1
                if r_rev_a["exit_code"] == 0:
                    rev_a_times.append(r_rev_a["wall_s"])
                else:
                    hop_failures += 1
            scc_s4 = collect_sccache_stats_and_stop(sccache_bin, ctx["envs"]["cargo_sccache"]) if arm == "cargo_sccache" else None
            s4_hop[arm] = {
                "switch_to_b_stats_s": compute_stats(sw_b_times),
                "revert_to_a_stats_s": compute_stats(rev_a_times),
                "failures": hop_failures,
                "resources": resource_stats(hops),
                "sccache_stats": scc_s4,
                "hops": hops,
            }
        git_checkout_clean(src_dir, head_commit, commit_locks)
        res["stages"]["t3_history_and_hopping"] = {"s3_git_history_replay": s3_replay, "s4_branch_hopping": s4_hop}

    return True
