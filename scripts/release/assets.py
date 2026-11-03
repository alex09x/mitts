# Mitts Build — validate the complete native release artifact set.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
import hashlib
import json
import os
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
DARWIN_ARCHES = {
    "aarch64-apple-darwin": "arm64",
    "x86_64-apple-darwin": "x86_64",
}
SIGNING_MARKERS = (
    "Signing-Team",
    "Signed-Binary-SHA256",
    "CodeDirectory-Hash",
    "Notarization-ID",
    "Notarization-Status",
    "Notarization-Receipt",
)
TEAM_RE = re.compile(r"[0-9A-Z]{10}")
SHA256_RE = re.compile(r"[0-9a-f]{64}")
CDHASH_RE = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})")
UUID_RE = re.compile(
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
)
FORBIDDEN_LINE_SEPARATORS = ("\r", "\v", "\f", "\x85", "\u2028", "\u2029")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def validate(folder, tag, sha):
    folder = Path(folder)
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


def _strict_json_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON key in notarization receipt: " + key)
        result[key] = value
    return result


def _reject_json_constant(constant):
    raise ValueError("Invalid JSON constant in notarization receipt: " + constant)


def _parse_signing_markers(metadata, archive_name):
    if any(sep in metadata for sep in FORBIDDEN_LINE_SEPARATORS):
        raise ValueError("Malformed line separator in metadata: " + archive_name)
    by_lower = {marker.lower(): marker for marker in SIGNING_MARKERS}
    found = {}
    for line in metadata.splitlines():
        key, sep, _ = line.partition(":")
        stripped = line.strip()
        token = stripped.split(" ", 1)[0].rstrip(":").lower() if stripped else ""
        canonical = by_lower.get(key.strip().lower()) if sep else by_lower.get(token)
        if canonical is None:
            continue
        prefix = canonical + ": "
        if not line.startswith(prefix):
            raise ValueError(f"Malformed {canonical} marker in {archive_name}")
        value = line[len(prefix) :]
        if not value or value != value.strip():
            raise ValueError(f"Malformed {canonical} marker value in {archive_name}")
        if canonical in found:
            raise ValueError(f"Duplicate {canonical} marker in {archive_name}")
        found[canonical] = value
    for marker in SIGNING_MARKERS:
        if marker not in found:
            raise ValueError(f"Missing {marker} marker in {archive_name}")
    return found


def _matches_target_path(path, target):
    if not isinstance(path, str) or "\\" in path or path.startswith("/"):
        return False
    parts = path.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        return False
    return parts[-2:] == [target, "mitts"]


def _validate_darwin_archive(archive, target, expected_arch, expected_team):
    name = archive.name
    with tarfile.open(archive) as content:
        metadata = content.extractfile("BUILD.txt").read().decode()
        binary_sha256 = hashlib.sha256(content.extractfile("mitts").read()).hexdigest()
    markers = _parse_signing_markers(metadata, name)
    team = markers["Signing-Team"]
    if not TEAM_RE.fullmatch(team) or team != expected_team:
        raise ValueError("Invalid Signing-Team in " + name)
    signed_sha256 = markers["Signed-Binary-SHA256"]
    if not SHA256_RE.fullmatch(signed_sha256) or signed_sha256 != binary_sha256:
        raise ValueError("Signed-Binary-SHA256 mismatch in " + name)
    cdhash = markers["CodeDirectory-Hash"]
    if not CDHASH_RE.fullmatch(cdhash):
        raise ValueError("Invalid CodeDirectory-Hash in " + name)
    notarization_id = markers["Notarization-ID"]
    if not UUID_RE.fullmatch(notarization_id):
        raise ValueError("Invalid Notarization-ID in " + name)
    if markers["Notarization-Status"] != "Accepted":
        raise ValueError("Notarization-Status must be Accepted in " + name)
    raw_receipt = markers["Notarization-Receipt"]
    if not raw_receipt.startswith("{"):
        raise ValueError("Notarization-Receipt must be a JSON object in " + name)
    receipt = json.loads(
        raw_receipt,
        object_pairs_hook=_strict_json_object,
        parse_constant=_reject_json_constant,
    )
    if receipt.get("status") != "Accepted":
        raise ValueError("Receipt status must be Accepted in " + name)
    if receipt.get("jobId") != notarization_id:
        raise ValueError("Receipt jobId does not match Notarization-ID in " + name)
    if "issues" not in receipt:
        raise ValueError("Receipt missing issues field in " + name)
    issues = receipt["issues"]
    if issues is not None and (not isinstance(issues, list) or len(issues) != 0):
        raise ValueError("Receipt contains notarization issues in " + name)
    tickets = receipt.get("ticketContents")
    if (
        not isinstance(tickets, list)
        or not tickets
        or not all(isinstance(entry, dict) for entry in tickets)
    ):
        raise ValueError("Receipt missing valid ticketContents in " + name)
    matched = [
        entry for entry in tickets if _matches_target_path(entry.get("path"), target)
    ]
    if len(matched) != 1:
        raise ValueError(
            f"Receipt must contain one ticket for {target}/mitts in {name}"
        )
    ticket = matched[0]
    if ticket.get("arch") != expected_arch:
        raise ValueError(f"Receipt ticket arch mismatch for {target} in {name}")
    if ticket.get("cdhash") != cdhash:
        raise ValueError(f"Receipt ticket cdhash mismatch for {target} in {name}")


def validate_notarized(folder, tag, sha):
    """Validate release archives plus Darwin signing/notarization policy binding.

    Trust boundary: this check validates archive layout, source commit provenance,
    APPLE_TEAM_ID policy, archived mitts binary SHA-256 binding, and Apple notary
    log receipt consistency recorded in BUILD.txt. It is NOT independent Mach-O
    cryptographic signature verification or live Apple ticket verification; actual
    Apple codesign and notary checks run in the gated macOS signer job and in
    unprivileged native macOS verification jobs. Never trust BUILD.txt metadata
    alone as cryptographic Apple verification.
    """
    folder = Path(folder)
    sums = folder / "SHA256SUMS"
    had_sums = sums.exists()
    manifest = validate(folder, tag, sha)
    try:
        expected_team = os.environ.get("APPLE_TEAM_ID", "")
        if not TEAM_RE.fullmatch(expected_team):
            raise ValueError(
                "APPLE_TEAM_ID must be 10 uppercase alphanumeric characters"
            )
        normalized_tag = stable_tag(tag)
        for target, expected_arch in DARWIN_ARCHES.items():
            archive = folder / f"mitts-{normalized_tag}-{target}.tar.gz"
            _validate_darwin_archive(archive, target, expected_arch, expected_team)
    except Exception:
        if not had_sums:
            sums.unlink(missing_ok=True)
        raise
    return manifest


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("folder", type=Path)
    parser.add_argument("tag")
    parser.add_argument("sha")
    parser.add_argument(
        "--require-notarized",
        action="store_true",
        help="Require Darwin signing and notarization metadata binding",
    )
    args = parser.parse_args(argv)
    validator = validate_notarized if args.require_notarized else validate
    print(json.dumps(validator(args.folder, args.tag, args.sha), indent=2))


if __name__ == "__main__":
    main()
