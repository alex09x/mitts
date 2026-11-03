# Mitts Build — Developer ID and fail-closed notarization before release packaging.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
import hashlib
import json
import tarfile
import tempfile
import zipfile
from pathlib import Path

from apple_keychain import credentials, expected_team, run
from apple_verify import MAC_TARGETS, extract_archive, receipt_matches, signature
from assets import validate
from plan import stable_tag


def sign_release(source, output, evidence, tag, sha):
    tag = stable_tag(tag)
    validate(source, tag, sha)
    output.mkdir(parents=True, exist_ok=False)
    evidence.mkdir(parents=True, exist_ok=False)
    team = expected_team()
    with tempfile.TemporaryDirectory(prefix="mitts-notary-payload-") as directory:
        root = Path(directory)
        binaries = {}
        metadata = {}
        for target in MAC_TARGETS:
            name = f"mitts-{tag}-{target}.tar.gz"
            metadata[target] = extract_archive(
                source / name, root / target, tag, sha, target
            )
            binaries[target] = root / target / "mitts"
        with credentials() as auth:
            cdhashes = {}
            for target, binary in binaries.items():
                run(
                    [
                        "codesign",
                        "--force",
                        "--sign",
                        auth["identity"],
                        "--keychain",
                        auth["keychain"],
                        "--identifier",
                        "com.mittsbuild.mitts",
                        "--options",
                        "runtime",
                        "--timestamp",
                        binary,
                    ]
                )
                cdhashes[target] = signature(binary, team)
            submission_zip = root / "submission.zip"
            # Exact allowlist: private P12/P8/keychain files are never in payload.
            with zipfile.ZipFile(submission_zip, "w", zipfile.ZIP_DEFLATED) as archive:
                for target, binary in binaries.items():
                    archive.write(binary, target + "/mitts")
            result = run(
                [
                    "xcrun",
                    "notarytool",
                    "submit",
                    submission_zip,
                    *auth["auth"],
                    "--wait",
                    "--timeout",
                    "20m",
                    "--output-format",
                    "json",
                ],
                timeout=1300,
            )
            submission = json.loads(result.stdout)
            if submission.get("status") != "Accepted":
                raise ValueError(
                    "Apple notarization was not Accepted; no release artifacts produced"
                )
            job = submission["id"]
            log_file = evidence / "apple-notary-log.json"
            run(
                ["xcrun", "notarytool", "log", job, *auth["auth"], log_file],
                timeout=120,
            )
            receipt = json.loads(log_file.read_text())
        zip_sha = hashlib.sha256(submission_zip.read_bytes()).hexdigest()
        for target, binary in binaries.items():
            receipt_matches(receipt, job, zip_sha, target, cdhashes[target])
            clean = [
                line
                for line in metadata[target].splitlines()
                if not line.startswith(
                    ("Signing-", "Signed-", "CodeDirectory-", "Notarization-")
                )
            ]
            clean += [
                f"Signing-Team: {team}",
                "Signed-Binary-SHA256: "
                + hashlib.sha256(binary.read_bytes()).hexdigest(),
                "CodeDirectory-Hash: " + cdhashes[target],
                "Notarization-ID: " + job,
                "Notarization-Status: Accepted",
                "Notarization-Receipt: " + json.dumps(receipt, separators=(",", ":")),
            ]
            (root / target / "BUILD.txt").write_text("\n".join(clean) + "\n")
            name = f"mitts-{tag}-{target}.tar.gz"
            archive = output / name
            with tarfile.open(archive, "w:gz") as content:
                for member in ["mitts", "BUILD.txt", "LICENSE-MIT", "LICENSE-APACHE"]:
                    content.add(root / target / member, arcname=member, recursive=False)
            (output / (name + ".sha256")).write_text(
                hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + name + "\n"
            )
        (evidence / "submission.json").write_text(
            json.dumps(
                {
                    "id": job,
                    "status": "Accepted",
                    "source": sha,
                    "tag": tag,
                    "zip_sha256": zip_sha,
                },
                indent=2,
            )
            + "\n"
        )
    print(
        "Supported Mac binary signed and notarized Accepted; final archives/checksums produced"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("tag")
    parser.add_argument("sha")
    args = parser.parse_args()
    sign_release(args.source, args.output, args.evidence, args.tag, args.sha)


if __name__ == "__main__":
    main()
