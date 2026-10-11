# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Measure raw disk and filesystem I/O performance:
- Sequential Write / Read throughput with Direct I/O (O_DIRECT)
- Random 4 KiB Read / Write IOPS & Latency with Direct I/O (preadv / pwritev)
- Metadata throughput (file creation, stat, hardlink, reflink, deletion)
"""

import argparse
import fcntl
import json
import mmap
import os
import random
import shutil
import subprocess
import time
from pathlib import Path

FICLONE = 0x40049409


def aligned_mmap(size_bytes: int) -> mmap.mmap:
    """Allocates a page-aligned buffer using anonymous mmap (required for O_DIRECT)."""
    return mmap.mmap(-1, size_bytes, mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS)


def get_device_model(dev_path: str) -> str:
    try:
        out = subprocess.check_output(
            ["lsblk", "-sno", "MODEL", dev_path], text=True, stderr=subprocess.DEVNULL
        ).strip().splitlines()
        for line in out:
            if line.strip():
                return line.strip()
    except Exception:
        pass
    return "unknown"


def get_mount_and_device_info(target_dir: Path):
    target = target_dir.resolve()
    while not target.exists() and target.parent != target:
        target = target.parent

    # Find mount point
    out = subprocess.check_output(["df", "-P", str(target)], text=True).strip().splitlines()
    if len(out) < 2:
        return {}
    parts = out[1].split()
    dev_path = parts[0]
    mount_point = parts[5]

    # Find filesystem type and mount options
    fstype = "unknown"
    mount_opts = "unknown"
    try:
        with open("/proc/mounts", "r") as f:
            for line in f:
                mp = line.split()
                if len(mp) >= 4 and mp[1] == mount_point:
                    fstype = mp[2]
                    mount_opts = mp[3]
                    break
    except Exception:
        pass

    disk_model = get_device_model(dev_path)

    return {
        "target_path": str(target_dir),
        "mount_point": mount_point,
        "device": dev_path,
        "model": disk_model,
        "filesystem": fstype,
        "mount_options": mount_opts,
    }


def measure_sequential_io(target_dir: Path, file_size_mb: int = 1024, block_size_mb: int = 1):
    test_file = target_dir / "bench_seq_io.bin"
    bs_bytes = block_size_mb * 1024 * 1024
    blocks = file_size_mb // block_size_mb
    buf = aligned_mmap(bs_bytes)
    buf[:] = os.urandom(bs_bytes)

    # 1. Sequential Write with O_DIRECT
    flags_w = os.O_CREAT | os.O_WRONLY | os.O_DIRECT | os.O_TRUNC
    fd = os.open(str(test_file), flags_w, 0o644)
    start_w = time.perf_counter()
    for _ in range(blocks):
        os.write(fd, buf)
    os.fsync(fd)
    os.close(fd)
    elapsed_w = time.perf_counter() - start_w
    write_mb_s = round(file_size_mb / elapsed_w, 2)

    # 2. Sequential Read with O_DIRECT
    flags_r = os.O_RDONLY | os.O_DIRECT
    fd = os.open(str(test_file), flags_r)
    start_r = time.perf_counter()
    read_bytes = 0
    for _ in range(blocks):
        n = os.readv(fd, [buf])
        read_bytes += n
        if n == 0:
            break
    os.close(fd)
    elapsed_r = time.perf_counter() - start_r
    read_mb_s = round((read_bytes / (1024 * 1024)) / elapsed_r, 2)

    if test_file.exists():
        test_file.unlink()
    buf.close()

    return {
        "file_size_mb": file_size_mb,
        "block_size_mb": block_size_mb,
        "write_mb_s": write_mb_s,
        "write_time_s": round(elapsed_w, 3),
        "read_mb_s": read_mb_s,
        "read_time_s": round(elapsed_r, 3),
    }


def measure_random_4k_io(target_dir: Path, file_size_mb: int = 256, ops_count: int = 10000):
    test_file = target_dir / "bench_rand_io.bin"
    bs_bytes = 4096
    total_blocks = (file_size_mb * 1024 * 1024) // bs_bytes
    buf = aligned_mmap(bs_bytes)
    buf[:] = os.urandom(bs_bytes)

    # Prepare file
    with open(test_file, "wb") as f:
        f.seek(file_size_mb * 1024 * 1024 - 1)
        f.write(b"\0")

    # Generate random block offsets
    rng = random.Random(42)
    block_indices = [rng.randint(0, total_blocks - 1) for _ in range(ops_count)]

    # 1. Random 4K Writes with O_DIRECT
    fd = os.open(str(test_file), os.O_RDWR | os.O_DIRECT)
    latencies_w_us = []
    start_w = time.perf_counter()
    for idx in block_indices:
        offset = idx * bs_bytes
        t0 = time.perf_counter_ns()
        os.pwritev(fd, [buf], offset)
        latencies_w_us.append((time.perf_counter_ns() - t0) / 1000.0)
    os.fsync(fd)
    total_w_s = time.perf_counter() - start_w
    os.close(fd)

    # 2. Random 4K Reads with O_DIRECT
    fd = os.open(str(test_file), os.O_RDONLY | os.O_DIRECT)
    latencies_r_us = []
    start_r = time.perf_counter()
    for idx in block_indices:
        offset = idx * bs_bytes
        t0 = time.perf_counter_ns()
        os.preadv(fd, [buf], offset)
        latencies_r_us.append((time.perf_counter_ns() - t0) / 1000.0)
    total_r_s = time.perf_counter() - start_r
    os.close(fd)

    if test_file.exists():
        test_file.unlink()
    buf.close()

    def stats(lats, total_s):
        s = sorted(lats)
        n = len(s)
        return {
            "iops": round(ops_count / total_s, 1),
            "throughput_mb_s": round((ops_count * bs_bytes / (1024 * 1024)) / total_s, 2),
            "latency_p50_us": round(s[int(n * 0.50)], 1),
            "latency_p95_us": round(s[int(n * 0.95)], 1),
            "latency_p99_us": round(s[int(n * 0.99)], 1),
            "latency_avg_us": round(sum(s) / n, 1),
        }

    return {
        "ops_count": ops_count,
        "write_4k": stats(latencies_w_us, total_w_s),
        "read_4k": stats(latencies_r_us, total_r_s),
    }


def measure_metadata_ops(target_dir: Path, files_count: int = 5000):
    meta_dir = target_dir / "bench_meta_tmp"
    meta_dir.mkdir(parents=True, exist_ok=True)
    payload = b"X" * 4096

    # 1. Create small files
    start_c = time.perf_counter()
    paths = []
    for i in range(files_count):
        p = meta_dir / f"file_{i:05d}.dat"
        with open(p, "wb") as f:
            f.write(payload)
        paths.append(p)
    elapsed_c = time.perf_counter() - start_c
    create_rate = round(files_count / elapsed_c, 1)

    # 2. Stat small files
    start_s = time.perf_counter()
    for p in paths:
        _ = os.stat(p)
    elapsed_s = time.perf_counter() - start_s
    stat_rate = round(files_count / elapsed_s, 1)

    # 3. Hardlink creation
    link_dir = target_dir / "bench_meta_links"
    link_dir.mkdir(parents=True, exist_ok=True)
    start_l = time.perf_counter()
    link_paths = []
    for i, p in enumerate(paths):
        lp = link_dir / f"link_{i:05d}.dat"
        os.link(p, lp)
        link_paths.append(lp)
    elapsed_l = time.perf_counter() - start_l
    hardlink_rate = round(files_count / elapsed_l, 1)

    # 4. Reflink check
    reflink_supported = False
    reflink_clone_time_ms = None
    ref_src = target_dir / "bench_reflink_src.bin"
    ref_dst = target_dir / "bench_reflink_dst.bin"
    try:
        with open(ref_src, "wb") as f:
            f.write(b"Z" * (100 * 1024 * 1024))
        with open(ref_dst, "wb") as f:
            pass
        fd_src = os.open(str(ref_src), os.O_RDONLY)
        fd_dst = os.open(str(ref_dst), os.O_WRONLY)
        t0 = time.perf_counter()
        fcntl.ioctl(fd_dst, FICLONE, fd_src)
        t1 = time.perf_counter()
        os.close(fd_src)
        os.close(fd_dst)
        reflink_supported = True
        reflink_clone_time_ms = round((t1 - t0) * 1000.0, 2)
    except Exception:
        reflink_supported = False
    finally:
        if ref_src.exists():
            ref_src.unlink()
        if ref_dst.exists():
            ref_dst.unlink()

    # 5. Delete hardlinks and original files
    start_d = time.perf_counter()
    for lp in link_paths:
        lp.unlink()
    for p in paths:
        p.unlink()
    elapsed_d = time.perf_counter() - start_d
    delete_rate = round((len(link_paths) + len(paths)) / elapsed_d, 1)

    shutil.rmtree(meta_dir, ignore_errors=True)
    shutil.rmtree(link_dir, ignore_errors=True)

    return {
        "files_count": files_count,
        "create_files_per_sec": create_rate,
        "stat_files_per_sec": stat_rate,
        "hardlink_files_per_sec": hardlink_rate,
        "reflink_supported": reflink_supported,
        "reflink_100mb_clone_time_ms": reflink_clone_time_ms,
        "delete_files_per_sec": delete_rate,
    }


def benchmark_filesystem(target_dir: Path):
    target_dir.mkdir(parents=True, exist_ok=True)
    info = get_mount_and_device_info(target_dir)
    print(f"\n=======================================================")
    print(f"Benchmarking {target_dir}")
    print(f"  Filesystem:    {info['filesystem']} ({info['mount_options']})")
    print(f"  Device:        {info['device']} ({info['model']})")
    print(f"  Mount Point:   {info['mount_point']}")
    print(f"=======================================================")

    print("  [1/3] Running sequential I/O (1 GiB, 1 MiB block size, O_DIRECT)...")
    seq_res = measure_sequential_io(target_dir, file_size_mb=1024, block_size_mb=1)
    print(f"    Sequential Write: {seq_res['write_mb_s']:>8.2f} MB/s ({seq_res['write_time_s']:.3f} s)")
    print(f"    Sequential Read:  {seq_res['read_mb_s']:>8.2f} MB/s ({seq_res['read_time_s']:.3f} s)")

    print("  [2/3] Running random 4 KiB I/O (10,000 ops, O_DIRECT)...")
    rand_res = measure_random_4k_io(target_dir, file_size_mb=256, ops_count=10000)
    w4k = rand_res['write_4k']
    r4k = rand_res['read_4k']
    print(f"    Random 4K Write:  {w4k['iops']:>8.1f} IOPS ({w4k['throughput_mb_s']} MB/s, p50: {w4k['latency_p50_us']} us, p99: {w4k['latency_p99_us']} us)")
    print(f"    Random 4K Read:   {r4k['iops']:>8.1f} IOPS ({r4k['throughput_mb_s']} MB/s, p50: {r4k['latency_p50_us']} us, p99: {r4k['latency_p99_us']} us)")

    print("  [3/3] Running metadata ops (5,000 files, create/stat/hardlink/delete)...")
    meta_res = measure_metadata_ops(target_dir, files_count=5000)
    print(f"    File Create (4K): {meta_res['create_files_per_sec']:>8.1f} files/s")
    print(f"    File Stat:        {meta_res['stat_files_per_sec']:>8.1f} stats/s")
    print(f"    Hardlink Create:  {meta_res['hardlink_files_per_sec']:>8.1f} links/s")
    if meta_res['reflink_supported']:
        print(f"    Reflink (100MB):  Supported ({meta_res['reflink_100mb_clone_time_ms']} ms)")
    else:
        print(f"    Reflink (100MB):  Unsupported (EOPNOTSUPP)")
    print(f"    File Delete:      {meta_res['delete_files_per_sec']:>8.1f} deletes/s")

    return {
        "mount_info": info,
        "sequential_io": seq_res,
        "random_4k_io": rand_res,
        "metadata_ops": meta_res,
    }


def main():
    parser = argparse.ArgumentParser(description="Measure raw disk & filesystem performance")
    parser.add_argument("--paths", default="/home/alex09x/disk_bench_tmp,/solana_data/mitts-bench/disk_bench_tmp",
                        help="Comma-separated target paths to benchmark")
    parser.add_argument("--output", default="benchmark-results/disk_performance_ram11.json",
                        help="Output JSON file path")
    args = parser.parse_args()

    results = {}
    paths = [Path(p.strip()) for p in args.paths.split(",") if p.strip()]

    for p in paths:
        try:
            res = benchmark_filesystem(p)
            fs_name = res["mount_info"]["filesystem"]
            key = f"{fs_name}_{res['mount_info']['mount_point'].replace('/', '_')}".strip("_")
            results[key] = res
        finally:
            if p.exists():
                shutil.rmtree(p, ignore_errors=True)

    out_p = Path(args.output)
    out_p.parent.mkdir(parents=True, exist_ok=True)
    with open(out_p, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nAll disk performance benchmarks saved to {out_p}")


if __name__ == "__main__":
    main()
