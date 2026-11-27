#!/usr/bin/env python3
"""Physical normal-mode YouTube UI driver (test-only, no JS actions).

prepare_normal_playback() drives the REAL player UI with physical OS input
only: optional Reject-all, pause, Settings -> Quality -> 720p, Play — each
click exactly once per phase, <=10 UI actions, <=90s wall. Coordinates come
only from the native controls rects (CSS document coords) mapped through the
shell .pole rect + window origin + devicePixelRatio; the controls viewport
must match the .pole size within 2 CSS px or the driver refuses.

binding_check() (held super/tab/browser/route generation + native PID/birth,
owned by ROOT) must return exactly True before AND after every OS action;
anything else is a typed UiBlocked. No JS click/play/focus/quality/scroll, no
devtools/debugger, no GHA spoof, no direct HTTP/cookies/seek/rate changes.
The helper never claims the UI label equals the actual resolution — ROOT
verifies real frames after start.
"""
import json
import os
import subprocess
import time
from pathlib import Path

MAX_WALL_SECONDS = 90
MAX_UI_ACTIONS = 10
MAX_POSITION_BEFORE_PLAY = 29.0
VIEWPORT_TOLERANCE = 2.0
OBSERVE_STEP = 0.25

CONTROL_KEYS = {'viewport', 'reject_all_present', 'play', 'settings', 'settings_menu', 'quality', 'p720', 'reject_all'}


class UiBlocked(Exception):
    """Typed refusal with a fixed code; never raw coordinates/urls."""


def _rect(value):
    if value is None:
        return None
    if not isinstance(value, dict) or set(value) != {'x', 'y', 'width', 'height'}:
        raise UiBlocked('controls_invalid')
    numbers = [value[k] for k in ('x', 'y', 'width', 'height')]
    if any(type(n) not in (int, float) or n != n for n in numbers):
        raise UiBlocked('controls_invalid')
    if value['width'] <= 0 or value['height'] <= 0 or value['x'] < 0 or value['y'] < 0:
        raise UiBlocked('controls_invalid')
    return value


def validate_controls(state):
    """Strict fixed-key validation of the native controls object."""
    if not isinstance(state, dict):
        raise UiBlocked('snapshot_invalid')
    controls = state.get('controls')
    if not isinstance(controls, dict) or set(controls) != CONTROL_KEYS:
        raise UiBlocked('controls_invalid')
    viewport = controls['viewport']
    if not isinstance(viewport, dict) or set(viewport) != {'width', 'height'}:
        raise UiBlocked('controls_invalid')
    for dim in ('width', 'height'):
        n = viewport[dim]
        if type(n) not in (int, float) or n != n or not 1 <= n <= 16384:
            raise UiBlocked('controls_invalid')
    if type(controls['reject_all_present']) is not bool:
        raise UiBlocked('controls_invalid')
    for key in ('play', 'settings', 'settings_menu', 'quality', 'p720', 'reject_all'):
        rect = _rect(controls[key])
        if rect is not None and (rect['x'] + rect['width'] > viewport['width']
                                 or rect['y'] + rect['height'] > viewport['height']):
            raise UiBlocked('controls_invalid')
    return controls


def center(rect):
    return rect['x'] + rect['width'] / 2, rect['y'] + rect['height'] / 2


def viewport_matches(controls, pole):
    return (abs(controls['viewport']['width'] - pole['w']) <= VIEWPORT_TOLERANCE
            and abs(controls['viewport']['height'] - pole['h']) <= VIEWPORT_TOLERANCE)


class RealIO:
    """Physical OS input against the owned window; coordinates are mapped from
    CSS through .pole + inner_position + devicePixelRatio."""

    def __init__(self, client, device):
        self.client = client
        self.device = device

    def pole(self):
        value = self.client.js('(()=>{const e=document.querySelector(".pole");if(!e)return null;'
                               'const r=e.getBoundingClientRect();'
                               'return r.width>0&&r.height>0?{x:r.x,y:r.y,w:r.width,h:r.height}:null})()')
        if not isinstance(value, dict):
            raise UiBlocked('pole_unavailable')
        return value

    def geometry(self):
        from security_native import call
        label = self.client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        origin = call(self.client, 'plugin:window|inner_position', label=label)
        scale = self.client.js('devicePixelRatio||1') or 1
        return origin, scale

    def foreground(self):
        from youtube_player_native import foreground
        foreground(self.device)

    def _screen(self, css_x, css_y):
        pole = self.pole()
        origin, scale = self.geometry()
        return (round(origin['x'] + (pole['x'] + css_x) * scale),
                round(origin['y'] + (pole['y'] + css_y) * scale))

    def click_css(self, css_x, css_y):
        pole = self.pole()
        self.client.mouse(pole['x'] + css_x, pole['y'] + css_y)

    def move_css(self, css_x, css_y):
        x, y = self._screen(css_x, css_y)
        env = {**os.environ, 'DISPLAY': self.device.display}
        subprocess.run(['xdotool', 'mousemove', str(x), str(y)], env=env, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)

    def scroll_css(self, css_x, css_y, notches):
        x, y = self._screen(css_x, css_y)
        env = {**os.environ, 'DISPLAY': self.device.display}
        subprocess.run(['xdotool', 'mousemove', str(x), str(y)] +
                       ['click', '5'] * int(notches), env=env, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)

    def screenshot(self, path):
        """OS screenshot of the fixture display; True only when a file was written."""
        try:
            from PIL import ImageGrab
            with ImageGrab.grab(xdisplay=self.device.display) as shot:
                shot.save(path)
            return True
        except Exception:
            return False

    def monotonic(self):
        return time.monotonic()

    def sleep(self, seconds):
        time.sleep(seconds)


