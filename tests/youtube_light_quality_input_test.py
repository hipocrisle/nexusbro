#!/usr/bin/env python3
"""Offline fake-IO tests for youtube_light_quality_native.prepare_light_av1."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[0] / 'gui'))
from youtube_light_quality_native import prepare_light_av1, LightQualityBlocked


def state(*, active=1, count=5, index=3, width=1280, height=720,
          playing=True, position=10.0, decoded=100):
    return {'active_control': active, 'quality_count': count,
            'quality_index': index, 'width': width, 'height': height,
            'playing': playing, 'position': position, 'decoded': decoded}


class Script:
    def __init__(self, steps):
        self.steps = steps
        self.index = 0

    def snapshot(self):
        value = self.steps[min(self.index, len(self.steps) - 1)]
        self.index += 1
        return value


class Clock:
    def __init__(self):
        self.value = 0.0

    def now(self):
        return self.value

    def sleep(self, seconds):
        self.value += seconds


def run(steps, binding=None):
    keys = []
    clock = Clock()
    result = {}
    script = Script(steps)
    final = prepare_light_av1(script.snapshot, keys.append,
                              binding or (lambda: True), result,
                              clock=clock.now, sleep=clock.sleep)
    return final, result['light_av1_input'], keys, clock


class LightAv1InputTest(unittest.TestCase):
    HAPPY = [state(), state(active=2), state(active=3), state(active=3, index=4),
             state(active=3, index=4, position=11.0, decoded=6)]

    def test_happy_path_exactly_three_keys(self):
        final, receipt, keys, _ = run(list(self.HAPPY))
        self.assertEqual(keys, ['Tab', 'Tab', 'Down'])
        self.assertEqual(receipt['ledger'], ['Tab', 'Tab', 'Down'])
        self.assertEqual(receipt['keys_sent'], 3)
        self.assertEqual(receipt['quality_count'], 5)
        self.assertEqual(receipt['default_index'], 3)
        self.assertEqual(receipt['final_index'], 4)
        self.assertEqual(final['quality_index'], 4)

    def test_binding_failure_before_first_key_sends_nothing(self):
        with self.assertRaisesRegex(LightQualityBlocked, 'binding_unproven_before_Tab'):
            run(list(self.HAPPY), binding=lambda: False)

    def test_wrong_default_index_sends_zero_keys(self):
        steps = [state(index=2)] + list(self.HAPPY)
        with self.assertRaisesRegex(LightQualityBlocked, 'default_not_predeclared_position'):
            run(steps)

    def test_first_tab_without_focus_change_stops_before_more_keys(self):
        steps = [state(), state(active=1)] + list(self.HAPPY)
        keys = []
        clock = Clock()
        with self.assertRaisesRegex(LightQualityBlocked, 'focus_not_on_quality_after_first_tab'):
            prepare_light_av1(Script(steps).snapshot, keys.append,
                              lambda: True, {}, clock=clock.now, sleep=clock.sleep)
        self.assertEqual(keys, ['Tab'])

    def test_timeout_sends_no_extra_keys(self):
        stuck = state(active=3, index=4, playing=False)
        steps = [state(), state(active=2), state(active=3), state(active=3, index=4)] + [stuck] * 100
        keys = []
        clock = Clock()
        with self.assertRaisesRegex(LightQualityBlocked, 'av1_switch_not_confirmed'):
            prepare_light_av1(Script(steps).snapshot, keys.append,
                              lambda: True, {}, clock=clock.now, sleep=clock.sleep)
        self.assertEqual(keys, ['Tab', 'Tab', 'Down'])
        self.assertGreaterEqual(clock.value, 20.0)

    def test_changed_count_after_down_fails_closed(self):
        steps = [state(), state(active=2), state(active=3),
                 state(active=3, index=4, count=6)]
        keys = []
        with self.assertRaisesRegex(LightQualityBlocked, 'quality_count_changed'):
            prepare_light_av1(Script(steps).snapshot, keys.append,
                              lambda: True, {}, clock=Clock().now, sleep=Clock().sleep)
        self.assertEqual(keys, ['Tab', 'Tab', 'Down'])

    def test_invalid_snapshot_is_typed_failure_without_input(self):
        bad = state()
        bad['position'] = float('nan')
        keys = []
        with self.assertRaisesRegex(LightQualityBlocked, 'state_invalid_preconditions'):
            prepare_light_av1(Script([bad]).snapshot, keys.append,
                              lambda: True, {}, clock=Clock().now, sleep=Clock().sleep)
        self.assertEqual(keys, [])

    def test_not_playing_or_wrong_geometry_refuse_before_input(self):
        for bad in (state(playing=False), state(width=640), state(count=1),
                    state(count=25), state(index=5)):
            keys = []
            with self.assertRaises(LightQualityBlocked):
                prepare_light_av1(Script([bad]).snapshot, keys.append,
                                  lambda: True, {}, clock=Clock().now,
                                  sleep=Clock().sleep)
            self.assertEqual(keys, [], bad)


def main():
    suite = unittest.TestLoader().loadTestsFromTestCase(LightAv1InputTest)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == '__main__':
    sys.exit(main())
