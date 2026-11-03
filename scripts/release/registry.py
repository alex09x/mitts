# Mitts Build — exact-source crates.io verification and fail-closed publication.
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
import subprocess
import time
import traceback
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from registry_archive import (
    MAX_CRATE_BYTES,
    MAX_METADATA_BYTES,
    PACKAGE,
    _identity,
    _strict_json,
    _validate_manifest,
    validate_crate_bytes,
)

REPO = "alex09x/mitts"
INDEX_URL = "https://index.crates.io/mi/tt/mitts"
CRATE_URL = "https://static.crates.io/crates/mitts/{filename}"
RECEIPT_NAME = "crate-receipt.json"
RECEIPT_KEYS = frozenset({"tag", "source", "filename", "sha256"})
USER_AGENT = "mitts-release/1.0 (+https://github.com/alex09x/mitts)"
SHA256_RE = re.compile(r"[0-9a-f]{64}")
FORBIDDEN_SEPARATORS = ("\v", "\f", "\x85", "\u2028", "\u2029")
HTTP_TIMEOUT, PUBLISH_TIMEOUT = 30, 300
READBACK_ATTEMPTS, READBACK_INTERVAL = 18, 5.0
MAX_INDEX_BYTES = 8 << 20


def _secret_variants(token):
    if not isinstance(token, str) or not token:
        return ()
    raw = (token, token.strip())
    items = {s for v in raw for s in (v, urllib.parse.quote(v, safe="")) if s}
    return tuple(sorted(items, key=len, reverse=True))


def _redact(text, secrets):
    for secret in secrets:
        text = text.replace(secret, "[REDACTED]")
    return text


def _raise_sanitized(error, secrets):
    trace = "".join(traceback.format_exception(error)) if secrets else ""
    if any(secret in trace for secret in secrets):
        exc_type = ValueError if isinstance(error, ValueError) else RuntimeError
        raise exc_type(_redact(str(error), secrets)) from None
    raise error


def _read_regular(path, label, max_bytes=MAX_CRATE_BYTES):
    target = Path(path)
    if target.is_symlink() or not target.is_file():
        raise ValueError(f"Missing or non-regular {label}: {target.name}")
    with target.open("rb") as stream:
        data = stream.read(max_bytes + 1)
    if len(data) > max_bytes:
        raise ValueError(f"{label} exceeds {max_bytes} bytes: {target.name}")
    return data


def _load_verified_local(folder, tag, sha, *, require_receipt):
    folder = Path(folder)
    normalized_tag, _, filename = _identity(tag, sha)
    receipt_path = folder / RECEIPT_NAME
    if receipt_path.is_symlink():
        raise ValueError("Refusing symlink crate-receipt.json")
    if require_receipt and not receipt_path.is_file():
        raise ValueError("Missing crate-receipt.json")
    verified = validate_crate_bytes(
        _read_regular(folder / filename, "crate"), normalized_tag, sha
    )
    if receipt_path.exists():
        raw = _read_regular(receipt_path, "crate receipt", MAX_METADATA_BYTES).decode(
            "utf-8"
        )
        recorded = _strict_json(raw, RECEIPT_NAME)
        if (
            not isinstance(recorded, dict)
            or set(recorded) != RECEIPT_KEYS
            or recorded != verified
        ):
            raise ValueError("crate-receipt.json does not match verified local crate")
    return verified


def prepare(folder, tag, sha):
    folder = Path(folder)
    verified = _load_verified_local(folder, tag, sha, require_receipt=False)
    (folder / RECEIPT_NAME).write_text(
        json.dumps(verified, indent=2) + "\n", encoding="utf-8"
    )
    return verified


def http_get(
    url, *, allow_not_found=False, max_bytes=MAX_CRATE_BYTES, timeout=HTTP_TIMEOUT
):
    if not isinstance(url, str) or not url.startswith("https://") or timeout <= 0:
        raise ValueError("HTTPS URL and positive timeout are required")
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT}, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if not str(getattr(resp, "url", None) or url).startswith("https://"):
                raise ValueError(f"Refusing non-HTTPS redirect from {url}")
            status = getattr(resp, "status", None) or resp.getcode()
            if status != 200:
                raise RuntimeError(f"Unexpected HTTP status {status} from {url}")
            data = resp.read(max_bytes + 1)
    except urllib.error.HTTPError as error:
        try:
            code = error.code
        finally:
            error.close()
        if allow_not_found and code == 404:
            return None
        raise RuntimeError(f"HTTP {code} fetching {url}") from None
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        raise RuntimeError(f"Network error fetching {url}: {error}") from None
    if len(data) > max_bytes:
        raise ValueError(f"Response from {url} exceeds {max_bytes} bytes")
    return data


