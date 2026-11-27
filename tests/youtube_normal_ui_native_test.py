#!/usr/bin/env python3
"""Offline controller tests for youtube_normal_ui_native (fake IO, no GUI)."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tests' / 'gui'))
import youtube_normal_ui_native as m


def rect(x, y, w=48, h=48):
    return {'x': x, 'y': y, 'width': w, 'height': h}


def state(*, playing=False, paused=True, position=10.0, vw=1280, vh=720,
          reject_present=False, reject=None, play=None, settings=None,
          quality=None, p720=None, settings_menu=None, width=1280, height=720):
    return {'playing': playing, 'paused': paused, 'position': position,
            'video_width': width, 'video_height': height,
            'controls': {'viewport': {'width': vw, 'height': vh},
                         'reject_all_present': reject_present,
                         'play': play, 'settings': settings,
                         'quality': quality, 'p720': p720, 'settings_menu':settings_menu, 'reject_all': reject}}


class FakeIO:
    def __init__(self):
        self.actions = []
        self.clock = 0.0
    def pole(self):
        return {'x': 10, 'y': 200, 'w': 1280, 'h': 720}
    def geometry(self):
        return {'x': 100, 'y': 50}, 1.0
    def foreground(self):
        self.actions.append('foreground')
    def click_css(self, x, y):
        self.actions.append(('click', round(x), round(y)))
    def move_css(self, x, y):
        self.actions.append(('move', round(x), round(y)))
    def scroll_css(self, x, y, notches):
        self.actions.append(('scroll', round(x), round(y), notches))
    def monotonic(self):
        return self.clock
    def sleep(self, seconds):
        self.clock += seconds


class Script:
    """snapshot() отдаёт следующее состояние сценария (clamp на последнем)."""
    def __init__(self, io, steps):
        self.steps = steps
        self.index = 0
    def snapshot(self):
        state = self.steps[min(self.index, len(self.steps) - 1)]
        self.index += 1
        return state


def run(steps, binding=None, expected=(1280, 720)):
    io = FakeIO()
    script = Script(io, steps)
    result = {}
    state_out = m.prepare_normal_playback(None, None, script.snapshot,
                                          expected[0], expected[1], result,
                                          binding or (lambda: True), io=io)
    return state_out, result, io


def expect_blocked(steps, code, binding=None):
    try:
        run(steps, binding=binding)
    except m.UiBlocked as error:
        return str(error) == code
    return False


PLAY = rect(12, 640)
SETTINGS = rect(1100, 640)
QUALITY = rect(900, 300, 160, 32)
P720 = rect(900, 340, 160, 32)
REJECT = rect(560, 300, 120, 36)


def main():
    checks = {}
    playing = lambda **kw: state(playing=True, paused=False, **kw)
    base = state(settings=SETTINGS)                       # оболочка без playing
    p_play = playing(play=PLAY, settings=SETTINGS)        # playing + play rect
    paused_play = state(play=PLAY, settings=SETTINGS)     # paused + play rect
    with_quality = state(settings=SETTINGS, quality=QUALITY)
    with_p720 = state(settings=SETTINGS, quality=QUALITY, p720=P720)
    started = playing(play=PLAY, settings=SETTINGS, position=11.0)
    # Точный trace: phase1.current, phase2.current, reveal.current, click.current,
    # observe(paused), reveal.current, click.current, observe(quality),
    # click.current, observe(p720), click.current, phase4.current,
    # reveal.current, click.current, observe(started)
    happy = [base, p_play, p_play, p_play, paused_play,
             base, base, with_quality, with_quality, with_p720, with_p720,
             paused_play, paused_play, paused_play, started]

    final, result, io = run(happy)
    checks['happy_path'] = final['playing'] is True
    menu_only=state(settings_menu=rect(800,200,300,300))
    scrolled_final, scrolled_result, scrolled_io = run(happy[:7]+[menu_only]*22+[with_quality,with_quality]+happy[9:])
    checks['scrolled_quality_real_wheel_once'] = (scrolled_final['playing'] is True and scrolled_result['normal_ui_actions']['ledger'].count('quality_menu_scroll')==1
        and ('scroll', 950, 350, 4) in scrolled_io.actions)
    # Delayed submenu transition: the target appears while waiting -> NO wheel
    # against the stale pre-transition menu rect.
    late_final, late_result, late_io = run(happy[:7]+[menu_only,menu_only,with_quality,with_quality]+happy[9:])
    checks['delayed_quality_transition_no_scroll'] = (late_final['playing'] is True
        and late_result['normal_ui_actions']['menu_scrolls']==0
        and not any(a[0]=='scroll' for a in late_io.actions if isinstance(a,tuple)))
    p720_late_final, p720_late_result, _ = run(happy[:9]+[with_quality,with_p720]+happy[9:])
    checks['delayed_p720_transition_no_scroll'] = (p720_late_final['playing'] is True
        and p720_late_result['normal_ui_actions']['menu_scrolls']==0)
    # Menu vanished during the wait (page scrolled away): fail closed, no wheel.
    gone_io = FakeIO(); gone_script = Script(gone_io, happy[:7]+[menu_only]*21+[state()]+[menu_only]*10)
    try:
        m.prepare_normal_playback(None, None, gone_script.snapshot, 1280, 720, {}, lambda: True, io=gone_io)
        checks['menu_gone_fail_closed_no_wheel'] = False
    except m.UiBlocked as error:
        checks['menu_gone_fail_closed_no_wheel'] = (str(error)=='quality_menu_unavailable'
            and not any(a[0]=='scroll' for a in gone_io.actions if isinstance(a,tuple)))

    late = state(reject_present=True, reject=REJECT)
    late_final, late_result, _ = run(happy[:5] + [late, late, base] + happy[6:])
    checks['late_consent_handled_once'] = (
        late_final['playing'] is True
        and late_result['normal_ui_actions']['ledger'].count('reject_all_click') == 1
        and late_result['normal_ui_actions']['reject_all_clicked'] is True)

    receipt = result['normal_ui_actions']
    checks['ledger_order'] = receipt['ledger'] == ['pause_click', 'settings_click', 'quality_click',
        'p720_click', 'play_click', 'playback_started']
    checks['receipt_shape'] = set(receipt) == {'ledger', 'ui_actions', 'reject_all_clicked',
        'paused_once', 'quality_720_clicked', 'played_once', 'hover_reveals', 'scrolls', 'menu_scrolls'} \
        and receipt['ui_actions'] == 5 and receipt['scrolls'] == 0
    checks['receipt_no_raw'] = 'http' not in json.dumps(result) and 'url' not in json.dumps(result)
    checks['control_center_forwarded'] = ('click', 36, 664) in io.actions

    # Exercise the real adapter mapping, which the controller's fake IO cannot prove.
    class Client:
        def js(self, _):return {'x':10,'y':200,'w':1280,'h':720}
        def mouse(self,x,y):self.clicked=(x,y)
    client=Client();adapter=m.RealIO(client,None);adapter.click_css(36,664)
    checks['real_adapter_includes_pole_offset']=client.clicked==(46,864)
    transition=happy[:14]+[playing(play=PLAY,settings=SETTINGS,position=11,width=854,height=480),started]
    final,_,_=run(transition)
    checks['quality_transition_waits_for_actual_target']=final['video_height']==720

    # Hover-reveal: play скрыт автогашением -> один mousemove, клика для показа нет
    p_hidden = playing(play=None, settings=SETTINGS)
    hover = [base, p_hidden, p_hidden, p_play, p_play, paused_play] + happy[5:]
    final, result, io = run(hover)
    checks['hover_reveal_once'] = result['normal_ui_actions']['hover_reveals'] == 1 \
        and ('move', 640, 360) in io.actions
    checks['no_click_to_unhide'] = sum(1 for a in io.actions if a[0] == 'click') == 5

    # Reject-all: scroll при null -> rect -> один клик -> исчезновение
    reject_none = state(reject_present=True, reject=None, settings=SETTINGS)
    reject_rect = state(reject_present=True, reject=REJECT, settings=SETTINGS)
    reject = [reject_none, reject_rect, reject_rect, base] + happy[1:]
    final, result, io = run(reject)
    checks['reject_scroll_then_click'] = result['normal_ui_actions']['reject_all_clicked'] is True \
        and result['normal_ui_actions']['scrolls'] == 1 \
        and ('scroll', 960, 540, 8) in io.actions \
        and result['normal_ui_actions']['ledger'][:2] == ['reject_scroll', 'reject_all_click']

    # Негативы
    checks['reject_unreachable'] = expect_blocked([reject_none] * 30, 'reject_all_unreachable')
    checks['viewport_mismatch'] = expect_blocked([state(vw=1000, play=PLAY)], 'viewport_mismatch')
    checks['setup_too_late'] = expect_blocked(
        [base, state(position=29.5, play=PLAY)], 'setup_too_late')
    checks['binding_before_blocks'] = expect_blocked(
        happy, 'binding_unproven_before_pause_click', binding=lambda: False)
    calls = []
    def binding_after_fails():
        calls.append(1)
        return len(calls) < 2
    checks['binding_after_blocks'] = expect_blocked(
        happy, 'binding_unproven_after_pause_click', binding=binding_after_fails)
    checks['quality_missing'] = expect_blocked(
        [base, p_play, p_play, p_play, paused_play] + [base] * 30, 'quality_menu_unavailable')
    checks['resolution_mismatch'] = expect_blocked(
        happy[:14] + [playing(play=PLAY, settings=SETTINGS, position=11.0, width=854, height=480)],
        'resolution_mismatch')
    checks['controls_invalid'] = expect_blocked(
        [{'playing': False, 'controls': {'viewport': {'width': 1280}}}], 'controls_invalid')
    checks['snapshot_invalid'] = expect_blocked([None], 'snapshot_invalid')
    checks['p720_never_offered_no_fallback'] = expect_blocked(
        happy[:9] + [with_quality] * 30, 'p720_unavailable')
    checks['playback_stall_no_retry'] = expect_blocked(
        happy[:14] + [paused_play] * 90, 'playback_not_started')

    # Исходник: никаких JS-действий/seek/rate
    src = (Path(__file__).resolve().parents[1] / 'tests' / 'gui' / 'youtube_normal_ui_native.py').read_text()
    checks['no_js_actions'] = all(token not in src for token in
        ('.click()', 'playVideo', 'seekTo', 'currentTime=', 'playbackRate=', 'dispatchEvent'))
    checks['budgets_fixed'] = m.MAX_WALL_SECONDS == 90 and m.MAX_UI_ACTIONS == 10 \
        and m.MAX_POSITION_BEFORE_PLAY == 29.0

    failed = [name for name, ok in checks.items() if not ok]
    print(json.dumps({'normal_ui_checks': 'PASS' if not failed else 'FAIL', 'failed': failed}))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
