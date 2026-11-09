# Mitts Build — README benchmark chart and translation checker regressions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CHARTS = ROOT / "scripts" / "site" / "readme_charts.py"
CHECKER = ROOT / "scripts" / "check_readme_translations.py"
BENCH_DATA = ROOT / "docs" / "data" / "benchmarks.json"


class ReadmeChartsAndTranslationsTests(unittest.TestCase):
    def _require_bench_data(self):
        if not BENCH_DATA.is_file() and os.environ.get("CI") != "true":
            self.skipTest("docs/data/benchmarks.json omitted from remote sync mirror")

    def test_committed_readme_charts_match_generator(self):
        self._require_bench_data()
        res = subprocess.run(
            [sys.executable, str(CHARTS), "--check"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)

    def test_chart_check_detects_stale_or_missing_svg(self):
        self._require_bench_data()
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp)
            shutil.copytree(ROOT / "docs" / "img", out_dir, dirs_exist_ok=True)
            target = out_dir / "bench-scenarios-light.svg"
            target.write_text(target.read_text(encoding="utf-8") + "<!-- drift -->\n", encoding="utf-8")
            res = subprocess.run(
                [sys.executable, str(CHARTS), "--out-dir", str(out_dir), "--check"],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(res.returncode, 0)
            self.assertIn("bench-scenarios-light.svg", res.stderr)

    def test_all_nine_readme_translations_pass_checker(self):
        res = subprocess.run(
            [sys.executable, str(CHECKER)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)

    def test_checker_validates_srcset_and_image_filenames(self):
        with tempfile.TemporaryDirectory() as tmp:
            readme_dir = Path(tmp) / "readme"
            readme_dir.mkdir()
            # Copy a valid translation inside the real repo's readme/ dir structure via a temp file inside ROOT/readme?
            # Never mutate ROOT: import check_readme_translations and call check() on a temp path inside a virtual subdir.
            sys.path.insert(0, str(ROOT / "scripts"))
            import check_readme_translations as crt

            canonical = (ROOT / "README.md").read_text(encoding="utf-8")
            valid_ru = (ROOT / "readme" / "README.ru.md").read_text(encoding="utf-8")

            # 1. Broken srcset path (missing ../ prefix from readme/)
            bad_srcset = valid_ru.replace(
                'srcset="../docs/img/bench-scenarios-dark.svg"',
                'srcset="docs/img/bench-scenarios-dark.svg"',
                1,
            )
            fake_path = ROOT / "readme" / "_tmp_check.md"
            self.assertFalse(fake_path.exists())
            probe = Path(tmp) / "README.ru.md"
            probe.write_text(bad_srcset, encoding="utf-8")
            errors = crt.local_link_errors(ROOT / "readme", crt.FENCES.sub("", bad_srcset))
            self.assertTrue(
                any("broken local link/image: docs/img/bench-scenarios-dark.svg" in e for e in errors),
                errors,
            )

            # 2. Different image filename from canonical English README
            swapped = valid_ru.replace("bench-scenarios-dark.svg", "bench-scenarios-light.svg", 1)
            self.assertNotEqual(crt.image_filenames(swapped), crt.image_filenames(canonical))

    def test_svg_subtitles_and_bar_label_gaps(self):
        ns = {"s": "http://www.w3.org/2000/svg"}
        for svg_path in sorted((ROOT / "docs" / "img").glob("bench-*.svg")):
            text = svg_path.read_text(encoding="utf-8")
            self.assertNotIn("Mitts mitts", text, svg_path.name)
            elems = list(ET.fromstring(text))
            for idx, el in enumerate(elems):
                if el.tag.endswith("rect") and el.find("s:title", ns) is not None:
                    bar_end = float(el.attrib["x"]) + float(el.attrib["width"])
                    label_x = float(elems[idx + 1].attrib["x"])
                    self.assertGreaterEqual(
                        label_x - bar_end,
                        10.0,
                        f"{svg_path.name}: label touches bar ({label_x} vs {bar_end})",
                    )