def fetch_index_version(version, *, timeout=HTTP_TIMEOUT):
    raw = http_get(
        INDEX_URL, allow_not_found=True, max_bytes=MAX_INDEX_BYTES, timeout=timeout
    )
    if raw is None:
        return None
    text = raw.decode("utf-8")
    if any(sep in text for sep in FORBIDDEN_SEPARATORS):
        raise ValueError("Malformed line separator in crates.io sparse index")
    entries = {}
    for raw_line in text.split("\n"):
        line = raw_line.rstrip("\r")
        if not line:
            continue
        entry = _strict_json(line, "crates.io sparse index")
        if not isinstance(entry, dict) or entry.get("name") != PACKAGE:
            raise ValueError("Unexpected package entry in crates.io sparse index")
        vers, cksum, yanked = (
            entry.get("vers"),
            entry.get("cksum"),
            entry.get("yanked", False),
        )
        if not isinstance(vers, str) or not vers or vers in entries:
            raise ValueError("Missing or duplicate version in crates.io sparse index")
        if not isinstance(cksum, str) or not SHA256_RE.fullmatch(cksum):
            raise ValueError(f"Invalid checksum in crates.io sparse index for {vers}")
        if not isinstance(yanked, bool):
            raise ValueError(
                f"Invalid yanked flag in crates.io sparse index for {vers}"
            )
        entries[vers] = entry
    if not entries:
        raise ValueError("Empty crates.io sparse index response")
    return entries.get(version)


def _verify_remote(
    entry, local_receipt, *, allow_archive_missing=False, timeout=HTTP_TIMEOUT
):
    version = local_receipt["tag"][1:]
    if entry.get("yanked", False) is not False:
        raise ValueError(f"Published crate {PACKAGE} {version} is yanked")
    if entry["cksum"] != local_receipt["sha256"]:
        raise ValueError(f"Sparse index checksum mismatch for {PACKAGE} {version}")
    url = CRATE_URL.format(filename=local_receipt["filename"])
    remote = http_get(
        url,
        allow_not_found=allow_archive_missing,
        max_bytes=MAX_CRATE_BYTES,
        timeout=timeout,
    )
    if remote is None:
        return False
    remote_sha256 = hashlib.sha256(remote).hexdigest()
    if remote_sha256 != entry["cksum"] or remote_sha256 != local_receipt["sha256"]:
        raise ValueError(f"Downloaded crate checksum mismatch for {PACKAGE} {version}")
    if (
        validate_crate_bytes(remote, local_receipt["tag"], local_receipt["source"])
        != local_receipt
    ):
        raise ValueError(
            f"Downloaded crate provenance mismatch for {PACKAGE} {version}"
        )
    return True


def verify(folder, tag, sha):
    local_receipt = _load_verified_local(folder, tag, sha, require_receipt=False)
    version = local_receipt["tag"][1:]
    entry = fetch_index_version(version)
    if entry is None:
        raise ValueError(f"Crate {PACKAGE} {version} is absent from crates.io")
    _verify_remote(entry, local_receipt, allow_archive_missing=False)
    return local_receipt


def _require_clean_source(manifest, sha):
    environment = {
        key: value for key, value in os.environ.items() if key != "CARGO_REGISTRY_TOKEN"
    }

    def git(*args):
        result = subprocess.run(
            ["git", "-C", str(manifest.parent), *args],
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
            env=environment,
        )
        if result.returncode:
            raise ValueError("Cannot verify the exact clean publication checkout")
        return result.stdout.strip()

    if git("rev-parse", "HEAD") != sha or git(
        "status", "--porcelain", "--untracked-files=all"
    ):
        raise ValueError(
            "Publication checkout differs from the verified source SHA or is dirty"
        )


