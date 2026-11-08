# Mitts Build — isolated binary installer regressions and native archive smoke.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import hashlib
import io
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INSTALLER = ROOT / "site/install.sh"
TAG = "v0.3.18"


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="mitts-installer-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / "commands"
        self.bin.mkdir()
        self.fixtures = self.root / "fixtures"
        self.fixtures.mkdir()
        self.prefix = self.root / "install with spaces"
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("MITTS_", "CARGO_"))
        }
        self.env.update(
            PATH=f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            HOME=str(self.root / "home"),
            MITTS_INSTALL_ROOT=str(self.prefix),
            FIXTURE_DIR=str(self.fixtures),
        )
        self.command(
            "curl",
            f"#!{sys.executable}\n"
            "import os, pathlib, shutil, sys\n"
            "args=sys.argv[1:]; out=args[args.index('-o')+1]\n"
            "url=args[args.index('-o')-1]\n"
            "fx=pathlib.Path(os.environ['FIXTURE_DIR'])\n"
            "if url.startswith('https://api.github.com/'):\n"
            " (fx/'api-calls.txt').open('a').write(url+'|'+' '.join(args)+'\\n')\n"
            " if os.environ.get('FAIL_API'): sys.exit(22)\n"
            " src=fx/('api-latest.json' if url.endswith('/releases/latest') else 'api-release.json' if '/releases/tags/' in url else 'api-asset-'+url.rsplit('/',1)[-1])\n"
            " if not src.is_file(): sys.exit(22)\n"
            " shutil.copyfile(src,out); sys.exit(0)\n"
            "if os.environ.get('FAIL_GITHUB'): sys.exit(22)\n"
            "if url.endswith('/latest'):\n"
            " print((pathlib.Path(os.environ['FIXTURE_DIR'])/'latest.txt').read_text(),end=''); sys.exit(0)\n"
            "name=url.rsplit('/',1)[-1]\n"
            "src=pathlib.Path(os.environ['FIXTURE_DIR'])/name\n"
            "if not src.is_file(): sys.exit(22)\n"
            "shutil.copyfile(src,out)\n",
        )
        self.platform("Linux", "x86_64")
        (self.fixtures / "latest.txt").write_text(
            f"https://github.com/alex09x/mitts/releases/tag/{TAG}"
        )

    def command(self, name, body):
        path = self.bin / name
        path.write_text(body)
        path.chmod(0o755)

    def platform(self, system, architecture):
        self.command(
            "uname",
            f'#!/bin/sh\ncase "$1" in -s) echo {system};; -m) echo {architecture};; esac\n',
        )

    def archive(
        self, target="x86_64-unknown-linux-gnu", version="0.3.18", check_location=False
    ):
        name = f"mitts-{TAG}-{target}.tar.gz"
        path = self.fixtures / name
        script = "#!/bin/sh\n"
        if check_location:
            script += 'case "$0" in "$MITTS_INSTALL_ROOT/bin/.mitts-install."*) ;; *) exit 77;; esac\n'
        data = (script + f'printf "mitts {version} (fixture)\\n"\n').encode()
        with tarfile.open(path, "w:gz") as archive:
            info = tarfile.TarInfo("mitts")
            info.size = len(data)
            info.mode = 0o755
            archive.addfile(info, io.BytesIO(data))
        self.checksum(path)
        return path

    def checksum(self, path):
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        (self.fixtures / "SHA256SUMS").write_text(f"{digest}  {path.name}\n")

    def run_installer(self, success=True):
        result = subprocess.run(
            ["sh", str(INSTALLER)],
            env=self.env,
            text=True,
            capture_output=True,
            timeout=30,
            check=False,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((self.prefix / "bin/mitts").is_file())
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(list(self.prefix.glob("bin/.mitts-install.*")))
        return result

    def test_latest_binary_and_all_platforms(self):
        for system, architecture, target in [
            ("Linux", "x86_64", "x86_64-unknown-linux-gnu"),
            ("Linux", "aarch64", "aarch64-unknown-linux-gnu"),
            ("Darwin", "arm64", "aarch64-apple-darwin"),
        ]:
            with self.subTest(target=target):
                self.platform(system, architecture)
                self.archive(target)
                result = self.run_installer()
                self.assertIn(target, result.stdout)

    def test_explicit_version_skips_latest(self):
        self.archive()
        (self.fixtures / "latest.txt").unlink()
        self.env["MITTS_VERSION"] = "0.3.18"
        self.run_installer()

    def test_default_cargo_home_prefix(self):
        self.archive()
        del self.env["MITTS_INSTALL_ROOT"]
        self.env["CARGO_HOME"] = str(self.root / "cargo home")
        self.prefix = Path(self.env["CARGO_HOME"])
        self.run_installer()

    def test_default_home_prefix(self):
        self.archive()
        del self.env["MITTS_INSTALL_ROOT"]
        self.prefix = Path(self.env["HOME"]) / ".cargo"
        self.run_installer()

    def test_bad_checksum_preserves_old_binary(self):
        self.archive()
        (self.fixtures / "SHA256SUMS").write_text(
            f"{'0' * 64}  mitts-{TAG}-x86_64-unknown-linux-gnu.tar.gz\n"
        )
        old = self.prefix / "bin/mitts"
        old.parent.mkdir(parents=True)
        old.write_text("existing binary")
        self.run_installer(False)
        self.assertEqual(old.read_text(), "existing binary")

    def test_wrong_binary_version(self):
        self.archive(version="0.2.0")
        self.run_installer(False)
        self.assertFalse((self.prefix / "bin/mitts").exists())

    def test_version_check_runs_at_destination_not_tmp(self):
        self.archive(check_location=True)
        self.run_installer()

    def test_missing_asset(self):
        self.run_installer(False)

    def test_missing_checksum(self):
        self.archive()
        (self.fixtures / "SHA256SUMS").unlink()
        self.run_installer(False)

    def test_duplicate_checksum(self):
        self.archive()
        path = self.fixtures / "SHA256SUMS"
        path.write_text(path.read_text() * 2)
        self.run_installer(False)

    def test_html_latest_response(self):
        (self.fixtures / "latest.txt").write_text("<!doctype html>")
        self.run_installer(False)

    def test_invalid_tag(self):
        self.env["MITTS_VERSION"] = "../../main"
        self.run_installer(False)

    def test_unsupported_platform(self):
        self.platform("Windows_NT", "x86_64")
        self.run_installer(False)

    def test_intel_macos_is_rejected_without_replacing_existing_installation(self):
        self.platform("Darwin", "x86_64")
        old = self.prefix / "bin/mitts"
        old.parent.mkdir(parents=True)
        old.write_text("existing binary")
        result = self.run_installer(False)
        self.assertIn("support ended with v0.3.19", result.stderr)
        self.assertEqual(old.read_text(), "existing binary")

    def test_unsupported_architecture(self):
        self.platform("Linux", "riscv64")
        self.run_installer(False)

    def api_fixtures(self, archive):
        sums = self.fixtures / "SHA256SUMS"
        (self.fixtures / "api-asset-101").write_bytes(archive.read_bytes())
        (self.fixtures / "api-asset-102").write_bytes(sums.read_bytes())
        base = "https://api.github.com/repos/alex09x/mitts/releases"
        (self.fixtures / "api-release.json").write_text(
            '{\n  "url": "%s/900",\n  "name": "%s",\n  "assets": [\n'
            '    {\n      "url": "%s/assets/101",\n      "id": 101,\n      "name": "%s",\n'
            '      "uploader": {"login": "x", "url": "https://api.github.com/users/x"}\n    },\n'
            '    {\n      "url": "%s/assets/102",\n      "id": 102,\n      "name": "SHA256SUMS"\n    }\n  ]\n}\n'
            % (base, TAG, base, archive.name, base)
        )
        (self.fixtures / "api-latest.json").write_text(
            '{\n  "url": "%s/900",\n  "tag_name": "%s",\n  "name": "%s"\n}\n'
            % (base, TAG, TAG)
        )

    def test_api_fallback_when_github_com_fails(self):
        self.api_fixtures(self.archive())
        self.env["FAIL_GITHUB"] = "1"
        result = self.run_installer()
        self.assertIn("api.github.com", result.stderr)
        calls = (self.fixtures / "api-calls.txt").read_text()
        self.assertIn("releases/latest", calls)
        self.assertIn("application/octet-stream", calls)
        self.assertNotIn("Authorization", calls)

    def test_api_fallback_with_explicit_version_and_token(self):
        self.api_fixtures(self.archive())
        self.env.update(FAIL_GITHUB="1", MITTS_VERSION=TAG, GITHUB_TOKEN="t0ken")
        self.run_installer()
        calls = (self.fixtures / "api-calls.txt").read_text()
        self.assertNotIn("releases/latest", calls)
        self.assertIn("Authorization: Bearer t0ken", calls)

    def test_api_fallback_still_verifies_checksum(self):
        self.api_fixtures(self.archive())
        (self.fixtures / "api-asset-102").write_text(
            f"{'0' * 64}  mitts-{TAG}-x86_64-unknown-linux-gnu.tar.gz\n"
        )
        self.env["FAIL_GITHUB"] = "1"
        self.run_installer(False)
        self.assertFalse((self.prefix / "bin/mitts").exists())

    def test_both_hosts_down_fails(self):
        self.archive()
        self.env.update(FAIL_GITHUB="1", FAIL_API="1", MITTS_VERSION=TAG)
        self.run_installer(False)

    def test_retries_are_requested(self):
        self.api_fixtures(self.archive())
        self.env.update(FAIL_GITHUB="1", MITTS_VERSION=TAG)
        self.run_installer()
        self.assertIn("--retry 3", (self.fixtures / "api-calls.txt").read_text())

    def test_script_is_shell_and_headers_are_text(self):
        self.assertTrue(INSTALLER.read_bytes().startswith(b"#!/bin/sh\n"))
        subprocess.run(["sh", "-n", str(INSTALLER)], check=True)
        headers = (ROOT / "site/_headers").read_text()
        self.assertIn("/install.sh\n  Content-Type: text/plain", headers)


def native_smoke(archive, tag):
    """Use the exact packaged archive through the installer on its native runner."""
    case = InstallerTests("test_missing_asset")
    case.setUp()
    try:
        (case.bin / "uname").unlink()
        archive = archive.resolve()
        shutil.copyfile(archive, case.fixtures / archive.name)
        case.checksum(case.fixtures / archive.name)
        case.env["MITTS_VERSION"] = tag
        case.run_installer()
        installed = case.prefix / "bin/mitts"
        with tarfile.open(archive) as content:
            original = content.extractfile("mitts").read()
        assert installed.read_bytes() == original, (
            "Installed binary differs from archive"
        )
        subprocess.run(
            [str(installed), "--help"], check=True, stdout=subprocess.DEVNULL
        )
        print(f"Native archive install passed: {archive.name}")
    finally:
        case.doCleanups()


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--archive":
        native_smoke(Path(sys.argv[2]), sys.argv[3])
    else:
        unittest.main()
