#!/usr/bin/env python3
"""One predeclared physical keyboard path to the AV1 quality option of the
Light player (single predeclared clip M7lc1UVf-VE, default VP9 720p is the
second-to-last entry, AV1 720p is the last one by policy sort(height,fps,mp4)).

No URL/DOM access, no codec guessing from labels, no extra keys/clicks/JS.
Root sends every OS key through its own owner-PID/birth-checked channel and
runs the FIRST native decode probe afterwards (AV1/Dav1d required there).
This helper itself claims neither codec nor paired success, and once any
expectation is violated it fails closed and never sends further input.
"""
import math
import time

MAX_QUALITY_COUNT = 24
OBSERVE_BUDGET_SECONDS = 20.0
OBSERVE_STEP_SECONDS = 0.5


class LightQualityBlocked(Exception):
    pass


def _require(condition, code):
    if not condition:
        raise LightQualityBlocked(code)


def _valid(state):
    if not isinstance(state, dict):
        return False
    for field in ('position', 'decoded'):
        value = state.get(field)
        if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
            return False
    for field in ('active_control', 'quality_count', 'quality_index'):
        if type(state.get(field)) is not int:
            return False
    return True


def prepare_light_av1(snapshot, send_key, binding_check, result, *,
                      clock=time.monotonic, sleep=time.sleep):
    """Exactly Tab, Tab, Down with binding proof around each key, then a
    bounded observation of the switched playback. Returns the final state."""
    ledger = []
    receipt = {'ledger': ledger, 'keys_sent': 0}
    result['light_av1_input'] = receipt

    def bound(action):
        if binding_check() is not True:
            raise LightQualityBlocked('binding_unproven_' + action)

    def take(tag):
        state = snapshot()
        _require(_valid(state), 'state_invalid_' + tag)
        return state

    state = take('preconditions')
    count = state['quality_count']
    _require(2 <= count <= MAX_QUALITY_COUNT, 'quality_count_out_of_range')
    index = state['quality_index']
    _require(0 <= index <= count - 2, 'quality_index_out_of_range')
    _require(index == count - 2, 'default_not_predeclared_position')
    _require(state['active_control'] == 1, 'unexpected_focus_before_start')
    _require(state.get('video_width') == 1280 and state.get('video_height') == 720,
             'unexpected_geometry')
    _require(state.get('playing') is True, 'not_playing_before_switch')
    receipt['quality_count'] = count
    receipt['default_index'] = index

    def key(name):
        bound('before_' + name)
        send_key(name)
        receipt['keys_sent'] += 1
        ledger.append(name)
        bound('after_' + name)
        return take('after_' + name)

    state = key('Tab')
    _require(state['active_control'] == 2, 'focus_not_on_seek_after_first_tab')
    state = key('Tab')
    _require(state['active_control'] == 3, 'focus_not_on_quality_after_second_tab')
    state = key('Down')
    _require(state['quality_count'] == count, 'quality_count_changed')
    _require(state['quality_index'] == index + 1, 'quality_index_not_advanced')

    deadline = clock() + OBSERVE_BUDGET_SECONDS
    while True:
        bound('before_observe')
        state = take('observe')
        bound('after_observe')
        if clock()>deadline:raise LightQualityBlocked('av1_switch_not_confirmed')
        _require(state['quality_count'] == count, 'quality_count_changed')
        # The native src switch may reset the decoded counter, so the check is
        # an absolute floor, never a comparison with the pre-switch counter.
        if (state.get('playing') is True and state['position'] > 0
                and state['decoded'] >= 6
                and state.get('video_width') == 1280 and state.get('video_height') == 720
                and state['quality_index'] == count - 1
                and state['active_control'] == 3):
            receipt['final_index'] = state['quality_index']
            receipt['final_decoded'] = int(state['decoded'])
            receipt['observe_seconds'] = round(
                OBSERVE_BUDGET_SECONDS - max(0.0, deadline - clock()), 3)
            return state
        if clock() >= deadline:
            raise LightQualityBlocked('av1_switch_not_confirmed')
        sleep(OBSERVE_STEP_SECONDS)