def _run_cargo_publish(manifest, token, secrets):
    cmd = [
        "cargo",
        "publish",
        "--locked",
        "--no-verify",
        "--manifest-path",
        str(manifest),
    ]
    if any(secret in arg for arg in cmd for secret in secrets):
        raise ValueError("Refusing secret value in cargo publish arguments")
    try:
        result = subprocess.run(
            cmd,
            env={**os.environ, "CARGO_REGISTRY_TOKEN": token},
            capture_output=True,
            text=True,
            timeout=PUBLISH_TIMEOUT,
            check=False,
        )
    except subprocess.TimeoutExpired:
        raise RuntimeError(
            "cargo publish timed out; subprocess output suppressed"
        ) from None
    except OSError as error:
        raise RuntimeError(
            _redact(f"cargo publish failed to start: {error}", secrets)
        ) from None
    if result.returncode != 0:
        out = _redact(
            (result.stderr or "") + "\n" + (result.stdout or ""), secrets
        ).strip()
        raise RuntimeError(
            f"cargo publish failed ({result.returncode}): {out[-6000:]}"
        ) from None


def publish(
    folder,
    tag,
    sha,
    manifest_path,
    *,
    attempts=READBACK_ATTEMPTS,
    interval=READBACK_INTERVAL,
):
    secrets = _secret_variants(os.environ.get("CARGO_REGISTRY_TOKEN", ""))
    try:
        normalized_tag, version, _ = _identity(tag, sha)
        if (
            os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("GITHUB_REPOSITORY") != REPO
            or os.environ.get("GITHUB_REF") != "refs/heads/main"
        ):
            raise ValueError(
                "Publication requires GITHUB_ACTIONS=true on alex09x/mitts main"
            )
        token = os.environ.get("CARGO_REGISTRY_TOKEN", "")
        if not token or not token.strip():
            raise ValueError(
                "CARGO_REGISTRY_TOKEN is required for crates.io publication"
            )
        secrets = _secret_variants(token)
        if attempts < 1:
            raise ValueError("attempts must be at least 1")
        manifest = Path(manifest_path)
        if (
            any(secret in str(manifest) for secret in secrets)
            or manifest.name != "Cargo.toml"
        ):
            raise ValueError("Invalid Cargo.toml manifest path")
        manifest_text = _read_regular(
            manifest, "Cargo.toml", MAX_METADATA_BYTES
        ).decode("utf-8")
        _validate_manifest(manifest_text, version, "Cargo.toml")
        local_receipt = _load_verified_local(
            folder, normalized_tag, sha, require_receipt=True
        )
        existing = fetch_index_version(version)
        if existing is not None:
            _verify_remote(existing, local_receipt, allow_archive_missing=False)
            print(f"{PACKAGE} {version} ({sha}) already published identically; no-op")
            return local_receipt
        _require_clean_source(manifest, sha)
        _run_cargo_publish(manifest, token, secrets)
        for attempt in range(1, attempts + 1):
            published = fetch_index_version(version)
            if published is not None and _verify_remote(
                published, local_receipt, allow_archive_missing=(attempt < attempts)
            ):
                print(f"Published and verified {PACKAGE} {version} ({sha})")
                return local_receipt
            if attempt < attempts:
                time.sleep(interval)
        raise ValueError(
            f"Published crate {PACKAGE} {version} failed bounded public readback"
        )
    except Exception as error:
        _raise_sanitized(error, secrets)


def main(argv=None):
    secrets = _secret_variants(os.environ.get("CARGO_REGISTRY_TOKEN", ""))
    try:
        parser = argparse.ArgumentParser()
        subs = parser.add_subparsers(dest="operation", required=True)
        for name in ("prepare", "verify", "publish"):
            sub = subs.add_parser(name)
            sub.add_argument("folder", type=Path)
            sub.add_argument("tag")
            sub.add_argument("sha")
            if name == "publish":
                sub.add_argument("--manifest-path", type=Path, required=True)
        args = parser.parse_args(argv)
        if args.operation == "prepare":
            print(json.dumps(prepare(args.folder, args.tag, args.sha), indent=2))
        elif args.operation == "verify":
            print(json.dumps(verify(args.folder, args.tag, args.sha), indent=2))
        else:
            publish(args.folder, args.tag, args.sha, args.manifest_path)
    except Exception as error:
        _raise_sanitized(error, secrets)


if __name__ == "__main__":
    main()
