# Mitts Build — exact-source crate archive and receipt validation.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import hashlib
import io
import json
import re
import tarfile

import tomllib
from plan import stable_tag

PACKAGE = "mitts"
MAX_CRATE_BYTES = 64 << 20
MAX_METADATA_BYTES = 4 << 20
PROVENANCE_MEMBERS = frozenset(
    {"cargo.toml", "cargo.toml.orig", ".cargo_vcs_info.json", "cargo.lock"}
)


def _strict_json(raw_text, context):
    def pairs_hook(pairs):
        obj = {}
        for key, value in pairs:
            if key in obj:
                raise ValueError(f"Duplicate JSON key: {key}")
            obj[key] = value
        return obj

    def bad_const(constant):
        raise ValueError(f"Invalid JSON constant: {constant}")

    try:
        return json.loads(
            raw_text, object_pairs_hook=pairs_hook, parse_constant=bad_const
        )
    except json.JSONDecodeError as error:
        raise ValueError(f"Malformed JSON in {context}") from error


def _identity(tag, sha):
    if not isinstance(tag, str) or not isinstance(sha, str):
        raise ValueError("Tag and source commit SHA must be strings")
    normalized_tag = stable_tag(tag)
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Expected a full 40-character lowercase commit SHA")
    version = normalized_tag[1:]
    return normalized_tag, version, f"{PACKAGE}-{version}.crate"


def _validate_manifest(raw_text, version, context):
    try:
        package = tomllib.loads(raw_text).get("package")
    except tomllib.TOMLDecodeError as error:
        raise ValueError(f"Malformed TOML in {context}") from error
    if (
        not isinstance(package, dict)
        or package.get("name") != PACKAGE
        or package.get("version") != version
    ):
        raise ValueError(f"Package name/version mismatch in {context}")


def _member_text(archive, by_path, path, *, required):
    member = by_path.get(path)
    if member is None:
        if required:
            raise ValueError(f"Missing required crate member: {path}")
        return None
    if not member.isfile() or not (0 <= member.size <= MAX_METADATA_BYTES):
        raise ValueError(f"Invalid metadata member in crate archive: {path}")
    handle = archive.extractfile(member)
    if handle is None:
        raise ValueError(f"Unreadable crate member: {path}")
    with handle:
        raw = handle.read(MAX_METADATA_BYTES + 1)
    if len(raw) != member.size:
        raise ValueError(f"Truncated or oversized crate member: {path}")
    return raw.decode("utf-8")


def validate_crate_bytes(raw_bytes, tag, sha):
    normalized_tag, version, filename = _identity(tag, sha)
    if not isinstance(raw_bytes, (bytes, bytearray)) or not (
        0 < len(raw_bytes) <= MAX_CRATE_BYTES
    ):
        raise ValueError("Invalid crate archive byte size")
    archive_sha256 = hashlib.sha256(raw_bytes).hexdigest()
    root = f"{PACKAGE}-{version}"
    try:
        with tarfile.open(fileobj=io.BytesIO(raw_bytes), mode="r:gz") as archive:
            members = archive.getmembers()
            if not members:
                raise ValueError("Empty .crate archive")
            seen_paths, seen_prov, by_path = set(), set(), {}
            for member in members:
                if not (member.isfile() or member.isdir()):
                    raise ValueError(f"Unsupported tar member type: {member.name}")
                name = member.name
                if (
                    not name
                    or name[0] == "/"
                    or "\\" in name
                    or any(c in name for c in "\x00\r\n")
                ):
                    raise ValueError(f"Unsafe tar member path: {name}")
                trimmed = name[:-1] if member.isdir() and name.endswith("/") else name
                if trimmed.endswith("/") or (member.isfile() and member.size < 0):
                    raise ValueError(f"Malformed tar member: {name}")
                parts = trimmed.split("/")
                if any(p in {"", ".", ".."} for p in parts) or parts[0] != root:
                    raise ValueError(f"Unexpected tar member path: {name}")
                if member.isfile() and len(parts) < 2:
                    raise ValueError(f"Unexpected root file in crate archive: {name}")
                canonical = "/".join(parts)
                if canonical in seen_paths:
                    raise ValueError(f"Duplicate archive member: {canonical}")
                seen_paths.add(canonical)
                if len(parts) == 2 and parts[1].lower() in PROVENANCE_MEMBERS:
                    if not member.isfile() or parts[1].lower() in seen_prov:
                        raise ValueError(f"Invalid/duplicate provenance: {canonical}")
                    seen_prov.add(parts[1].lower())
                by_path[canonical] = member
            cargo_toml = _member_text(
                archive, by_path, f"{root}/Cargo.toml", required=True
            )
            cargo_orig = _member_text(
                archive, by_path, f"{root}/Cargo.toml.orig", required=False
            )
            vcs_raw = _member_text(
                archive, by_path, f"{root}/.cargo_vcs_info.json", required=True
            )
    except (tarfile.TarError, OSError, EOFError) as error:
        raise ValueError("Invalid .crate gzip tar archive") from error
    _validate_manifest(cargo_toml, version, "normalized Cargo.toml")
    if cargo_orig is not None:
        _validate_manifest(cargo_orig, version, "Cargo.toml.orig")
    vcs = _strict_json(vcs_raw, ".cargo_vcs_info.json")
    git_info = vcs.get("git") if isinstance(vcs, dict) else None
    if not isinstance(git_info, dict) or git_info.get("sha1") != sha:
        raise ValueError(
            "Crate .cargo_vcs_info.json git.sha1 does not match source SHA"
        )
    if ("dirty" in git_info and git_info["dirty"] is not False) or (
        "dirty" in vcs and vcs["dirty"] is not False
    ):
        raise ValueError("Refusing dirty crate provenance in .cargo_vcs_info.json")
    if "path_in_vcs" in vcs and vcs["path_in_vcs"] != "":
        raise ValueError("Crate must be packaged from repository root")
    return {
        "tag": normalized_tag,
        "source": sha,
        "filename": filename,
        "sha256": archive_sha256,
    }
