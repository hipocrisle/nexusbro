#!/usr/bin/env python3
"""Offline tests for the initial address-entry receipt (no Windows/GUI).

Pure classifier (windows_input_receipt) via direct import; wiring facts of
windows_physical.py via source (module itself is Windows-only: argparse+windll
at import)."""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests' / 'gui'))
import windows_input_receipt as r

SRC = (ROOT / 'tests' / 'gui' / 'windows_physical.py').read_text(encoding='utf-8')


def main():
    checks = {}
    fixture = 'http://127.0.0.1:8080/page'

    # Классификация
    checks['exact'] = r.classify_value(fixture, fixture) == {
        'actual_length': len(fixture), 'match_class': 'exact', 'first_mismatch': None}
    checks['empty'] = r.classify_value('', fixture)['match_class'] == 'empty'
    checks['prefix'] = r.classify_value(fixture[:10], fixture)['match_class'] == 'prefix'
    checks['contains'] = r.classify_value('127.0.0.1:8080', fixture)['match_class'] == 'contains'
    checks['other'] = r.classify_value('zzz', fixture)['match_class'] == 'other'
    checks['first_mismatch_index'] = r.classify_value(
        fixture[:5] + 'X' + fixture[6:], fixture)['first_mismatch'] == 5
    checks['length_diff_mismatch'] = r.classify_value(fixture + 'x', fixture)['first_mismatch'] == len(fixture)

    # Границы и приватность
    long_secret = 'p@ssw0rd-' * 800
    receipt = r.classify_value(long_secret, fixture)
    blob = json.dumps(receipt)
    checks['length_bounded'] = receipt['actual_length'] == r.MAX_VALUE_LENGTH
    checks['no_raw_secret'] = 'p@ssw0rd' not in blob and 'http' not in blob
    checks['mismatch_bounded'] = receipt['first_mismatch'] is None or 0 <= receipt['first_mismatch'] <= r.MAX_VALUE_LENGTH
    checks['non_string_actual'] = r.classify_value(None, fixture)['match_class'] == 'empty'
    checks['classes_fixed'] = all(r.classify_value(a, fixture)['match_class'] in r.MATCH_CLASSES
                                  for a in ('', fixture, fixture[:3], 'x', None, 123))

    # Проводка в windows_physical.py (source-факты)
    block = SRC[SRC.index("assert initial_address is not None"):]
    block = block[:block.index("gui.press('enter')")]
    checks['single_click'] = block.count('click_input()') == 1
    checks['single_write'] = block.count('gui.write(') == 1
    checks['single_ctrl_a'] = block.count("gui.hotkey('ctrl','a')") == 1
    checks['no_retype_no_js'] = 'anchor.click' not in block and block.count('gui.write(') == 1
    checks['focus_wait_bounded'] = 'CurrentHasKeyboardFocus' in block and 'focus_deadline=time.monotonic()+3' in block
    checks['focus_typed_failure'] = "raise AssertionError('Initial address control did not take keyboard focus')" in block
    checks['modifiers_checked'] = 'ctrl_released' in block and 'shift_released' in block
    checks['humanlike_interval'] = 'gui.write(fixture, interval=.02)' in block and "entry={'focus_ready':focus_ready,'interval':0.02}" in block
    checks['old_rate_not_in_initial'] = 'interval=.003' not in block  # исторические кейсы не тронуты
    checks['receipt_before_assert'] = block.index("results.append({'initial_address_entry':entry})") < block.index(
        "assert actual_value==fixture,'Initial physical address entry failed'")
    checks['exact_assert_kept'] = "assert actual_value==fixture,'Initial physical address entry failed'" in block
    checks['late_reread_single'] = block.count('time.sleep(1)') == 1 and "'late_exact'" in block
    checks['abi_restype_declared'] = 'u.GetForegroundWindow.restype = wintypes.HWND' in block \
        and 'u.GetAsyncKeyState.restype = wintypes.SHORT' in block
    checks['diag_does_not_mask'] = "entry['value_check']='unavailable'" in block \
        and "entry['focus_facts']='unavailable'" in block

    failed = [name for name, ok in checks.items() if not ok]
    print(json.dumps({'initial_input_checks': 'PASS' if not failed else 'FAIL', 'failed': failed}))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
