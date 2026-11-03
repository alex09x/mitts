# Mitts Build — verify the final Developer ID binary and Apple's accepted ticket.
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
import tempfile
import time
from pathlib import Path

from apple_keychain import expected_team, run

MAC_TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")


def signature(binary, team):
    requirement = (
        f'anchor apple generic and certificate leaf[subject.OU] = "{team}" '
        "and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
    )
    run(["codesign", "--verify", "--strict", "-R", requirement, binary])
    detail = run(["codesign", "--display", "--verbose=4", binary]).stderr
    if "Signature=adhoc" in detail or f"TeamIdentifier={team}" not in detail:
        raise ValueError("Developer ID team/signature mismatch")
    flags = re.search(r"flags=0x([0-9a-fA-F]+)", detail)
    cdhash = re.search(
        r"^CDHash=([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$", detail, re.MULTILINE
    )
    if (
        not flags
        or not int(flags[1], 16) & 0x10000
        or "Timestamp=" not in detail
        or not cdhash
    ):
        raise ValueError("Hardened runtime, trusted timestamp and CDHash are mandatory")
    return cdhash[1].lower()


def receipt_matches(receipt, submission, zip_sha, target, cdhash):
    arch = "arm64" if target.startswith("aarch64") else "x86_64"
    if (
        receipt.get("status") != "Accepted"
        or receipt.get("jobId") != submission
        or receipt.get("sha256", "").lower() != zip_sha
        or receipt.get("issues") not in (None, [])
    ):
        raise ValueError("Apple did not accept this exact submission")
    if not any(
        ticket.get("cdhash", "").lower() == cdhash
        and ticket.get("arch") == arch
        and ticket.get("path", "").endswith(target + "/mitts")
        for ticket in receipt.get("ticketContents", [])
    ):
        raise ValueError("Apple's ticket does not cover the final binary/architecture")


def extract_archive(archive, destination, tag, sha, target):
    with tarfile.open(archive) as content:
        members = content.getmembers()
        names = {"mitts", "BUILD.txt", "LICENSE-MIT", "LICENSE-APACHE"}
        if (
            len(members) != 4
            or {m.name for m in members} != names
            or not all(m.isfile() for m in members)
        ):
            raise ValueError("Unexpected release archive members")
        destination.mkdir(parents=True, exist_ok=True)
        for member in members:
            (destination / member.name).write_bytes(content.extractfile(member).read())
    metadata = (destination / "BUILD.txt").read_text()
    for line in [
        f"Mitts Build {tag}",
        f"Target: {target}",
        f"Source: https://github.com/alex09x/mitts/tree/{sha}",
    ]:
        if line not in metadata.splitlines():
            raise ValueError("Archive source/target does not match tested release")
    (destination / "mitts").chmod(0o755)
    return metadata


def verify_archive(archive, target, tag, sha):
    team = expected_team()
    with tempfile.TemporaryDirectory(prefix="mitts-signed-verify-") as directory:
        root = Path(directory)
        metadata = extract_archive(archive, root / "archive", tag, sha, target)
        fields = {}
        for line in metadata.splitlines():
            if ": " in line:
                key, value = line.split(": ", 1)
                if key.startswith(
                    ("Signing-", "Signed-", "CodeDirectory-", "Notarization-")
                ):
                    if key in fields:
                        raise ValueError("Duplicate signing proof")
                    fields[key] = value
        binary = root / "archive/mitts"
        actual = signature(binary, team)
        if (
            fields.get("Signing-Team") != team
            or fields.get("CodeDirectory-Hash") != actual
        ):
            raise ValueError("Signing proof does not describe actual code signature")
        if (
            fields.get("Signed-Binary-SHA256")
            != hashlib.sha256(binary.read_bytes()).hexdigest()
        ):
            raise ValueError("Signed binary bytes differ from proof")
        receipt = json.loads(fields["Notarization-Receipt"])
        receipt_matches(
            receipt, fields["Notarization-ID"], receipt["sha256"], target, actual
        )
        if fields.get("Notarization-Status") != "Accepted":
            raise ValueError("Missing accepted notarization")
        for attempt in range(6):
            result = run(
                [
                    "codesign",
                    "--verify",
                    "--strict",
                    "-R=notarized",
                    "--check-notarization",
                    binary,
                ],
                timeout=90,
                check=False,
            )
            if result.returncode == 0:
                break
            if attempt == 5:
                raise ValueError("Apple's online notarization assessment did not pass")
            time.sleep(10)
        version = run([binary, "--version"]).stdout.strip()
        if version != f"mitts {tag[1:]} ({sha})":
            raise ValueError("Installed signed binary has wrong version/source")
        project = root / "project"
        (project / "src").mkdir(parents=True)
        (project / "Cargo.toml").write_text(
            '[package]\nname="signed-smoke"\nversion="0.0.0"\nedition="2021"\n'
        )
        (project / "src/lib.rs").write_text(
            "pub fn answer() -> u8 { 42 }\n#[test] fn ok() { assert_eq!(answer(),42); }\n"
        )
        run(
            [
                binary,
                "test",
                "--no-run",
                "--manifest-path",
                project / "Cargo.toml",
                "--cache-dir",
                root / "cache",
            ],
            timeout=180,
        )
    print(
        "Developer ID, hardened runtime, accepted ticket and signed CLI compiler smoke passed:",
        target,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("archive", type=Path)
    parser.add_argument("target", choices=MAC_TARGETS)
    parser.add_argument("tag")
    parser.add_argument("sha")
    args = parser.parse_args()
    verify_archive(args.archive, args.target, args.tag, args.sha)


if __name__ == "__main__":
    main()
