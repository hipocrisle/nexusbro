#!/usr/bin/env python3
"""Contract test for downloads open/folder policy — REAL production function.

Compiles the actual production module desktop/src-tauri/src/downloads_open_policy.rs
with `rustc --test` (tiny single-file build, not a project build) and runs its
unit tests against real temp files. Also asserts the wiring contract in
zagruzki.rs. No app/browser execution.
"""
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
POLICY = ROOT / 'desktop/src-tauri/src/downloads_open_policy.rs'
ZAGRUZKI = ROOT / 'desktop/src-tauri/src/zagruzki.rs'


def main():
    report = {'schema': 1, 'production_policy_tests': None, 'wiring': {}, 'skipped': None}
    rustc = shutil.which('rustc')
    if not rustc:
        report['skipped'] = 'rustc_unavailable'
        print(json.dumps(report))
        return 1
    with tempfile.TemporaryDirectory(prefix='nb-policy-test-') as tmp:
        binary = Path(tmp) / 'policy_test'
        compile_run = subprocess.run(
            [rustc, '--test', '--edition', '2021', str(POLICY), '-o', str(binary)],
            capture_output=True, text=True, timeout=300)
        if compile_run.returncode != 0:
            report['production_policy_tests'] = 'compile_failed'
            print(json.dumps(report))
            return 1
        run = subprocess.run([str(binary)], capture_output=True, text=True, timeout=120)
        passed = 'test result: ok' in run.stdout and '0 failed' in run.stdout
        report['production_policy_tests'] = 'PASS' if passed else 'FAIL'
        report['policy_test_tail'] = run.stdout.strip().splitlines()[-1] if run.stdout.strip() else ''
    src = ZAGRUZKI.read_text(encoding='utf-8')
    report['wiring'] = {
        'uses_policy_open': 'crate::downloads_open_policy::check_open_target' in src,
        'uses_policy_folder': 'crate::downloads_open_policy::check_folder_target' in src,
        'no_inline_danger_list_left': 'DANGEROUS' not in src,
        'neutral_spawn_errors': 'map_err(|_|"Не удалось открыть' in src,
    }
    ok = report['production_policy_tests'] == 'PASS' and all(report['wiring'].values())

    # --- Реальный localhost-прогон собственной Fixture (без браузера) ---
    sys.path.insert(0, str(ROOT / 'tests' / 'gui'))
    import urllib.request
    import downloads_library_native as dln
    fixture = dln.Fixture()
    origin = fixture.start()
    progress = {}
    try:
        with urllib.request.urlopen(origin + '/file-known.bin', timeout=30) as response:
            progress['known_headers'] = (response.headers.get('Content-Length') == str(dln.KNOWN_TOTAL)
                                         and 'attachment' in (response.headers.get('Content-Disposition') or '')
                                         and 'file-known.bin' in response.headers.get('Content-Disposition'))
            body = response.read()
        progress['known_bytes_exact'] = len(body) == dln.KNOWN_TOTAL
        with urllib.request.urlopen(origin + '/file-unknown.bin', timeout=30) as response:
            progress['unknown_no_length'] = response.headers.get('Content-Length') is None
            body = response.read()
        progress['unknown_bytes_exact'] = len(body) == dln.KNOWN_TOTAL
        with urllib.request.urlopen(origin + '/note.txt', timeout=10) as response:
            progress['doc_bytes'] = len(response.read()) == 5
    finally:
        fixture.close()

    # --- FakeClient: sanitize-снимок и typed-коды (не rg-only) ---
    class FakeClient:
        def __init__(self, records): self.records = records
    fake = FakeClient([
        {'name': 'file-known.bin', 'status': 'Загружается', 'received': 65536, 'total': dln.KNOWN_TOTAL,
         'percent': 8, 'speed': 65536, 'url': 'http://secret/url', 'path': '/secret/path', 'id': 'x:1:2'},
        {'name': 'чужое-имя.bin', 'status': 'Неведомый', 'received': float('nan'), 'total': -5,
         'percent': 10 ** 15, 'speed': None},
        'not-a-dict',
    ])
    original_call = dln.call
    dln.call = lambda client, command, **kw: client.records
    try:
        summary = dln.download_summary(fake)
    finally:
        dln.call = original_call
    blob = json.dumps(summary, ensure_ascii=False)
    progress['summary_available'] = summary['available'] is True and summary['count'] == 2
    progress['summary_kind_enum'] = [r['file_kind'] for r in summary['records']] == ['known', 'other']
    progress['summary_status_enum'] = [r['status'] for r in summary['records']] == ['Загружается', 'other']
    progress['summary_scalars_bounded'] = summary['records'][1]['received'] is None \
        and summary['records'][1]['total'] is None and summary['records'][1]['percent'] is None
    progress['summary_no_raw_fields'] = 'secret' not in blob and 'http' not in blob \
        and 'чужое' not in blob
    progress['wait_messages_mapped'] = all(code in ('known_progress_timeout', 'unknown_total_timeout',
        'received_stall_timeout', 'doc_complete_timeout', 'known_complete_timeout',
        'unknown_complete_timeout', 'library_open_timeout') for code in dln.WAIT_MESSAGES.values())
    native_src = (ROOT / 'tests/gui/downloads_library_native.py').read_text(encoding='utf-8')
    progress['native_failure_caught'] = 'WAIT_MESSAGES.get(message' in native_src \
        and "'native_assertion'" in native_src
    progress['snapshot_before_and_failure'] = "report['downloads_before_wait']" in native_src \
        and "report['downloads_at_failure']" in native_src
    progress['downloads_serialized'] = 'time.sleep(.3)' not in native_src
    progress['preparation_false_on_real_run'] = "report['preparation'] = False" in native_src
    progress['shim_still_not_os_proof'] = "'open_launch': 'test_shim_only'" in native_src
    report['progress_facts'] = progress
    ok = ok and all(progress.values())
    print(json.dumps(report, ensure_ascii=False))
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
