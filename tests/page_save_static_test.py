#!/usr/bin/env python3
"""Offline-проверки page-save модулей (Ctrl+S -> MHTML), без сборки и CEF.

Что доказывает: наличие обязательных guard'ов/связок в новых Rust-файлах,
отсутствие запрещённых паттернов (логи, сеть, renderer-path), корректность
civil-алгоритма имени файла (python-эталон против datetime) и rustfmt-parse.
Что НЕ доказывает: компиляцию, native save, реальный CDP — это за Root/CI.
"""

import json
import re
import shutil
import subprocess
import sys
import unittest
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHILD = ROOT / "desktop" / "src-tauri" / "src" / "cef_page_save.rs"
PARENT = ROOT / "desktop" / "src-tauri" / "src" / "page_save_parent.rs"

FORBIDDEN = ("println!", "eprintln!", "eprint!", "dbg!(",
             "reqwest", "std::net", "TcpStream", "ureq", "curl")
# \b: «dialog::» не должен матчиться как «log::»
FORBIDDEN_RE = (re.compile(r"\blog::"), re.compile(r"\btracing::"))


def assert_clean(test, text):
    for pat in FORBIDDEN:
        test.assertNotIn(pat, text, pat)
    for rx in FORBIDDEN_RE:
        test.assertIsNone(rx.search(text), rx.pattern)


def civil_from_days(z: int):
    """Тот же алгоритм (Howard Hinnant), что в cef_page_save.rs."""
    z += 719468
    era = z // 146097 if z >= 0 else (z - 146096) // 146097
    # Rust: целочисленное деление с усечением к нулю; для z>=0 совпадает с //
    doe = z - era * 146097
    yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
    y = yoe + era * 400
    doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
    mp = (5 * doy + 2) // 153
    d = doy - (153 * mp + 2) // 5 + 1
    m = mp + 3 if mp < 10 else mp - 9
    return (y + 1 if m <= 2 else y, m, d)


def make_filename(unix_secs: int) -> str:
    y, mo, d = civil_from_days(unix_secs // 86400)
    secs = unix_secs % 86400
    return "page-%04d%02d%02d-%02d%02d%02d.mhtml" % (
        y, mo, d, secs // 3600, (secs % 3600) // 60, secs % 60)


class PageSaveStaticTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.child = CHILD.read_text(encoding="utf-8")
        cls.parent = PARENT.read_text(encoding="utf-8")

    def test_01_files_exist(self):
        self.assertTrue(CHILD.is_file(), CHILD)
        self.assertTrue(PARENT.is_file(), PARENT)

    def test_02_child_uses_official_cdp_capture_snapshot_mhtml(self):
        self.assertIn('"Page.captureSnapshot"', self.child)
        self.assertIn('"format":"mhtml"', self.child)
        self.assertIn("cdp::reserve(1)", self.child)
        self.assertIn("DevToolsMessageObserver", self.child)

    def test_03_child_guards_present(self):
        for marker in (
            "locked_global", "route_generation", "currently_on(ThreadId::UI)",
            "document_matches", "MAX_MHTML_BYTES", "DIALOG_TIMEOUT",
            "CAPTURE_TIMEOUT", "valid_request", "valid_filename",
            "is_ascii_hexdigit", "fs::rename", "watch::channel",
            "Zeroizing", "page-save-state", '"cancelled":true',
            "MIME-Version",  # не выдаём HTML-only за MHTML
        ):
            self.assertIn(marker, self.child, marker)

    def test_04_child_has_no_logs_or_network(self):
        assert_clean(self, self.child)

    def test_05_parent_has_no_logs_or_network(self):
        assert_clean(self, self.parent)

    def test_06_parent_dialog_contract(self):
        for marker in (
            '"page-save"', 'add_filter("MHTML", &["mhtml"])', "set_file_name",
            "set_parent", "PageSaveTarget", "OknoKomanda", "locked_global",
            "is_absolute", "route_generation", "route_pending",
        ):
            self.assertIn(marker, self.parent, marker)

    def test_07_parent_never_trusts_renderer_path(self):
        # путь существует только из native dialog callback; renderer data
        # не содержит и не может содержать path
        self.assertNotIn('data["path"]', self.parent)
        self.assertNotIn("data.path", self.parent)
        self.assertIn("picked", self.parent)

    def test_08_child_never_logs_and_events_are_typed(self):
        # typed state event существует и компактен
        self.assertIn('"tip":"page-save"', self.child)
        self.assertIn('"tip":"page-save-state"', self.child)
        # в json! событиях нет path/document/url полей
        for event in re.findall(r"json!\(\{[^}]*\}\)", self.child):
            self.assertNotIn('"path"', event)
            self.assertNotIn('"url"', event)
            self.assertNotIn('"document"', event)

    def test_09_civil_algorithm_matches_datetime(self):
        for ts in (0, 86400, 951782400, 2147483647, 4102444800, 1234567890):
            y, m, d = civil_from_days(ts // 86400)
            ref = datetime.fromtimestamp(ts, tz=timezone.utc)
            self.assertEqual((y, m, d), (ref.year, ref.month, ref.day), ts)
            self.assertEqual(
                make_filename(ts),
                "page-%s.mhtml" % ref.strftime("%Y%m%d-%H%M%S"), ts)

    def test_10_rust_tests_pin_same_constants(self):
        # Rust #[cfg(test)] использует те же эталонные epoch, что test_09
        for marker in ("951782400", "2147483647", "page-20000229-000000.mhtml",
                       "page-20380119-031407.mhtml", "#[cfg(test)]"):
            self.assertIn(marker, self.child, marker)

    def test_11_rustfmt_parse(self):
        if shutil.which("rustfmt") is None:
            self.skipTest("rustfmt недоступен")
        for path in (CHILD, PARENT):
            proc = subprocess.run(
                ["rustfmt", "--check", "--edition", "2021", str(path)],
                capture_output=True, text=True, timeout=60)
            self.assertNotEqual(proc.returncode, 2,
                                f"rustfmt parse error {path}: {proc.stderr[:400]}")


def main():
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(PageSaveStaticTest)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    failed = {t.id().rsplit(".", 1)[-1] for t, _ in result.failures + result.errors}
    names = sorted(n for n in dir(PageSaveStaticTest) if n.startswith("test_"))
    checks = {n: (n not in failed) for n in names}
    print("RECEIPT " + json.dumps(
        {"total": result.testsRun, "failed": len(failed), "checks": checks},
        sort_keys=True))
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
