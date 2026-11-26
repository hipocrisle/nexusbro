#!/usr/bin/env python3
"""Trusted page-input helpers for the downloads native test (Linux, no JS).

The download gesture must be a REAL OS click on the fixture page's own anchor
inside the owned CEF region — shell DOM clicks and CEF scripts are not a user
gesture. Pure geometry/color/mapping helpers here are offline-testable; the
native click re-verifies process ownership and the actual painted pixel.
"""
import json
import subprocess
import time

PAGE_BG = (0x12, 0x34, 0x56)
TILES = {
    'doc': {'left': 40, 'top': 40, 'width': 200, 'height': 80, 'rgb': (0xC0, 0x20, 0x20)},
    'known': {'left': 40, 'top': 160, 'width': 200, 'height': 80, 'rgb': (0x20, 0xC0, 0x20)},
    'unknown': {'left': 40, 'top': 280, 'width': 200, 'height': 80, 'rgb': (0x20, 0x20, 0xC0)},
}
KNOWN_URL_KINDS = ('page', 'doc', 'known', 'unknown')


def tile_center(kind):
    """Fixed CSS center of a fixture tile."""
    tile = TILES[kind]
    return tile['left'] + tile['width'] // 2, tile['top'] + tile['height'] // 2


def screen_point(origin, pole, tile_css, scale):
    """Pure math: page-CSS point -> screen pixel (same rounding as Client.mouse)."""
    x = round(origin['x'] + (pole['x'] + tile_css[0]) * scale)
    y = round(origin['y'] + (pole['y'] + tile_css[1]) * scale)
    return x, y


def color_matches(pixel, expected, tolerance=24):
    """Pure: RGB within tolerance per channel."""
    return all(abs(int(a) - int(b)) <= tolerance for a, b in zip(pixel[:3], expected))


def url_kind(adres, origin):
    """Map a tab URL to one of the four known fixture kinds; never returns raw."""
    if not isinstance(adres, str) or not isinstance(origin, str):
        return 'other'
    for kind, path in (('page', '/page.html'), ('doc', '/note.txt'),
                       ('known', '/file-known.bin'), ('unknown', '/file-unknown.bin')):
        if adres == origin + path:
            return kind
    return 'other'


def tab_facts(session, tab, origin):
    """Typed tab facts: booleans + url_kind only, no raw URL/path."""
    row = next((v for v in session.get('vkladki', []) if v.get('id') == tab), None) or {}
    return {
        'browser_positive': type(row.get('browser_id')) is int and row.get('browser_id', 0) > 0,
        'zagruzka': row.get('zagruzka') is True,
        'route_pending': row.get('route_pending') is True,
        'route_error_present': bool(row.get('route_error')),
        'url_kind': url_kind(row.get('adres'), origin),
    }


def painted_tile(device, client, pole, kind, tolerance=24):
    """Real OS paint proof: the exact screen pixel of the tile center has the
    tile's unique color. Read-only grab inside the owned display."""
    from PIL import ImageGrab
    label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    from security_native import call
    origin = call(client, 'plugin:window|inner_position', label=label)
    scale = client.js('devicePixelRatio||1') or 1
    point = screen_point(origin, pole, tile_center(kind), scale)
    with ImageGrab.grab(bbox=(point[0], point[1], point[0] + 1, point[1] + 1),
                        xdisplay=device.display) as image:
        pixel = image.convert('RGB').getpixel((0, 0))
    return color_matches(pixel, TILES[kind]['rgb'], tolerance), point


def trusted_tile_click(device, client, pole, kind, identity_check):
    """ONE real OS click on the painted tile after ownership verification.
    identity_check() must return True (owned window/process alive); no retries
    of the click itself, no JS events."""
    import os
    painted, point = painted_tile(device, client, pole, kind)
    if not painted:
        return None, point
    if not identity_check():
        return None, point
    env = {**os.environ, 'DISPLAY': device.display}
    subprocess.run(['xdotool', 'mousemove', str(point[0]), str(point[1]), 'click', '1'],
                   env=env, check=True, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, timeout=3)
    return point, point


def wait_counter(fixture, kind, minimum, timeout, sleeper=time.sleep, clock=time.monotonic):
    """Wait until the fixture's own HTTP counter for kind reaches minimum.
    Returns the observed count or None on timeout (never raises for polling)."""
    deadline = clock() + timeout
    while clock() < deadline:
        if fixture.counts().get(kind, 0) >= minimum:
            return fixture.counts()[kind]
        sleeper(0.1)
    return None
