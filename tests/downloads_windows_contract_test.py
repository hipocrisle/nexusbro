#!/usr/bin/env python3
"""Offline contract tests for downloads_windows_native helpers (no GUI).

Negative cases required by ROOT review: exact-path vs note.txt.extra, foreign
window/PID, reused PID birth, missing folder proof, unknown notepad location.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tests' / 'gui'))
import downloads_windows_native as m


def fake_snapshot(rows):
    return {pid: {'name': name, 'exe': exe, 'cmdline': cmd, 'birth': birth}
            for pid, name, exe, cmd, birth in rows}


def main():
    checks = {}
    target = 'C:\\Users\\runner\\AI\\dl\\downloads\\note.txt'

    # normalize/argv exactness
    checks['verbatim_prefix_normalized'] = m.normalize_win_path('\\\\?\\C:\\A\\b.txt') == 'c:\\a\\b.txt'
    checks['quotes_normalized'] = m.normalize_win_path('"C:\\A\\B.txt"') == 'c:\\a\\b.txt'
    checks['exact_argv_match'] = m.argv_has_exact_path(['notepad.exe', target], target) is True
    checks['extra_suffix_rejected'] = m.argv_has_exact_path(['notepad.exe', target + '.extra'], target) is False
    checks['substring_rejected'] = m.argv_has_exact_path(['notepad.exe', 'x' + target], target) is False
    checks['joined_only_rejected'] = m.argv_has_exact_path(['notepad.exe ' + target], target) is False

    # known notepad locations
    checks['system32_notepad_known'] = m.is_known_notepad('C:\\Windows\\System32\\notepad.exe') is True
    checks['windowsapps_notepad_known'] = m.is_known_notepad(
        'C:\\Program Files\\WindowsApps\\Microsoft.WindowsNotepad_11.1_x64__8weky\\notepad.exe') is True
    checks['random_notepad_rejected'] = m.is_known_notepad('C:\\Tools\\notepad.exe') is False
    checks['other_editor_rejected'] = m.is_known_notepad('C:\\Windows\\System32\\cmd.exe') is False

    # own_born_editor with fake snapshots
    before = fake_snapshot([(100, 'notepad.exe', 'C:\\Windows\\System32\\notepad.exe', ['notepad.exe'], 1000.0)])
    m.processes_snapshot = lambda: fake_snapshot([
        (100, 'notepad.exe', 'C:\\Windows\\System32\\notepad.exe', ['notepad.exe'], 1000.0),
        (200, 'notepad.exe', 'C:\\Windows\\System32\\notepad.exe', ['notepad.exe', target], 2000.0),
    ])
    import psutil
    real_process = psutil.Process
    psutil.Process = lambda pid: type('P', (), {'pid': pid})()
    try:
        found = m.own_born_editor(before, target, 1999.0)
        checks['own_born_found'] = bool(found and found['pid'] == 200 and found['exe_known'])
        # PID present in before (foreign) must never match
        m.processes_snapshot = lambda: fake_snapshot([
            (100, 'notepad.exe', 'C:\\Windows\\System32\\notepad.exe', ['notepad.exe', target], 1000.0)])
        checks['foreign_pid_rejected'] = m.own_born_editor(before, target, 1999.0) is None
        # PID reused but born BEFORE the click must never match
        m.processes_snapshot = lambda: fake_snapshot([
            (300, 'notepad.exe', 'C:\\Windows\\System32\\notepad.exe', ['notepad.exe', target], 1500.0)])
        checks['reused_pid_birth_rejected'] = m.own_born_editor(before, target, 1999.0) is None
        # unknown notepad location rejected even with exact path
        m.processes_snapshot = lambda: fake_snapshot([
            (400, 'notepad.exe', 'C:\\Tools\\notepad.exe', ['notepad.exe', target], 2000.0)])
        checks['unknown_location_rejected'] = m.own_born_editor(before, target, 1999.0) is None
    finally:
        psutil.Process = real_process

    # explorer proof: foreign window / no fresh explorer -> Blocked (unclosed)
    class BlockedExc(Exception):
        pass
    try:
        m.explorer_folder_proof(fake_snapshot([(1, 'explorer.exe', 'C:\\Windows\\explorer.exe', [], 1.0)]),
                                Path('C:\\dl'), snapshot=fake_snapshot([(1, 'explorer.exe', 'C:\\Windows\\explorer.exe', [], 1.0)]))
        checks['foreign_explorer_rejected'] = False
    except Exception as error:
        checks['foreign_explorer_rejected'] = 'own_born' in str(error) or 'explorer' in str(error)
    try:
        m.explorer_folder_proof(fake_snapshot([]), Path('C:\\dl'),
                                snapshot=fake_snapshot([(9, 'explorer.exe', 'C:\\Windows\\explorer.exe', [], 5.0)]))
        # fresh explorer exists but no UIA address proof on this host -> must be Blocked, not proof
        checks['missing_uia_unclosed'] = False
    except Exception:
        checks['missing_uia_unclosed'] = True

    # report contract
    report = m.initial_report()
    checks['split_completion_flags'] = report['open_completed'] is False and report['folder_completed'] is False
    checks['full_acceptance_default_false'] = report['full_acceptance'] is False
    checks['folder_default_unclosed'] = report['folder_proof'] == 'unclosed'

    failed = [name for name, ok in checks.items() if not ok]
    print(json.dumps({'windows_contract_checks': 'PASS' if not failed else 'FAIL', 'failed': failed}))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
