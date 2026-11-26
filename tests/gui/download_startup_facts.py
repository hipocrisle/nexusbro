#!/usr/bin/env python3
"""Sanitized startup facts for the downloads native test (read-only parsing).

Collects ONLY allowlisted native bootstrap/document facts from the app log
(app-N.log in the device root) and the engine log (profil/cef.log per
cef_dvizhok.rs:272) before cleanup. Never exports URLs, paths, nonces or raw
lines — only fixed stages, booleans and bounded numbers. Absence of logs or
events is reported explicitly (available/readable/lines_seen/no_events),
never silently treated as pass.
"""
import hashlib
import re
from pathlib import Path

STAGES = ('observer_ready', 'deadline', 'command_rejected', 'timer_rejected',
          'host_missing', 'owned_blank')
MAX_LINES = 4000
MAX_EVENTS = 64

_BOOTSTRAP = re.compile(r'NEXUSBRO_MEMORY_BOOTSTRAP(?:\s+stage=([a-z_]+))?\s+browser=(\d+)'
                        r'(?:\s+registered=(true|false))?(?:\s+safety=(early|keep))?')
_DOCUMENT = re.compile(r'NEXUSBRO_MEMORY_DOCUMENT\s+browser=(\d+)\s+accepted=(true|false)')
_TERMINATED = re.compile(r'cef: .*(renderer terminated|terminated)')
_ERROR_CODE = re.compile(r'\berror_code=(-?\d{1,6})\b')


def parse_native_facts(text):
    """Pure parser: allowlisted prefixes -> typed events; everything else is
    counted but never exported."""
    events = []
    lines_seen = 0
    for line in (text or '').splitlines():
        if lines_seen >= MAX_LINES:
            break
        lines_seen += 1
        event = None
        match = _BOOTSTRAP.search(line)
        if match:
            stage, browser, registered, _safety = match.groups()
            event = {'kind': 'bootstrap',
                     'stage': stage if stage in STAGES else ('finish' if stage is None else 'other'),
                     'browser_ref': int(browser) % 100000,
                     'registered': (registered == 'true') if registered else None}
        else:
            match = _DOCUMENT.search(line)
            if match:
                event = {'kind': 'document', 'browser_ref': int(match.group(1)) % 100000,
                         'accepted': match.group(2) == 'true'}
            elif _TERMINATED.search(line):
                event = {'kind': 'renderer_terminated'}
        if event is not None:
            code = _ERROR_CODE.search(line)
            if code:
                event['error_code'] = int(code.group(1))
            if len(events) < MAX_EVENTS:
                events.append(event)
    return {'lines_seen': lines_seen, 'events': events,
            'no_events': len(events) == 0}


def _read_log(path):
    result = {'available': False, 'readable': False, 'lines_seen': 0,
              'events': [], 'no_events': True}
    try:
        path = Path(path)
        if not path.is_file() or path.is_symlink():
            return result
        result['available'] = True
        raw = path.read_bytes()[:512 * 1024]
        result['readable'] = True
        facts = parse_native_facts(raw.decode('utf-8', errors='replace'))
        result.update(facts)
    except OSError:
        pass
    return result


def collect_startup_facts(device, identifier):
    """App log (последний app-N.log в device root) + engine profil/cef.log."""
    facts = {}
    try:
        root = Path(device.root)
        app_logs = sorted(root.glob('app-*.log'))
        facts['app'] = _read_log(app_logs[-1]) if app_logs else {
            'available': False, 'readable': False, 'lines_seen': 0, 'events': [], 'no_events': True}
    except Exception:
        facts['app'] = {'available': False, 'readable': False, 'lines_seen': 0,
                        'events': [], 'no_events': True, 'error': 'collect_failed'}
    try:
        cef_log = Path(device.data) / 'profili' / identifier / 'cef.log'
        facts['engine'] = _read_log(cef_log)
    except Exception:
        facts['engine'] = {'available': False, 'readable': False, 'lines_seen': 0,
                           'events': [], 'no_events': True, 'error': 'collect_failed'}
    return facts


def owned_failure_screenshot(device, out_dir, identity_check):
    """Скриншот ТОЛЬКО owned-окна своего Xvfb при отказе. Строгие проверки:
    identity (PID/birth) до и после, окно из native_app_window, bbox — геометрия
    этого окна на own display. Никакого owner desktop; второго инспектора нет."""
    receipt = {'saved': False, 'reason': 'not_attempted'}
    try:
        from PIL import ImageGrab
        from youtube_player_native import native_app_window
        import subprocess
        if not identity_check():
            return {'saved': False, 'reason': 'identity_unproven_before'}
        window = native_app_window(device, identity_check.identity)
        geometry = subprocess.check_output(
            ['xdotool', 'getwindowgeometry', '--shell', str(window)],
            env={'DISPLAY': device.display, 'PATH': '/usr/bin:/bin'},
            text=True, timeout=5)
        values = {}
        for line in geometry.splitlines():
            key, _, value = line.partition('=')
            if key in ('X', 'Y', 'WIDTH', 'HEIGHT') and value.lstrip('-').isdigit():
                values[key] = int(value)
        if set(values) != {'X', 'Y', 'WIDTH', 'HEIGHT'} or values['WIDTH'] <= 0 or values['HEIGHT'] <= 0:
            return {'saved': False, 'reason': 'geometry_unavailable'}
        box = (values['X'], values['Y'], values['X'] + min(values['WIDTH'], 1600),
               values['Y'] + min(values['HEIGHT'], 1000))
        if not identity_check():
            return {'saved': False, 'reason': 'identity_unproven_after'}
        target = Path(out_dir) / 'owned-failure.png'
        with ImageGrab.grab(bbox=box, xdisplay=device.display) as image:
            image.save(target, format='PNG')
        payload = target.read_bytes()
        if len(payload) > 3 * 1024 * 1024:
            target.unlink()
            return {'saved': False, 'reason': 'too_large'}
        return {'saved': True, 'width': box[2] - box[0], 'height': box[3] - box[1],
                'sha256': hashlib.sha256(payload).hexdigest()}
    except Exception:
        return {'saved': False, 'reason': 'capture_failed'}
