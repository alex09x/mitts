#!/usr/bin/env python3
# Mitts Build for Helix — editor integration for the Mitts Rust build tool
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Checks the Helix configuration against a real Helix (`hx` on PATH or $HX).

- both files are TOML with the expected values;
- `hx --health rust` reads languages.toml without an error and lists rust-analyzer;
- on Unix, Helix starts in a pseudo-terminal with config.toml without "Bad config";
  a broken key map in the same harness does give "Bad config", so the check can fail.
"""

import os
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
HX = os.environ.get("HX") or shutil.which("hx") or "hx"
CHECK = ["mitts", "check", "--workspace", "--all-targets", "--message-format=json"]
failures = []


def expect(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        failures.append(what)


def config_env(config_dir: Path) -> dict:
    """An environment whose Helix configuration directory is `config_dir/helix`."""
    env = dict(os.environ)
    env["XDG_CONFIG_HOME"] = str(config_dir)
    env["APPDATA"] = str(config_dir)  # Windows: %AppData%\helix
    env["HOME"] = str(config_dir)
    return env


def with_config(files: dict) -> Path:
    root = Path(tempfile.mkdtemp())
    (root / "helix").mkdir()
    for name, text in files.items():
        (root / "helix" / name).write_text(text, encoding="utf-8")
    return root


def start_in_pty(config_dir: Path, seconds: float = 4.0) -> bytes:
    """Helix's output for its first seconds in a pseudo-terminal (Unix)."""
    import pty
    import select
    import signal

    pid, fd = pty.fork()
    if pid == 0:
        os.environ.update(config_env(config_dir))
        os.environ["TERM"] = "xterm-256color"
        os.execvp(HX, [HX, str(HERE / "test" / "scratch.rs")])
    out = b""
    end = time.time() + seconds
    while time.time() < end:
        ready, _, _ = select.select([fd], [], [], 0.2)
        if ready:
            try:
                out += os.read(fd, 65536)
            except OSError:
                break
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    return out


languages_text = (HERE / "languages.toml").read_text(encoding="utf-8")
config_text = (HERE / "config.toml").read_text(encoding="utf-8")
languages = tomllib.loads(languages_text)
config = tomllib.loads(config_text)

ra = languages["language-server"]["rust-analyzer"]["config"]
expect(ra["check"]["overrideCommand"] == CHECK, "rust-analyzer checks through Mitts")
expect(ra["runnables"]["command"] == "mitts", "rust-analyzer runnables through Mitts")
keys = config["keys"]["normal"]["space"]["m"]
expect(
    keys == {k: f":sh mitts {c}" for k, c in
             {"c": "check", "b": "build", "t": "test", "r": "run", "k": "clippy"}.items()},
    "Space m keys run Mitts",
)

version = subprocess.run([HX, "--version"], capture_output=True, text=True)
print(version.stdout.strip() or version.stderr.strip())

health = subprocess.run(
    [HX, "--health", "rust"], capture_output=True, text=True,
    env=config_env(with_config({"languages.toml": languages_text})),
)
report = health.stdout + health.stderr
expect("Error" not in report, f"hx --health reads languages.toml without an error\n{report}")
expect("rust-analyzer" in report, "hx --health lists rust-analyzer for Rust")

broken = subprocess.run(
    [HX, "--health", "rust"], capture_output=True, text=True,
    env=config_env(with_config({"languages.toml": languages_text + "\nbogus = = 1\n"})),
)
expect("Error" in broken.stdout + broken.stderr, "a broken languages.toml is reported (control)")

if os.name == "posix":
    good = start_in_pty(with_config({"config.toml": config_text}))
    expect(b"Bad config" not in good, "Helix starts with config.toml")
    bad = start_in_pty(with_config({"config.toml": '[keys.normal]\nC-z = "no_such_command"\n'}))
    expect(b"Bad config" in bad, "a broken key map is reported (control)")
else:
    print("skip key map start-up check (no pseudo-terminal here)")

sys.exit(1 if failures else 0)
