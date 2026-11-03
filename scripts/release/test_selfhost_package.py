# Mitts Build — immutable CAS output packaging regression.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import hashlib
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

import test_ci_scripts as fixtures

ROOT = Path(__file__).resolve().parents[2]


class SelfhostPackagingTests(unittest.TestCase):
    def test_signing_private_copy_does_not_mutate_builder_cas_inode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            source.mkdir()
            for name, content in {
                "Cargo.toml": '[package]\nname = "mitts"\nversion = "0.3.20"\n',
                "Cargo.lock": "fixture lock bytes",
                "LICENSE-MIT": "MIT fixture",
                "LICENSE-APACHE": "Apache fixture",
            }.items():
                (source / name).write_text(content)

            def git(*args):
                return subprocess.check_output(
                    [
                        "git",
                        "-c",
                        "commit.gpgsign=false",
                        "-c",
                        "tag.gpgsign=false",
                        "-C",
                        str(source),
                        *args,
                    ],
                    text=True,
                ).strip()

            git("init", "-q")
            git("config", "user.name", "Fixture")
            git("config", "user.email", "fixture@example.invalid")
            git("add", ".")
            git("commit", "-qm", "fixture")
            sha = git("rev-parse", "HEAD")
            git("tag", "v0.3.20")
            tools = root / "tools"
            tools.mkdir()
            builder = tools / "mitts"
            fixtures.write_executable(
                builder,
                """\
                import os, pathlib, sys
                if sys.argv[1:] == ["--version"]:
                    print("mitts 0.3.19 (bootstrap)"); sys.exit(0)
                assert sys.argv[1] == "build" and "--native" in sys.argv and "--release" in sys.argv
                target=pathlib.Path(sys.argv[sys.argv.index("--target-dir")+1])/"release"
                target.mkdir(parents=True)
                payload=("#!/bin/sh\\nprintf 'mitts 0.3.20 ("+os.environ["FIXTURE_SHA"]+")\\\\n'\\n").encode()
                obj=pathlib.Path(os.environ["FIXTURE_CAS"])
                obj.write_bytes(payload); obj.chmod(0o555)
                os.link(obj,target/"mitts")
            """,
            )
            fixtures.write_executable(
                tools / "rustc", "print('rustc fixture\\nhost: aarch64-apple-darwin')"
            )
            fixtures.write_executable(tools / "sw_vers", "print('macOS fixture')")
            fixtures.write_executable(
                tools / "codesign",
                """\
                import pathlib, sys
                if "--verify" not in sys.argv:
                    with pathlib.Path(sys.argv[-1]).open("a") as output: output.write("# signed private copy\\n")
            """,
            )
            fixtures.write_executable(
                tools / "cargo",
                "raise SystemExit('Cargo must not compile this release')",
            )
            home = root / "home"
            home.mkdir()
            obj = root / "cas-object"
            env = {
                **os.environ,
                "PATH": str(tools) + os.pathsep + os.environ["PATH"],
                "HOME": str(home),
                "MITTS_CI_BIN": str(builder),
                "MITTS_CI_CACHE_DIR": str(root / "cache"),
                "FIXTURE_SHA": sha,
                "FIXTURE_CAS": str(obj),
            }
            result = subprocess.run(
                [
                    "bash",
                    str(ROOT / "scripts/release/package.sh"),
                    str(source),
                    "v0.3.20",
                    "aarch64-apple-darwin",
                    str(root / "dist"),
                ],
                env=env,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            built = source / "target/mitts-release/release/mitts"
            self.assertEqual(obj.stat().st_ino, built.stat().st_ino)
            self.assertNotIn(b"signed private copy", obj.read_bytes())
            with tarfile.open(
                root / "dist/mitts-v0.3.20-aarch64-apple-darwin.tar.gz"
            ) as archive:
                packaged = archive.extractfile("mitts").read()
                self.assertIn(b"signed private copy", packaged)
                self.assertNotEqual(
                    hashlib.sha256(packaged).digest(),
                    hashlib.sha256(obj.read_bytes()).digest(),
                )
