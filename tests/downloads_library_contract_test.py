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
    print(json.dumps(report, ensure_ascii=False))
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
