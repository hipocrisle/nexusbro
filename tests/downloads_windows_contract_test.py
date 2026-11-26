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
    checks['temp_notepad_rejected'] = m.is_known_notepad('C:\\Windows\\Temp\\notepad.exe') is False
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

    # Shell-window proof via DI fakes (no GUI)
    import time as _time
    class FakeItem:
        def __init__(self, hwnd, url): self.HWND = hwnd; self.LocationURL = url
    class FakeWindows:
        def __init__(self, items): self._items = items; self.Count = len(items)
        def Item(self, i): return self._items[i]
    folder = 'C:\\AI\\dl folder'
    url = 'file:///C:/AI/dl%20folder'
    now = _time.time()
    good_info = lambda pid: {'exe': 'C:\\Windows\\explorer.exe', 'birth': now - 5000, 'name': 'explorer.exe'}
    before = {'com': {111}, 'uia': {111}}
    proof = m.explorer_folder_proof(before, folder,
        shell_windows=FakeWindows([FakeItem(111, url), FakeItem(222, url)]),
        pid_of=lambda hwnd: 900, process_info=good_info, uia_windows=[])
    checks['new_hwnd_exact_url_proof'] = proof['hwnd'] == 222 and proof['proof'] == 'com_locationurl_exact'
    try:
        m.explorer_folder_proof(before, folder,
            shell_windows=FakeWindows([FakeItem(111, url)]), pid_of=lambda h: 900,
            process_info=good_info, uia_windows=[])
        checks['old_hwnd_rejected'] = False
    except Exception:
        checks['old_hwnd_rejected'] = True
    try:
        m.explorer_folder_proof(before, folder,
            shell_windows=FakeWindows([FakeItem(222, url + '.extra')]), pid_of=lambda h: 900,
            process_info=good_info, uia_windows=[])
        checks['url_extra_rejected'] = False
    except Exception:
        checks['url_extra_rejected'] = True
    try:
        m.explorer_folder_proof(before, folder,
            shell_windows=FakeWindows([FakeItem(222, url)]), pid_of=lambda h: 900,
            process_info=lambda pid: {'exe': 'C:\\Tools\\explorer.exe', 'birth': now - 5000}, uia_windows=[])
        checks['wrong_exe_rejected'] = False
    except Exception:
        checks['wrong_exe_rejected'] = True
    try:
        m.explorer_folder_proof(before, folder,
            shell_windows=FakeWindows([FakeItem(222, url)]), pid_of=lambda h: 900,
            process_info=lambda pid: {'exe': 'C:\\Windows\\explorer.exe', 'birth': now + 5000}, uia_windows=[])
        checks['future_birth_rejected'] = False
    except Exception:
        checks['future_birth_rejected'] = True
    # UIA fallback: NEW hwnd + exact address; substring-only rejected
    proof = m.explorer_folder_proof(before, folder, shell_windows=FakeWindows([]),
        pid_of=lambda h: 900, process_info=good_info,
        uia_windows=[{'hwnd': 333, 'pid': 901, 'addresses': ['Address: ' + folder]}])
    checks['uia_new_hwnd_exact'] = proof['proof'] == 'uia_new_hwnd_exact'
    try:
        m.explorer_folder_proof(before, folder, shell_windows=FakeWindows([]),
            pid_of=lambda h: 900, process_info=good_info,
            uia_windows=[{'hwnd': 333, 'pid': 901, 'addresses': ['Address: ' + folder + ' extra']},
                         {'hwnd': 111, 'pid': 901, 'addresses': [folder]}])
        checks['uia_substring_and_old_rejected'] = False
    except Exception:
        checks['uia_substring_and_old_rejected'] = True

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
