#!/usr/bin/env python3
"""Offline contract tests for sanitized unlock receipts (no GUI/app).

Verifies: receipt carries only booleans (never the passphrase/value/length),
guard_pending is set before each unlock guard, exactly one trusted fill and
one physical unlock click remain, failure report gains allowlisted type +
pending guard, and existing fixed messages stay mapped.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = (ROOT / 'tests/gui/youtube_player_native.py').read_text(encoding='utf-8')

sys.path.insert(0, str(ROOT / 'tests' / 'gui'))
import youtube_player_native as ypn


class FakeClient:
    def __init__(self, values):
        self.values = values
        self.clicks = 0
    def js(self, expression):
        for marker, value in self.values.items():
            if marker in expression:
                return value
        return True
    def click(self, selector):
        self.clicks += 1


def main():
    checks = {}

    # trusted_fill receipt: только булевы, секрет не утекает
    secret = 'master-passphrase-123'
    client = FakeClient({'!e.disabled': True, 'activeElement': True, '.value===': True})
    receipt = {}
    original_wait = ypn.js_wait
    ypn.js_wait = lambda *a, **k: None
    try:
        import pyautogui  # noqa: F401 — win32 path not used on linux CI host
        win = True
    except Exception:
        win = False
    old_platform = ypn.sys.platform
    ypn.sys.platform = 'linux'
    import subprocess as sp
    original_run = sp.run
    sp.run = lambda *a, **k: type('R', (), {'returncode': 0})()
    try:
        fake_device = type('D', (), {'display': ':99'})()
        ypn.trusted_fill(client, fake_device, '[aria-label="x"]', secret, receipt=receipt)
    finally:
        ypn.js_wait = original_wait
        ypn.sys.platform = old_platform
        sp.run = original_run
    blob = json.dumps(receipt)
    checks['receipt_bools_only'] = all(type(v) is bool for v in receipt.values())
    checks['receipt_expected_keys'] = set(receipt) == {'input_present', 'input_enabled', 'form_focused', 'value_matches'}
    checks['secret_not_leaked'] = secret not in blob and 'length' not in blob and 'hash' not in blob

    # guard_pending последовательность в исходнике
    unlock_block = SRC[SRC.index("report['stage']='physical-unlock-'"):]
    unlock_block = unlock_block[:unlock_block.index("new-fixture-admission")]
    order = [m.group(1) for m in re.finditer(r"guard_pending'\]=(?:'([a-z_]+)'|None)", unlock_block)]
    checks['guard_sequence'] = order == ['trusted_fill', 'unlock_button_click', 'master_ui_unlock',
                                         'lock_browser_identity', 'old_grant_refusal', 'old_grant_revoked', None] or \
                               order[:6] == ['trusted_fill', 'unlock_button_click', 'master_ui_unlock',
                                             'lock_browser_identity', 'old_grant_refusal', 'old_grant_revoked']
    checks['exactly_one_fill'] = unlock_block.count('trusted_fill(') == 1
    checks['exactly_one_unlock_click'] = unlock_block.count("text_button('Разблокировать браузер')") == 1
    checks['no_js_form_submit'] = 'submit(' not in unlock_block and 'requestSubmit' not in unlock_block
    checks['no_unlock_retry_loop'] = not re.search(r'for (attempt|retry|_) in range|while .*(retry|attempt|unlock)', unlock_block)
    for key in ('gate_present', 'gate_closed', 'okno_exists', 'browser_unchanged', 'browser_positive',
                'old_refusal_received', 'old_state_absent', 'old_error_present'):
        checks['receipt_field_' + key] = f"unlock['{key}']" in unlock_block

    # failure report: тип из allowlist + pending guard
    checks['failure_type_allowlist'] = "report['failure_type']=type(error).__name__ if type(error).__name__ in allowed_types else 'Other'" in SRC
    checks['guard_pending_in_report'] = "report['unlock_guard_pending']=unlock_diag['guard_pending']" in SRC
    for message, code in (('Native unlock input missing', 'unlock_input_missing'),
                          ('Trusted unlock input did not match', 'unlock_input_mismatch'),
                          ('Actual master UI did not unlock browser', 'master_unlock_failed'),
                          ('Lock unexpectedly replaced the actual player browser', 'lock_browser_changed'),
                          ('Old player authority refusal missing', 'old_grant_refusal_missing'),
                          ('Old player authority survived browser lock', 'old_grant_survived')):
        checks['mapped_' + code] = f"'{message}':'{code}'" in SRC

    # требования не ослаблены
    checks['requires_intact'] = all(text in SRC for text in (
        "'Actual master UI did not unlock browser',25",
        "'Lock unexpectedly replaced the actual player browser'",
        "'Old player authority survived browser lock'"))

    failed = [name for name, ok in checks.items() if not ok]
    print(json.dumps({'unlock_receipt_checks': 'PASS' if not failed else 'FAIL', 'failed': failed}))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
