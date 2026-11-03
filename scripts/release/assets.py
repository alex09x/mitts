# Mitts Build — validate the complete native release artifact set.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
import hashlib
import json
import re
import tarfile
from pathlib import Path

from plan import stable_tag

TARGETS = (
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def validate(folder, tag, sha):
    tag = stable_tag(tag)
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Expected a full source commit")
    names = set()
    lines = []
    for target in TARGETS:
        name = f"mitts-{tag}-{target}.tar.gz"
        archive = folder / name
        checksum = folder / (name + ".sha256")
        expected = digest(archive)
        if checksum.read_text().split() != [expected, name]:
            raise ValueError("Invalid checksum: " + name)
        with tarfile.open(archive) as content:
            members = content.getmembers()
            required = {"mitts", "BUILD.txt", "LICENSE-MIT", "LICENSE-APACHE"}
            if len(members) != 4 or {m.name for m in members} != required:
                raise ValueError("Invalid archive member set: " + name)
            if not all(m.isfile() for m in members):
                raise ValueError("Release archive must contain regular files only")
            metadata = content.extractfile("BUILD.txt").read().decode()
            for line in [
                f"Mitts Build {tag}",
                f"Target: {target}",
                f"Source: https://github.com/alex09x/mitts/tree/{sha}",
            ]:
                if line not in metadata.splitlines():
                    raise ValueError("Wrong source/target metadata: " + name)
        names.update([name, name + ".sha256"])
        lines.append(f"{expected}  {name}\n")
    sums = folder / "SHA256SUMS"
    assembled = "".join(sorted(lines, key=lambda line: line.split()[1]))
    if sums.exists() and sums.read_text() != assembled:
        raise ValueError("Existing SHA256SUMS does not match all four archives")
    sums.write_text(assembled)
    names.add("SHA256SUMS")
    if {p.name for p in folder.iterdir() if p.is_file()} != names:
        raise ValueError("Unexpected release artifact files")
    return {
        name: {"sha256": digest(folder / name), "size": (folder / name).stat().st_size}
        for name in sorted(names)
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("folder", type=Path)
    parser.add_argument("tag")
    parser.add_argument("sha")
    args = parser.parse_args()
    print(json.dumps(validate(args.folder, args.tag, args.sha), indent=2))


if __name__ == "__main__":
    main()
