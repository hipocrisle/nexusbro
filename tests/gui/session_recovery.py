"""GitHub-only Linux recovery acceptance against the actual packaged application.

No IPC/CEF mocks: preseeded committed sessions, physical tab click, abrupt owned
process-tree kill, actual restart and explicit-close/graceful-exit behavior.
All URLs and browsing data belong to one loopback fixture.
"""
import argparse
import http.server
import json
import os
import pathlib
import signal
import socket
import subprocess
import threading
import time
import urllib.parse

import psutil
from websocket import WebSocketException
from client import Client


def wait(check, message, timeout=30):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            last = check()
            if last:
                return last
        except (ConnectionError, OSError, RuntimeError, AssertionError, WebSocketException):
            pass
        time.sleep(.1)
    raise AssertionError((message, last))


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def run(exe, cef, out, display=':97'):
    assert os.environ.get('GITHUB_ACTIONS') == 'true', 'Native recovery must run on a disposable CI runner'
    out.mkdir(parents=True, exist_ok=False)
    data = out / 'data'
    data.mkdir()
    hits = []
    checks = []

    class Fixture(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            path = urllib.parse.urlsplit(self.path).path
            hits.append(path)
            title = {'/one': 'RecoveredOne', '/two': 'RecoveredTwo', '/cold-main': 'ColdMain', '/blocked': 'MustNeverLoad'}.get(path, 'Other')
            body = ('<!doctype html><title>' + title + '</title><style>body{background:#185541}</style><h1>' + title + '</h1>').encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{server.server_port}'
    inspector = free_port()
    env = {**os.environ, 'DISPLAY': display, 'CEF_PATH': str(cef), 'LD_LIBRARY_PATH': str(cef),
           'NEXUSBRO_TEST_DATA': str(data), 'NEXUSBRO_PROBA': '0',
           'WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS': '1',
           'WEBKIT_INSPECTOR_HTTP_SERVER': f'127.0.0.1:{inspector}'}
    main_id = 'recovery-main'

    def tab(ident, path, title, route=None):
        return {'id': ident, 'adres': path if path == 'about:blank' else base + path,
                'zagolovok': title, 'route': route or {'kind': 'inherit'}}

    def session(tabs, active):
        return {'vkladki': tabs, 'aktivnaya': active, 'sleduyushchaya': 999, 'zakrytye': []}

    def write(path, obj):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(obj))

    definitions = [{'id': main_id, 'nazvanie': 'Recovery', 'proxy': '', 'adres': 'about:blank', 'rezat_reklamu': False, 'pustaya': False},
                   {'id': 'legacy-private', 'nazvanie': 'Private', 'proxy': '', 'adres': 'about:blank', 'rezat_reklamu': False, 'pustaya': False, 'privatnaya': True}]
    write(data / 'vkladki.json', definitions)
    write(data / 'aktivnaya.json', main_id)
    write(data / 'nastrojki.json', {'avtoobnovleniya': False})
    profile = data / 'profili' / main_id
    write(profile / 'obychnye.backup.json', session([tab(1, 'about:blank', 'Blank'), tab(2, '/cold-main', 'ColdMain')], 1))
    (profile / 'obychnye.json').write_text('{truncated')
    tabs = [tab(201, '/one', 'RecoveryOne'), tab(202, '/two', 'RecoveryTwo', {'kind': 'direct'}),
            tab(203, '/blocked', 'Blocked', {'kind': 'connection', 'id': 'removed-connection'})]
    windows = [{'key': 'recovery-window', 'active_super': main_id, 'views': [{'super_id': main_id, 'session': session(tabs, 201)}]},
               {'key': 'private-window', 'active_super': 'legacy-private', 'views': [{'super_id': 'legacy-private', 'session': session([tab(1, '/private', 'Private')], 1)}]}]
    write(data / 'windows-recovery.json', {'schema': 1, 'windows': windows})
    app = None
    clients = {}
    x = None
    launches = 0
    expected_windows = 2

    def discover():
        nonlocal clients
        found = {}
        for index in range(5):
            try:
                c = Client(display=display, port=inspector, page_index=index)
                label = c.js('window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')
                if label == 'main' or (isinstance(label, str) and label.startswith('browser-')):
                    found[label] = c
                else:
                    c.close()
            except (ConnectionError, OSError, RuntimeError, WebSocketException):
                break
        if 'main' not in found or len(found) != expected_windows:
            for c in found.values():
                c.close()
            return False
        clients = found
        return True

    def start(expected=2):
        nonlocal app, launches, expected_windows
        expected_windows = expected
        launches += 1
        app = subprocess.Popen(['dbus-run-session', '--', str(exe)], env=env, start_new_session=True,
                               stdout=(out / f'app-{launches}.log').open('w'), stderr=subprocess.STDOUT)
        wait(discover, 'Main and one ordinary recovered window were not created', 45)
        main = clients['main']
        secondary = next((c for label, c in clients.items() if label != 'main'), None)
        main.wait('document.querySelectorAll(".obychnaya").length===2', timeout=40)
        if secondary:
            secondary.wait('document.querySelectorAll(".obychnaya").length===3', timeout=40)
            assert secondary.command('aktivnaya') == main_id
        assert main.command('aktivnaya') == main_id
        return main, secondary

    def stop(crash):
        nonlocal app, clients
        owned = psutil.Process(app.pid).children(recursive=True)
        if crash:
            # Kill the actual parent app first: no final UI/CEF callbacks may
            # turn this into a graceful-stop test. Everything killed is owned.
            parents = []
            for child in owned:
                try:
                    args = child.cmdline()
                    if '--nb-dvizhok' not in args and not any(a.startswith('--type=') for a in args) and pathlib.Path(child.exe()) == exe:
                        parents.append(child)
                except psutil.NoSuchProcess:
                    pass
            assert len(parents) == 1, 'Could not identify the isolated actual app process'
            parents[0].kill()
        else:
            try:
                clients['main'].js('window.__TAURI_INTERNALS__.invoke("plugin:process|exit",{code:0});true')
            except WebSocketException:
                pass
            app.wait(timeout=15)
            assert app.returncode == 0
        for c in clients.values():
            try:
                c.close()
            except Exception:
                pass
        clients = {}
        try:
            os.killpg(app.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        app.wait(timeout=10)
        for child in owned:
            try:
                child.kill()
            except psutil.NoSuchProcess:
                pass
        psutil.wait_procs(owned, timeout=5)
        app = None
        time.sleep(.4)

    try:
        x = subprocess.Popen(['Xvfb', display, '-screen', '0', '1280x900x24', '-nolisten', 'tcp'],
                             stdout=(out / 'xvfb.log').open('w'), stderr=subprocess.STDOUT)
        time.sleep(.4)
        main, secondary = start()
        time.sleep(2)
        assert not any(path in hits for path in ['/one', '/two', '/cold-main', '/blocked', '/private']), hits
        assert len(secondary.command('obychnye_spisok', id=main_id)['vkladki']) == 3
        checks.append({'corrupt_main_uses_backup_ordinary_windows_restored_private_excluded_and_no_cold_page_requests': True})
        secondary.command('fokus_obolochke')
        secondary.click('.obychnaya[data-id="202"]')
        live = wait(lambda: (lambda s: s if any(t['zagolovok'] == 'RecoveredTwo' and t['id'] == s['aktivnaya'] and not t['zagruzka'] for t in s['vkladki']) else None)(secondary.command('obychnye_spisok', id=main_id)), 'Physical click failed to wake exact restored row')
        chosen = next(t for t in live['vkladki'] if t['id'] == live['aktivnaya'])
        assert chosen['route'] == {'kind': 'direct'} and chosen['id'] != 202
        assert '/two' in hits and '/one' not in hits and '/cold-main' not in hits, hits
        checks.append({'physical_restored_tab_click_waits_native_ack_remaps_id_and_keeps_explicit_route': True})
        wait(lambda: any(w['key'] == 'recovery-window' and w['views'][0]['session']['aktivnaya'] == live['aktivnaya'] for w in json.loads((data / 'windows-recovery.json').read_text())['windows']), 'Acknowledged secondary session was not committed')
        stop(crash=True)
        before = len(hits)
        main, secondary = start()
        time.sleep(2)
        assert '/two' not in hits[before:], hits[before:]
        recovered = secondary.command('obychnye_spisok', id=main_id)
        assert len(recovered['vkladki']) == 3
        assert next(t for t in recovered['vkladki'] if t['id'] == recovered['aktivnaya'])['route'] == {'kind': 'direct'}
        checks.append({'actual_sigkill_restart_restores_independent_window_tabs_routes_lazily': True})
        # Application-wide quit keeps open windows, unlike explicit close.
        stop(crash=False)
        assert len(json.loads((data / 'windows-recovery.json').read_text())['windows']) == 1
        checks.append({'graceful_app_exit_retains_open_window_recovery_snapshot': True})
        main, secondary = start()
        label = secondary.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        try:
            secondary.command('plugin:window|close', label=label)
        except WebSocketException:
            pass
        wait(lambda: json.loads((data / 'windows-recovery.json').read_text())['windows'] == [], 'Explicit window close did not remove its recovery record')
        checks.append({'explicit_secondary_window_close_commits_removal': True})
        stop(crash=False)
        main, secondary = start(expected=1)
        assert secondary is None
        time.sleep(2)
        assert json.loads((data / 'windows-recovery.json').read_text())['windows'] == []
        checks.append({'closed_window_does_not_resurrect_on_next_start': True})
        stop(crash=False)
    finally:
        if app:
            try:
                stop(crash=True)
            except Exception:
                try:
                    os.killpg(app.pid, signal.SIGKILL)
                except (ProcessLookupError, AttributeError):
                    pass
        if x:
            x.terminate()
            x.wait(timeout=10)
        server.shutdown()
        (out / 'results.json').write_text(json.dumps({'completed': len(checks) == 6, 'checks': checks}, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True)
    parser.add_argument('--cef', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--display', default=':97')
    args = parser.parse_args()
    run(pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve(), pathlib.Path(args.out).resolve(), args.display)
