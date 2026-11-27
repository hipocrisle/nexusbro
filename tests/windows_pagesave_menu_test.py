#!/usr/bin/env python3
"""Pure tests for windows_app_window_handles (page_save_native menu fix).

Stubbed Win32: proves native top-level and child HWNDs are enumerated with
strict PID/visibility binding. It does not establish the real popup style
or prove why UIA's earlier desktop projection omitted the menu.
"""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[0] / 'gui'))
from page_save_native import windows_app_window_handles

# window table: hwnd -> (pid, visible, [children])
APP = 100


def make_api(table):
    def enum_top(collect):
        for hwnd in table:
            collect(hwnd)
    def enum_children(parent, collect):
        for child in table.get(parent, (None, None, []))[2]:
            collect(child)
    def pid_of(hwnd):
        return table[hwnd][0]
    def visible(hwnd):
        return table[hwnd][1]
    return dict(enum_top=enum_top, enum_children=enum_children,
                pid_of=pid_of, visible=visible)


class WindowsMenuEnumTest(unittest.TestCase):
    def test_child_popup_of_app_pid_is_seen(self):
        # 1 = shell top-level (app), 2 = menu popup CHILD of shell (app),
        # 3 = foreign top-level, 4 = foreign child of foreign.
        table = {1: (APP, True, [2]), 2: (APP, True, []),
                 3: (999, True, [4]), 4: (999, True, [])}
        found = windows_app_window_handles(APP, **make_api(table))
        self.assertEqual(found, {1, 2})

    def test_top_level_only_enumeration_would_miss_popup(self):
        table = {1: (APP, True, [2]), 2: (APP, True, [])}
        api = make_api(table)
        found = windows_app_window_handles(APP, **api)
        top_only = {h for h in (1,) if table[h][0] == APP}
        self.assertIn(2, found - top_only)

    def test_foreign_pid_never_included(self):
        table = {1: (APP, True, [2]), 2: (999, True, []),
                 5: (999, True, [])}
        found = windows_app_window_handles(APP, **make_api(table))
        self.assertEqual(found, {1})

    def test_invisible_windows_skipped(self):
        table = {1: (APP, True, [2, 6]), 2: (APP, False, []), 6: (APP, True, [])}
        found = windows_app_window_handles(APP, **make_api(table))
        self.assertEqual(found, {1, 6})

    def test_broken_window_does_not_break_enumeration(self):
        table = {1: (APP, True, [2]), 2: (APP, True, [])}
        api = make_api(table)
        original = api['pid_of']
        def flaky(hwnd):
            if hwnd == 2:
                raise RuntimeError('window died')
            return original(hwnd)
        api['pid_of'] = flaky
        found = windows_app_window_handles(APP, **api)
        self.assertEqual(found, {1})

    def test_diff_semantics_new_popup_only(self):
        before_table = {1: (APP, True, [])}
        after_table = {1: (APP, True, [2]), 2: (APP, True, [])}
        before = windows_app_window_handles(APP, **make_api(before_table))
        after = windows_app_window_handles(APP, **make_api(after_table))
        self.assertEqual(after - before, {2})


def main():
    suite = unittest.TestLoader().loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == '__main__':
    sys.exit(main())
