#!/usr/bin/env python3
# Mitts Build — isolated benchmark cache snapshots with internal hardlink topology
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

import os
import shutil
from pathlib import Path

from benchmark_worktree import chmod_rmtree


def _isolated_copy_function():
    """Copy each source inode once, linking only copies inside this new image.

    Never link the live store to a snapshot, or a snapshot to a restored store:
    cleanup and writes in either image must remain independent. A single copier
    spans all copied subdirectories so aliases and objects retain their identity.
    """
    copied = {}

    def copy(src, dst):
        metadata = os.stat(src)
        identity = (metadata.st_dev, metadata.st_ino)
        previous = copied.get(identity) if metadata.st_nlink > 1 else None
        if previous is not None:
            os.link(previous, dst)
            return dst
        result = shutil.copy2(src, dst)
        if metadata.st_nlink > 1:
            copied[identity] = dst
        return result

    return copy


def purge_mitts_cas(mitts_cas: Path, keep_sources: bool = True) -> None:
    if not mitts_cas.exists():
        mitts_cas.mkdir(parents=True, exist_ok=True)
        return
    for child in list(mitts_cas.iterdir()):
        if keep_sources and child.name == "sources":
            continue
        chmod_rmtree(child)
    leftover = [c.name for c in mitts_cas.iterdir() if not (keep_sources and c.name == "sources")]
    if leftover:
        raise RuntimeError(f"Failed to purge mitts_cas ({mitts_cas}), leftover: {leftover}")


def snapshot_dir(src: Path, dst: Path) -> None:
    chmod_rmtree(dst)
    if src.exists():
        shutil.copytree(
            src, dst, symlinks=True, copy_function=_isolated_copy_function(),
            ignore=shutil.ignore_patterns("*.fifo", "jobserver", "tmp", "*.lock"),
        )


def restore_dir(snap: Path, dst: Path, keep_sources: bool = False) -> None:
    copy = _isolated_copy_function()
    if keep_sources and dst.exists():
        purge_mitts_cas(dst, keep_sources=True)
        if snap.exists():
            for item in snap.iterdir():
                if item.name == "sources":
                    continue
                target_item = dst / item.name
                if item.is_symlink():
                    os.symlink(os.readlink(item), target_item)
                elif item.is_dir():
                    shutil.copytree(item, target_item, symlinks=True, copy_function=copy)
                else:
                    copy(item, target_item)
        return
    chmod_rmtree(dst)
    if snap.exists():
        shutil.copytree(snap, dst, symlinks=True, copy_function=copy)
    else:
        dst.mkdir(parents=True, exist_ok=True)