def prepare_normal_playback(client, device, snapshot, expected_width, expected_height,
                            result, binding_check, io=None, quality_diagnostic_dir=None):
    """Physical preparation of normal playback. Returns the final native state.
    snapshot()/binding_check() are ROOT-held closures; io is injectable for
    offline controller tests (defaults to RealIO)."""
    io = io or RealIO(client, device)
    ledger = []
    receipt = {'ledger': ledger, 'ui_actions': 0, 'reject_all_clicked': False,
               'paused_once': False, 'quality_720_clicked': False, 'played_once': False,
               'hover_reveals': 0, 'scrolls': 0, 'menu_scrolls': 0}
    result['normal_ui_actions'] = receipt
    started = io.monotonic()

    def deadline_left():
        left = MAX_WALL_SECONDS - (io.monotonic() - started)
        if left <= 0:
            raise UiBlocked('ui_budget_exceeded')
        return left

    def checked(action):
        deadline_left()
        if binding_check() is not True:
            raise UiBlocked('binding_unproven_before_' + action)
        io.foreground()
        yield_io = None
        return yield_io

    def after(action):
        deadline_left()
        if binding_check() is not True:
            raise UiBlocked('binding_unproven_after_' + action)

    def spend():
        receipt['ui_actions'] += 1
        if receipt['ui_actions'] > MAX_UI_ACTIONS:
            raise UiBlocked('ui_actions_exceeded')

    def observe(predicate, code, budget):
        deadline = io.monotonic() + min(budget, deadline_left())
        while io.monotonic() < deadline:
            state = snapshot()
            deadline_left()
            validate_controls(state)
            value = predicate(state)
            if value:
                return state
            io.sleep(OBSERVE_STEP)
        raise UiBlocked(code)

    def settled(predicate, budget):
        """Bounded observation returning None on timeout instead of raising."""
        deadline = io.monotonic() + min(budget, deadline_left())
        while io.monotonic() < deadline:
            state = snapshot()
            deadline_left()
            validate_controls(state)
            value = predicate(state)
            if value:
                return state
            io.sleep(OBSERVE_STEP)
        return None

    def current():
        state = snapshot()
        validate_controls(state)
        if not viewport_matches(state['controls'], io.pole()):
            raise UiBlocked('viewport_mismatch')
        return state

    def reveal(rect_key):
        """Controls may be autohidden: one real mousemove over the CEF center,
        then bounded observation; never a click just to unhide."""
        state = consent(current())
        if state['controls'][rect_key] is not None:
            return state
        pole = io.pole()
        checked('hover')
        spend()
        io.move_css(pole['w'] / 2, pole['h'] / 2)
        after('hover')
        receipt['hover_reveals'] += 1
        ledger.append('hover_reveal')
        def revealed(s):
            s = consent(s)
            return s if s['controls'][rect_key] is not None else None
        return observe(revealed, rect_key + '_unavailable', 5)

    def click(rect_key, ledger_name):
        state = current()
        if rect_key != 'reject_all':
            state = consent(state)
        rect = state['controls'][rect_key]
        if rect is None:
            raise UiBlocked(rect_key + '_not_actionable')
        cx, cy = center(rect)
        checked(ledger_name)
        spend()
        io.click_css(cx, cy)
        after(ledger_name)
        ledger.append(ledger_name)

    def consent(state):
        # The real consent modal can arrive after the first player snapshot.
        # React to its known native marker once; never click through an overlay.
        if not state['controls']['reject_all_present']:
            return state
        if receipt['reject_all_clicked']:
            raise UiBlocked('reject_all_reappeared')
        if state['controls']['reject_all'] is None:
            if receipt['scrolls']:
                raise UiBlocked('reject_all_unreachable')
            pole = io.pole()
            checked('reject_scroll')
            spend()
            io.scroll_css(pole['w'] * 0.75, pole['h'] * 0.75, 8)
            after('reject_scroll')
            receipt['scrolls'] += 1
            ledger.append('reject_scroll')
            observe(lambda s: s if s['controls']['reject_all'] is not None else None,
                    'reject_all_unreachable', 5)
        click('reject_all', 'reject_all_click')
        receipt['reject_all_clicked'] = True
        return observe(lambda s: s if s['controls']['reject_all_present'] is False else None,
                       'reject_all_still_present', 5)

    # 1) Known consent only, including late arrival during control revelation.
    consent(current())

    # 2) Pause при playing; позиция до старта не позже 29с.
    state = current()
    if state.get('position', 0) > MAX_POSITION_BEFORE_PLAY:
        raise UiBlocked('setup_too_late')
    if state.get('playing') is True:
        state = reveal('play')
        click('play', 'pause_click')
        state = observe(lambda s: s if s.get('paused') is True else None,
                        'pause_not_observed', 5)
        receipt['paused_once'] = True

    def menu_item(key, code):
        state=current()
        if state['controls'][key] is not None:return state
        # The parent click rebuilds the menu through a real transition: always
        # wait (bounded) for the target item itself before any wheel, so the
        # scroll never fires against a stale pre-transition rect. A wheel over
        # a closed or non-overflowing menu scrolls the whole page instead: the
        # player leaves the viewport and every control rect becomes null.
        state=settled(lambda s:s if s['controls'][key] is not None else None,5)
        if state is not None:return state
        state=current()
        scroll=state['controls']['settings_menu']
        if scroll is None or receipt['menu_scrolls']>=2:raise UiBlocked(code)
        checked(key+'_menu_scroll');spend()
        io.scroll_css(*center(scroll),4)
        after(key+'_menu_scroll')
        receipt['menu_scrolls']+=1
        ledger.append(key+'_menu_scroll')
        return observe(lambda s:s if s['controls'][key] is not None else None,code,5)

    # 3) Settings -> Quality -> 720p, без fallback-качества.
    state = reveal('settings')
    click('settings', 'settings_click')
    menu_item('quality','quality_menu_unavailable')
    click('quality', 'quality_click')
    if quality_diagnostic_dir is not None:
        # Bounded read-only stop requested by the owner: NO wheel and no
        # further clicks. Capture how the real submenu looks to the probe
        # (geometry facts only, no page data) plus one OS screenshot, then
        # stop; the caller records completed=false with this evidence.
        frames = []
        for _ in range(6):
            state = snapshot()
            validate_controls(state)
            controls = state['controls']
            frames.append({'viewport': controls['viewport'],
                           'settings_menu': controls['settings_menu'],
                           'quality': controls['quality'],
                           'p720': controls['p720'],
                           'playing': state.get('playing'),
                           'position': state.get('position')})
            io.sleep(0.5)
        shot_path = str(Path(quality_diagnostic_dir) / 'quality-submenu.png')
        captured = io.screenshot(shot_path)
        result['quality_diagnostic'] = {
            'frames': frames,
            'screenshot_written': captured,
            'screenshot': shot_path if captured else None,
            'stopped_before': 'p720_wheel_or_click',
        }
        raise UiBlocked('quality_submenu_diagnostic')
    menu_item('p720','p720_unavailable')
    click('p720', 'p720_click')
    receipt['quality_720_clicked'] = True

    # 4) Play и доказательство фактического воспроизведения ожидаемого качества.
    state = current()
    if state.get('position', 0) > MAX_POSITION_BEFORE_PLAY:
        raise UiBlocked('setup_too_late')
    if state.get('playing') is not True:
        state = reveal('play')
        click('play', 'play_click')
        receipt['played_once'] = True
    last_state = {}
    def started_playback(s):
        last_state.clear()
        last_state.update(s)
        if (s.get('playing') is True and (s.get('position') or 0) > 0
                and s.get('video_width') == expected_width
                and s.get('video_height') == expected_height):
            return s
        return None
    try:
        final = observe(started_playback, 'playback_not_started', 20)
    except UiBlocked as error:
        if str(error) == 'playback_not_started' and (
                last_state.get('video_width') not in (0, expected_width)
                or last_state.get('video_height') not in (0, expected_height)):
            raise UiBlocked('resolution_mismatch') from None
        raise
    ledger.append('playback_started')
    return final
