#!/usr/bin/env python3
"""Pure tests: unique_menu_item dedup + windows_owned_window_screenshot scope."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[0] / 'gui'))
from page_save_native import unique_menu_item, windows_owned_window_screenshot


class UniqueMenuItemTest(unittest.TestCase):
    def test_duplicate_runtime_id_collapses_to_one(self):
        el = object()
        self.assertIs(unique_menu_item([((1, 2), el), ((1, 2), el)]), el)

    def test_two_distinct_controls_stay_ambiguous(self):
        self.assertIsNone(unique_menu_item([((1,), object()), ((2,), object())]))

    def test_empty_and_missing_rid_are_none(self):
        self.assertIsNone(unique_menu_item([]))
        self.assertIsNone(unique_menu_item([(None, object())]))

    def test_single_match_returned(self):
        el = object()
        self.assertIs(unique_menu_item([((7, 9), el)]), el)

    def test_known_control_plus_unidentified_match_is_not_unique(self):
        known = object()
        self.assertIsNone(unique_menu_item([((1,2),known),(None,object())]))


class OwnedScreenshotTest(unittest.TestCase):
    def run_shot(self, table, out='/tmp'):
        calls = []
        def enum_top(collect):
            for hwnd in table:
                collect(hwnd)
        api = dict(
            enum_top=enum_top,
            pid_of=lambda h: table[h][0],
            title_of=lambda h: table[h][1],
            rect_of=lambda h: table[h][2],
            grab=lambda box, path: calls.append((box, path)),
        )
        result = windows_owned_window_screenshot(100, out, **api)
        return result, calls

    def test_prefers_own_popup_by_fixed_title(self):
        table = {1: (100, 'NexusBro', (0, 0, 900, 700)),
                 2: (100, 'NexusBro — меню', (50, 60, 250, 360)),
                 3: (999, 'NexusBro — меню', (0, 0, 100, 100))}
        result, calls = self.run_shot(table)
        self.assertTrue(result['saved'])
        self.assertEqual(result['scope'], 'own_popup')
        self.assertEqual(calls[0][0], (50, 60, 250, 360))

    def test_falls_back_to_own_main_never_foreign(self):
        table = {1: (100, 'NexusBro', (10, 10, 900, 700)),
                 3: (999, 'NexusBro — меню', (0, 0, 100, 100))}
        result, calls = self.run_shot(table)
        self.assertTrue(result['saved'])
        self.assertEqual(result['scope'], 'own_main')

    def test_no_own_window_is_typed_failure(self):
        result, calls = self.run_shot({3: (999, 'x', (0, 0, 10, 10))})
        self.assertEqual(result, {'saved': False, 'reason': 'owned_window_missing'})
        self.assertEqual(calls, [])

    def test_empty_rect_refused(self):
        result, _ = self.run_shot({1: (100, 'NexusBro — меню', (5, 5, 5, 50))})
        self.assertEqual(result, {'saved': False, 'reason': 'geometry_unavailable'})

    def test_bbox_bounded(self):
        table = {1: (100, 'NexusBro — меню', (0, 0, 5000, 4000))}
        result, calls = self.run_shot(table)
        self.assertTrue(result['saved'])
        self.assertEqual(calls[0][0], (0, 0, 1600, 1000))

    def test_grab_error_is_typed(self):
        table = {1: (100, 'NexusBro — меню', (0, 0, 100, 100))}
        def enum_top(collect):
            collect(1)
        api = dict(enum_top=enum_top, pid_of=lambda h: 100,
                   title_of=lambda h: 'NexusBro — меню',
                   rect_of=lambda h: (0, 0, 100, 100),
                   grab=lambda box, path: (_ for _ in ()).throw(OSError('no screen')))
        result = windows_owned_window_screenshot(100, '/tmp', **api)
        self.assertEqual(result, {'saved': False, 'reason': 'OSError'})


def main():
    suite = unittest.TestLoader().loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == '__main__':
    sys.exit(main())
