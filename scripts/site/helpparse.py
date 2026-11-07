# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Read a command's options from `mitts <command> --help` (clap output)."""
import re
import subprocess

OPT = re.compile(r"^  (?:(-\w), |    )(--[\w-]+)((?:\.\.\.)|(?:\[=<[^>]+>\])|(?: <[^>]+>(?:\.\.\.)?))?(?:\s{2,}(\S.*))?$")


def run_help(binary, command):
    return subprocess.run([binary, command, "--help"], capture_output=True, text=True, check=True).stdout


def parse(text):
    """Return (summary, usage, options, trailer). An option is a dict(short, long, value, text)."""
    lines = text.rstrip("\n").split("\n")
    summary = lines[0]
    usage = next((l[7:] for l in lines if l.startswith("Usage: ")), "")
    opts, cur, section, trailer = [], None, None, []
    for l in lines[1:]:
        if l in ("Options:", "Arguments:"):
            section, cur = l[:-1], None
            continue
        if section == "Options":
            m = OPT.match(l)
            if m:
                cur = {"short": m.group(1), "long": m.group(2), "value": (m.group(3) or "").strip(),
                       "text": (m.group(4) or "").strip()}
                opts.append(cur)
                continue
            if l.startswith("      ") and cur is not None and l.strip():
                cur["text"] = (cur["text"] + " " + l.strip()).strip()
                continue
            if not l.strip():
                continue
            section, cur = "tail", None
        if section == "tail" and l.strip():
            trailer.append(l.strip())
    return summary, usage, opts, trailer
