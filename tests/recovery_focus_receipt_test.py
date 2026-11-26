#!/usr/bin/env python3
"""Offline contract tests for the recovery-lock focus receipt (no GUI).

Extracts the pure recovery_focus_role via ast (the module itself runs argparse
at import and is Windows-only), verifies classification and privacy, and
source-checks the wiring: single Ctrl+Shift+L, original error preserved,
sanitized receipt fields only.
"""
import ast
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = (ROOT / 'tests/gui/windows_physical.py').read_text(encoding='utf-8')


def load_role():
    tree = ast.parse(SRC)
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name == 'recovery_focus_role':
            namespace = {}
            exec(compile(ast.Module(body=[node], type_ignores=[]), '<role>', 'exec'), namespace)
            return namespace['recovery_focus_role']
    raise AssertionError('recovery_focus_role not found')


def main():
    role = load_role()
    checks = {}

    # Классификация
    checks['none_without_focus'] = role(0, 10, {10, 11}, '', []) == 'none'
    checks['shell_main_process'] = role(10, 10, {10, 11}, 'nexusbro.exe', ['nexusbro.exe']) == 'shell'
    checks['webkit_webview_child'] = role(11, 10, {10, 11}, 'msedgewebview2.exe', ['msedgewebview2.exe']) == 'webkit'
    checks['cef_renderer'] = role(12, 10, {10, 12}, 'nexusbro.exe', ['nexusbro.exe', '--type=renderer']) == 'cef'
    checks['foreign_process'] = role(99, 10, {10, 11}, 'notepad.exe', ['notepad.exe']) == 'foreign'
    checks['family_nonrenderer_is_shell'] = role(13, 10, {10, 13}, 'crashpad.exe', ['crashpad.exe']) == 'shell'

    # Приватность: функция возвращает только строку-роль из allowlist
    allowed = {'webkit', 'cef', 'shell', 'none', 'foreign'}
    checks['role_allowlist'] = all(role(pid, 10, {10, 11, 12}, name, cmd) in allowed for pid, name, cmd in
                                   [(0, '', []), (10, 'nexusbro.exe', []), (11, 'msedgewebview2.exe', []),
                                    (12, 'nexusbro.exe', ['--type=gpu']), (99, 'x.exe', [])])

    # Обвязка в исходнике
    lock_block = SRC[SRC.index('def recovery_lock'):]
    lock_block = lock_block[:lock_block.index('def recovery_choose_original')]
    checks['single_shortcut'] = lock_block.count("gui.hotkey('ctrl','shift','l')") == 1
    checks['no_shortcut_retry'] = 'for ' not in lock_block.replace('for x in uia_descendants', '') and 'while ' not in lock_block
    checks['no_js_lock'] = 'invoke(' not in lock_block and 'lock_browser' not in lock_block
    checks['original_error_reraised'] = 'raise  # первоначальная ошибка сохраняется' in lock_block
    checks['receipt_before_and_after'] = "'before':before" in lock_block and "'after':recovery_focus_receipt()" in lock_block
    checks['receipt_on_failure_path'] = lock_block.index('except Exception:') < lock_block.index("'after':recovery_focus_receipt()")

    # Санитизация квитанции: только фиксированные поля, без PID/заголовков/путей
    receipt_fn = SRC[SRC.index('def recovery_focus_receipt'):SRC.index('def recovery_lock')]
    checks['receipt_fields_fixed'] = all(key in receipt_fn for key in
        ("'owned_foreground'", "'focus_role'", "'ctrl_released'", "'shift_released'", "'uia_gate'"))
    checks['receipt_no_raw_values'] = "'title'" not in receipt_fn and 'window_text' not in receipt_fn \
        and "receipt['pid" not in receipt_fn and "receipt['hwnd" not in receipt_fn \
        and "receipt['title" not in receipt_fn and "receipt['path" not in receipt_fn
    checks['uia_gate_bool_states'] = "'present'" in receipt_fn and "'missing'" in receipt_fn

    failed = [name for name, ok in checks.items() if not ok]
    print(json.dumps({'recovery_focus_checks': 'PASS' if not failed else 'FAIL', 'failed': failed}))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
