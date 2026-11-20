#!/usr/bin/env python3
"""Native per-tab route acceptance, on a disposable GitHub desktop only.

Requires a built app, CEF, and --tls-cert/--tls-key for 127.0.0.1 signed by a
CA already trusted by that disposable runner. No ignore-certificate flags,
production changes, simulated Tauri replies, or external network probes.
Unexecuted/missing prerequisites always leave completed=false and exit nonzero.
"""
import argparse
import json
import os
import pathlib
import socket
import subprocess
import sys
import time
import urllib.request
import urllib.parse

import psutil
import websocket
from client import Client
from route_fixture import Fixture
from stun_fixture import StunFixture
from security_native import Device, call, js_wait, port, require, settings, super_record, wait_for


class InspectorNoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        # An owned local inspector has no legitimate redirect destination.
        return None


class WindowsClient(Client):
    """WebView2 inspector reads shell IPC; all UI clicks are physical input."""
    def __init__(self, inspector, diagnostic=None):
        self.seq = 0
        diagnostic = diagnostic if diagnostic is not None else {}
        diagnostic['stage'] = 'http_target_list'
        # A Python/WinINET proxy must never receive an owned inspector request.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), InspectorNoRedirect())
        with opener.open('http://127.0.0.1:' + str(inspector) + '/json/list', timeout=2) as response:
            diagnostic['http_status'] = response.status
            pages = json.load(response)
        diagnostic['targets'] = [{'type': p.get('type'), 'title': str(p.get('title', ''))[:80],
                                  'url_scheme': urllib.parse.urlsplit(p.get('url', '')).scheme,
                                  'has_websocket': bool(p.get('webSocketDebuggerUrl'))} for p in pages[:10]]
        self.ws = None
        for page in pages:
            if page.get('type') != 'page' or not page.get('webSocketDebuggerUrl'):
                continue
            endpoint = urllib.parse.urlsplit(page['webSocketDebuggerUrl'])
            require(endpoint.scheme == 'ws' and endpoint.hostname in ('127.0.0.1', 'localhost')
                    and endpoint.port == inspector and not endpoint.username and not endpoint.password,
                    'Inspector advertised a WebSocket outside the owned loopback port')
            diagnostic['stage'] = 'websocket_connect'
            # websocket-client can fall back to proxy environment variables
            # even with http_proxy_host=None; its preconnected-socket path
            # bypasses proxy discovery entirely. Never resolve a target host.
            transport = socket.create_connection(('127.0.0.1', inspector), timeout=5)
            try:
                candidate = websocket.create_connection(page['webSocketDebuggerUrl'], socket=transport,
                    timeout=5, suppress_origin=True, http_proxy_host=None,
                    http_no_proxy=['127.0.0.1', 'localhost'])
            except Exception:
                transport.close()
                raise
            self.ws = candidate
            diagnostic['stage'] = 'shell_ipc_probe'
            try:
                if self.js('!!window.__TAURI_INTERNALS__?.invoke'):
                    diagnostic['stage'] = 'ready'
                    return
                diagnostic['shell_ipc_present'] = False
            except (OSError, RuntimeError, websocket.WebSocketException) as error:
                diagnostic['probe_error_type'] = type(error).__name__
            candidate.close()
            self.ws = None
        raise ConnectionError('No real Tauri WebView2 shell inspector')

    def js(self, expression):
        self.seq += 1
        sequence = self.seq
        self.ws.send(json.dumps({'id': sequence, 'method': 'Runtime.evaluate',
                                 'params': {'expression': expression, 'returnByValue': True}}))
        while True:
            reply = json.loads(self.ws.recv())
            if reply.get('id') != sequence:
                continue
            require(not reply.get('error') and not reply.get('result', {}).get('exceptionDetails'),
                    'Real WebView2 inspector evaluation failed')
            return reply.get('result', {}).get('result', {}).get('value')

    def mouse(self, x, y, button=1):
        import pyautogui
        label = self.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        origin = self.command('plugin:window|inner_position', label=label)
        scale = self.js('devicePixelRatio||1')
        pyautogui.click(round(origin['x'] + x * scale), round(origin['y'] + y * scale),
                        button={1: 'left', 2: 'middle', 3: 'right'}[button])


def windows_token_elevated():
    import ctypes
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    advapi = ctypes.WinDLL('advapi32', use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    advapi.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    advapi.GetTokenInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p,
                                         wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
    token, elevated, length = wintypes.HANDLE(), wintypes.DWORD(), wintypes.DWORD()
    if not advapi.OpenProcessToken(kernel.GetCurrentProcess(), 0x0008, ctypes.byref(token)):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        if not advapi.GetTokenInformation(token, 20, ctypes.byref(elevated), ctypes.sizeof(elevated), ctypes.byref(length)):
            raise ctypes.WinError(ctypes.get_last_error())
        return bool(elevated.value)
    finally:
        kernel.CloseHandle(token)


class OwnedInspectorPolicy:
    """Runner-only, exact-executable WebView2 override with typed restoration.

    Microsoft documents that elevated hosts ignore env/HKCU overrides but honor
    HKLM: https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/security
    ("For an elevated host app, use appropriate override flags"). No app change.
    """
    path = r'Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments'

    def __init__(self, executable, inspector, elevated):
        require(sys.platform == 'win32' and os.environ.get('GITHUB_ACTIONS') == 'true',
                'Inspector policy is limited to disposable GitHub Windows runners')
        import winreg
        require(executable.lower() == 'nexusbro.exe', 'Inspector policy must name only the owned NexusBro executable')
        self.registry, self.name = winreg, executable
        self.hive = winreg.HKEY_LOCAL_MACHINE if elevated else winreg.HKEY_CURRENT_USER
        self.scope = 'HKLM' if elevated else 'HKCU'
        self.access = winreg.KEY_QUERY_VALUE | winreg.KEY_SET_VALUE | winreg.KEY_WOW64_64KEY
        self.value = '--remote-debugging-port=' + str(inspector)
        self.previous, self.created_key, self.applied = None, False, False

    def apply(self):
        r = self.registry
        try:
            key = r.OpenKey(self.hive, self.path, 0, self.access)
        except FileNotFoundError:
            key = r.CreateKeyEx(self.hive, self.path, 0, self.access)
            self.created_key = True
        with key:
            try:
                self.previous = r.QueryValueEx(key, self.name)
            except FileNotFoundError:
                self.previous = None
            self.applied = True  # Restore even if writing/readback fails.
            r.SetValueEx(key, self.name, 0, r.REG_SZ, self.value)
            current, kind = r.QueryValueEx(key, self.name)
            require(current == self.value and kind == r.REG_SZ, 'Owned inspector policy readback mismatch')
        return {'policy_scope': self.scope, 'policy_named_exe': self.name, 'policy_port_matches': True,
                'previous_named_value_present': self.previous is not None}

    def restore(self):
        if not self.applied:
            return
        r = self.registry
        with r.OpenKey(self.hive, self.path, 0, self.access) as key:
            if self.previous is None:
                try:
                    r.DeleteValue(key, self.name)
                except FileNotFoundError:
                    pass
            else:
                value, kind = self.previous
                r.SetValueEx(key, self.name, 0, kind, value)
            empty = r.QueryInfoKey(key)[:2] == (0, 0)
        if self.created_key and empty:
            r.DeleteKeyEx(self.hive, self.path, r.KEY_WOW64_64KEY)
        self.applied = False


class WindowsDevice:
    def __init__(self, root, exe, cef):
        self.root, self.exe, self.cef = root, exe, cef
        self.data = root / 'data'
        self.data.mkdir(parents=True)
        self.inspector = port()
        self.app = self.client = self.log = None
        self.inspector_policy = None
        self.inspector_diagnostic = {'port': self.inspector, 'attempts': 0,
                                     'proxy_policy': 'owned_loopback_direct'}

    def start(self):
        import ctypes
        from pywinauto import Desktop
        require(sys.platform == 'win32' and os.environ.get('GITHUB_ACTIONS') == 'true',
                'Native Windows inspector fixture requires a disposable GitHub runner')
        elevated = windows_token_elevated()
        self.inspector_diagnostic['token_elevated'] = elevated
        self.inspector_policy = OwnedInspectorPolicy(self.exe.name, self.inspector, elevated)
        self.inspector_diagnostic.update({'stage': 'policy_override', 'policy_scope': self.inspector_policy.scope,
                                         'policy_named_exe': self.inspector_policy.name})
        try:
            self.inspector_diagnostic.update(self.inspector_policy.apply())
        except Exception:
            self.inspector_policy.restore()
            raise
        ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
        env = {**os.environ, 'NEXUSBRO_TEST_DATA': str(self.data), 'NEXUSBRO_PROBA': '0',
               'CEF_PATH': str(self.cef), 'PATH': str(self.cef) + os.pathsep + os.environ.get('PATH', ''),
               'WEBVIEW2_USER_DATA_FOLDER': str(self.root / 'webview2'),
               'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS': '--remote-debugging-port=' + str(self.inspector)}
        self.log = (self.root / ('app-' + str(time.time_ns()) + '.log')).open('wb')
        self.app = subprocess.Popen([str(self.exe)], env=env, stdout=self.log, stderr=subprocess.STDOUT)

        def ready():
            require(self.app.poll() is None, 'Owned native application exited during startup')
            self.inspector_diagnostic['attempts'] += 1
            try:
                self.client = WindowsClient(self.inspector, self.inspector_diagnostic)
                return self.client
            except (OSError, ValueError, websocket.WebSocketException) as error:
                self.inspector_diagnostic['last_error_type'] = type(error).__name__
                # Only local transport/parser errors, never request headers or
                # arbitrary process environment, enter the diagnostic artifact.
                self.inspector_diagnostic['last_error'] = str(error)[:300]
                return None
        client = wait_for(ready, 'WebView2 shell inspector did not become available', 45)
        windows = wait_for(lambda: Desktop(backend='uia').windows(process=self.app.pid, visible_only=True),
                           'Owned native Windows desktop window absent', 20)
        window = next((w for w in windows if 'NexusBro' in w.window_text()), windows[0])
        window.restore()
        window.move_window(x=20, y=20, width=1200, height=820)
        window.set_focus()
        return client

    def startup_diagnostics(self):
        diagnostic = dict(self.inspector_diagnostic)
        diagnostic['owned_processes'] = []
        if not self.app:
            return diagnostic
        try:
            root = psutil.Process(self.app.pid)
            for process in [root] + root.children(recursive=True):
                try:
                    relevant = ('--remote-debugging-port', '--remote-debugging-address',
                                '--user-data-dir', '--type=', '--embedded-browser-webview=')
                    flags = [arg[:500] for arg in process.cmdline() if arg.startswith(relevant)]
                    entry = {'pid': process.pid, 'name': process.name(), 'flags': flags}
                    try:
                        entry['inspector_listeners'] = [c.laddr.port for c in process.net_connections(kind='tcp')
                                                       if c.status == psutil.CONN_LISTEN and c.laddr.port == self.inspector]
                    except (psutil.NoSuchProcess, psutil.AccessDenied) as error:
                        entry['listener_error'] = type(error).__name__
                    diagnostic['owned_processes'].append(entry)
                except (psutil.NoSuchProcess, psutil.AccessDenied) as error:
                    diagnostic['owned_processes'].append({'pid': process.pid, 'error': type(error).__name__})
        except (psutil.NoSuchProcess, psutil.AccessDenied) as error:
            diagnostic['process_error'] = type(error).__name__
        return diagnostic

    def stop_app(self):
        try:
            self._stop_app()
        finally:
            if self.inspector_policy:
                self.inspector_policy.restore()

    def _stop_app(self):
        if self.client:
            self.client.close()
            self.client = None
        if self.app:
            try:
                process = psutil.Process(self.app.pid)
                owned = process.children(recursive=True) + [process]
                for child in reversed(owned):
                    try:
                        child.terminate()
                    except psutil.NoSuchProcess:
                        pass
                _, alive = psutil.wait_procs(owned, timeout=6)
                for child in alive:
                    try:
                        child.kill()
                    except psutil.NoSuchProcess:
                        pass
                psutil.wait_procs(alive, timeout=4)
            except psutil.NoSuchProcess:
                pass
            self.app = None
        if self.log:
            self.log.close()
            self.log = None

    close = stop_app


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding='utf-8')


def rows(client, owner, timeout=40):
    return call(client, 'obychnye_spisok', id=owner, timeout=timeout)


def row(client, owner, identifier, timeout=40):
    return next(t for t in rows(client, owner, timeout)['vkladki'] if t['id'] == identifier)


def click_control(client, selector, timeout):
    # Scrolling/geometry reads do not invoke application handlers. The final
    # action remains a real desktop mouse click through Client/WindowsClient.
    js_wait(client, '(()=>{const e=document.querySelector(' + json.dumps(selector) + ');'
            'if(!e)return false;e.scrollIntoView({block:"nearest",inline:"nearest",behavior:"instant"});'
            'const r=e.getBoundingClientRect(),x=r.x+r.width/2,y=r.y+r.height/2,h=document.elementFromPoint(x,y);'
            'return r.width>0&&r.height>0&&x>=0&&x<innerWidth&&y>=0&&y<innerHeight&&!!h&&(h===e||e.contains(h))})()',
            'Native UI control did not become physically reachable: ' + selector, timeout)
    client.click(selector)


def switch_super(client, owner, timeout=30):
    deadline = time.monotonic() + timeout
    remaining = lambda: max(.1, deadline - time.monotonic())
    selector = '.vkladka[data-super-id="' + owner + '"]'
    # IPC pereklyuchit updates native state but an already-running engine does
    # not emit the shell selection event. Only the actual sidebar click also
    # runs App's setAktivnaya handler; waiting after raw IPC cannot replace it.
    click_control(client, selector, remaining())
    active = '.vkladka.aktivna[data-super-id="' + owner + '"]'
    js_wait(client, '!!document.querySelector(' + json.dumps(active) + ')'
            '&&!!document.querySelector(".obychnaya[aria-selected=true]")',
            'Physical Super switch did not mount the requested shell owner', remaining())
    wait_for(lambda: call(client, 'aktivnaya', timeout=remaining()) == owner
             and any(v['id'] == owner and v['zapushchena'] for v in call(client, 'spisok', timeout=remaining())),
             'Physical Super switch did not start/select its native engine', remaining())


def target_dom(client, owner, identifier, timeout=20, route_button=False):
    deadline = time.monotonic() + timeout
    remaining = lambda: max(.1, deadline - time.monotonic())
    expected_url = row(client, owner, identifier, remaining())['adres']
    super_selector = '.vkladka.aktivna[data-super-id="' + owner + '"]'
    tab_selector = '.obychnaya[data-id="' + str(identifier) + '"][aria-selected=true]'
    expression = ('(()=>{const s=document.querySelector(' + json.dumps(super_selector) + '),'
                  't=document.querySelector(' + json.dumps(tab_selector) + ');'
                  'return !!s&&!!t&&(t.getAttribute("title")||"").includes(' + json.dumps(expected_url) + ')'
                  + ('&&!!document.querySelector(' + json.dumps('button[aria-label="Путь вкладки"]:not(:disabled)') + ')' if route_button else '')
                  + '})()')
    js_wait(client, expression, 'Shell owner/tab/document does not match the intended native target', remaining())


def activate_saved(client, owner):
    switch_super(client, owner)
    # Restored selected rows are lazy. A real tab click wakes the actual
    # selected row after the physical Super switch has mounted its shell.
    select(client, owner, rows(client, owner)['aktivnaya'])


def select(client, owner, identifier, timeout=30):
    deadline = time.monotonic() + timeout
    remaining = lambda: max(.1, deadline - time.monotonic())
    switch_super(client, owner, remaining())
    click_control(client, '.obychnaya[data-id="' + str(identifier) + '"]', remaining())
    def selected_loaded():
        current = rows(client, owner, remaining())
        return current['aktivnaya'] == identifier and any(t['id'] == identifier and not t['zagruzka'] for t in current['vkladki'])
    wait_for(selected_loaded, 'Native tab did not finish activation', remaining())
    target_dom(client, owner, identifier, remaining())


def choose(client, label):
    client.click('[role="combobox"][aria-label="Выберите путь вкладки"]')
    js_wait(client, '!!document.querySelector(".vybor-popup [role=option]")', 'Route dropdown did not open')
    # Attribute is a selector marker only; never invoke React or synthesize clicks.
    found = client.js('(()=>{const e=Array.from(document.querySelectorAll(".vybor-popup [role=option]")).find(e=>e.textContent.trim()===' + json.dumps(label) + ');if(!e)return false;e.setAttribute("data-route-test-choice","true");e.scrollIntoView({block:"nearest"});return true})()')
    require(found, 'Saved route absent from actual UI: ' + label)
    client.click('[data-route-test-choice="true"]')


def apply_ui(client, label, owner, identifier):
    target_dom(client, owner, identifier, route_button=True)
    require(call(client, 'aktivnaya') == owner and rows(client, owner)['aktivnaya'] == identifier,
            'Route UI target no longer matches the active native owner/tab')
    client.click('button[aria-label="Путь вкладки"]')
    js_wait(client, '!!document.querySelector("[data-tab-route-root] [role=combobox]")', 'Actual route dialog absent')
    choose(client, label)
    client.text_button('Применить путь')


def assert_receipt(fixture, case, lane, auth, storage, *, tls=False):
    records = wait_for(lambda: fixture.records(case), 'No actual CEF origin receipt for ' + case, 35)
    require(all(r['lane'] == lane and r['auth'] == auth and r['storage'] == storage and r['tls'] == tls for r in records),
            'Origin route/auth/storage mismatch for ' + case)


def apply_route_ack(client, owner, identifier, label, expected):
    """Physical selector actions, then authoritative native replacement ACK."""
    before = row(client, owner, identifier)
    apply_ui(client, label, owner, identifier)
    js_wait(client, '!document.querySelector("[data-tab-route-root]")', 'Route selection stayed open; native application was not confirmed', 40)

    def replacement():
        current = row(client, owner, identifier)
        return current if (current['route'] == expected
                           and current['route_generation'] == before['route_generation'] + 1
                           and current['browser_id'] > 0 and current['browser_id'] != before['browser_id']
                           and not current['route_pending'] and not current.get('route_error')) else None

    return wait_for(replacement, 'Route UI did not produce the acknowledged replacement browser', 40)


def run(args):
    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {'schema': 1, 'completed': False, 'checks': [], 'platform': sys.platform, 'coverage': {'http_ui': 'not_executed', 'https_connect': 'not_executed', 'refusal': 'not_executed', 'cold_restore': 'not_executed', 'transport_down': 'not_executed', 'rtc_dns_udp': 'not_executed', 'historical_cookies': 'not_executed', 'worker_ready_context': 'not_executed', 'worker_startup_pre_admission': 'not_executed'}}
    device = fixture = stun = None
    try:
        require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Only an ephemeral GitHub runner may run this native test')
        require(args.tls_cert and args.tls_key, 'HTTPS acceptance NOT EXECUTED: supply pretrusted runner-only CA-signed 127.0.0.1 certificate/key')
        stun_host = 'nexusbro-route-stun.test'
        resolved = {answer[4][0] for answer in socket.getaddrinfo(stun_host, None, socket.AF_INET, socket.SOCK_DGRAM)}
        require(resolved == {'127.0.0.1'}, 'Runner prerequisite missing: STUN fixture hostname must resolve only to 127.0.0.1')
        stun = StunFixture()
        fixture = Fixture(args.tls_cert, args.tls_key, 'stun:' + stun_host + ':' + str(stun.port))
        exe, cef = pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve()
        require(exe.is_file() and cef.is_dir(), 'Built native app/CEF prerequisite missing')
        device = WindowsDevice(out / 'device', exe, cef) if sys.platform == 'win32' else Device('routes', out / 'device', exe, cef)
        config = settings('grafit', 'unused')
        config['vyhody'] = [{'id': 'lane-' + lane, 'nazvanie': 'Fixture ' + lane,
                             'adres': 'http://127.0.0.1:' + str(fixture.proxies[lane]), 'proxy_uzel': ''} for lane in ('A', 'B')]
        write_json(device.data / 'nastrojki.json', config)
        supers = {name: super_record(name, name, 'about:blank', proxy=config['vyhody'][0]['adres']) for name in ('routes-a', 'routes-b')}
        supers['routes-direct'] = super_record('routes-direct', 'routes-direct', 'about:blank', proxy='')
        for name in ('routes-historical', 'routes-neighbor', 'routes-worker'):
            supers[name] = super_record(name, name, 'about:blank', proxy=config['vyhody'][0]['adres'])
        write_json(device.data / 'vkladki.json', list(supers.values()))

        def tab(identifier, case, route, seed=None, tls=False, rtc=False):
            return {'id': identifier, 'adres': fixture.url(case, seed, tls, rtc), 'zagolovok': case,
                    'route': route, 'zakreplena': False, 'otlozhena': False, 'privatnaya': False}
        inherit, direct = {'kind': 'inherit'}, {'kind': 'direct'}
        connection = lambda lane: {'kind': 'connection', 'id': 'lane-' + lane}
        tabs = [tab(1, 'seed-a', inherit, 'A'), tab(2, 'seed-b', connection('B'), 'B'),
                tab(3, 'share-a', connection('A')), tab(4, 'seed-direct', direct, 'Direct'),
                tab(5, 'share-direct', direct), tab(6, 'https-b', connection('B'), tls=True),
                tab(7, 'blocked-unknown', connection('missing')), tab(8, 'draft', inherit), tab(9, 'media', inherit),
                tab(10, 'rtc-vpn', inherit, rtc=True)]
        # Delayed tabs are activated explicitly, in order, to make first-request cookie witnesses deterministic.
        for t in tabs[1:]:
            t['otlozhena'] = True
        session_path = device.data / 'profili' / 'routes-a' / 'obychnye.json'
        write_json(session_path, {'vkladki': tabs, 'aktivnaya': 1, 'sleduyushchaya': 11, 'zakrytye': []})
        write_json(device.data / 'profili' / 'routes-b' / 'obychnye.json',
                   {'vkladki': [tab(1, 'other-super', inherit)], 'aktivnaya': 1, 'sleduyushchaya': 2, 'zakrytye': []})
        write_json(device.data / 'profili' / 'routes-direct' / 'obychnye.json',
                   {'vkladki': [tab(1, 'direct-super-seed', inherit, 'Direct'),
                                {**tab(2, 'direct-super-share', direct), 'otlozhena': True},
                                {**tab(3, 'rtc-direct', inherit, rtc=True), 'otlozhena': True}],
                    'aktivnaya': 1, 'sleduyushchaya': 4, 'zakrytye': []})
        historical_path = device.data / 'profili' / 'routes-historical' / 'obychnye.json'
        neighbor_path = device.data / 'profili' / 'routes-neighbor' / 'obychnye.json'
        historical_case = 'historical-clear-' + str(time.time_ns())
        neighbor_case = 'historical-neighbor-' + str(time.time_ns())
        write_json(historical_path, {'vkladki': [tab(1, 'historical-inherit', inherit),
                   {**tab(2, 'historical-seed', connection('B'), 'Historical'), 'otlozhena': True}],
                   'aktivnaya': 1, 'sleduyushchaya': 3, 'zakrytye': []})
        # Persistence is opt-in for these two witnesses, leaving every original
        # session-cookie/restart assertion and A/B seed case unchanged.
        saved_history = json.loads(historical_path.read_text(encoding='utf-8'))
        saved_history['vkladki'][1]['adres'] = fixture.url('historical-seed', 'Historical', persist=True)
        write_json(historical_path, saved_history)
        write_json(neighbor_path, {'vkladki': [{**tab(1, 'neighbor-seed', inherit, 'Neighbor'),
                   'adres': fixture.url('neighbor-seed', 'Neighbor', persist=True)}],
                   'aktivnaya': 1, 'sleduyushchaya': 2, 'zakrytye': []})
        write_json(device.data / 'profili' / 'routes-worker' / 'obychnye.json',
                   {'vkladki': [{'id': 1, 'adres': fixture.worker_url('worker-first'), 'zagolovok': 'Worker route fixture',
                                 'route': connection('B'), 'otlozhena': False}],
                    'aktivnaya': 1, 'sleduyushchaya': 2, 'zakrytye': []})
        client = device.start()
        activate_saved(client, 'routes-a')
        assert_receipt(fixture, 'seed-a', 'A', 'A', 'A')
        for identifier, case, lane, auth, storage, tls in [
                (2, 'seed-b', 'B', 'B', 'B', False), (3, 'share-a', 'A', 'A', 'A', False),
                (4, 'seed-direct', 'Direct', 'Direct', 'Direct', False),
                (5, 'share-direct', 'Direct', 'Direct', 'Direct', False),
                (6, 'https-b', 'B', 'B', 'empty', True)]:
            select(client, 'routes-a', identifier)
            assert_receipt(fixture, case, lane, auth, storage, tls=tls)
        with fixture.lock:
            require(any(t['lane'] == 'B' and t['port'] == fixture.https for t in fixture.tunnels), 'No actual HTTPS CONNECT witnessed')
            first_b = [r for r in fixture.receipts if r.get('case') == 'seed-b' and r['path'] == '/page']
            require(first_b and first_b[0]['auth'] is None, 'Different route inherited another profile before first navigation')
        report['coverage']['https_connect'] = 'passed'
        report['checks'].append('http_https_connect_same_effective_key_shares_different_key_isolates')
        # The owner's first route choice must work before any website loads.
        before_ids = {t['id'] for t in rows(client, 'routes-a')['vkladki']}
        call(client, 'obychnaya_komanda', id='routes-a', komanda={'deystvie': 'novaya', 'adres': 'about:blank'})

        def owned_blank():
            current = rows(client, 'routes-a')
            return next((t for t in current['vkladki'] if t['id'] not in before_ids
                         and current['aktivnaya'] == t['id'] and t['adres'] == 'about:blank'
                         and t['browser_id'] > 0 and not t['zagruzka'] and not t['route_pending']), None)

        blank = wait_for(owned_blank, 'New owned about:blank tab did not acquire a real browser', 30)
        require(not blank['nazad'], 'Owned blank bootstrap added an artificial Back entry')
        apply_route_ack(client, 'routes-a', blank['id'], 'Fixture B', connection('B'))
        require(not row(client, 'routes-a', blank['id'])['nazad'], 'Routed blank bootstrap added an artificial Back entry')
        blank_case = 'blank-to-b-' + str(time.time_ns())
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': blank['id'], 'adres': fixture.url(blank_case)})
        assert_receipt(fixture, blank_case, 'B', 'B', 'B')
        report['checks'].append('owned_about_blank_actual_route_selector_b_ack_then_b_auth_storage_network')
        start = time.monotonic()
        wait_for(lambda: any(r['time'] > start for r in fixture.records('seed-a')) and any(r['time'] > start for r in fixture.records('seed-b')),
                 'Two differently routed native pages did not run concurrently', 20)
        report['checks'].append('two_native_pages_concurrent_different_routes')
        activate_saved(client, 'routes-b')
        assert_receipt(fixture, 'other-super', 'A', None, 'empty')
        report['checks'].append('cross_super_profile_isolation')
        activate_saved(client, 'routes-direct')
        assert_receipt(fixture, 'direct-super-seed', 'Direct', 'Direct', 'Direct')
        select(client, 'routes-direct', 2)
        assert_receipt(fixture, 'direct-super-share', 'Direct', 'Direct', 'Direct')
        with fixture.lock:
            first_direct = [r for r in fixture.receipts
                            if r.get('case') == 'direct-super-seed' and r['path'] == '/page']
            require(first_direct and first_direct[0]['auth'] is None,
                    'Direct Super inherited another Super authentication profile')
        report['checks'].append('direct_super_inherit_matches_explicit_direct_profile_and_network')
        # Browser DNS + real ICE/STUN positive control precedes the VPN leak
        # check. A fixture-only protocol success cannot satisfy this assertion.
        def rtc_events(case):
            return [r for r in fixture.requests(case) if r['path'] == '/rtc']

        def complete_rtc(case, lane, minimum_seconds=0):
            events = rtc_events(case)
            names = {r['rtc_event'] for r in events}
            if not {'started', 'gathering-done', 'closed'} <= names or names & {'timeout', 'error'}:
                return None
            if any(r['lane'] != lane for r in events):
                return None
            started = next(r['time'] for r in events if r['rtc_event'] == 'started')
            return events if time.monotonic() - started >= minimum_seconds else None

        def start_rtc(owner, identifier, began):
            select(client, owner, identifier, timeout=max(.1, 15 - (time.monotonic() - began)))

        direct_started = time.monotonic()
        start_rtc('routes-direct', 3, direct_started)
        def direct_rtc_witness():
            events = complete_rtc('rtc-direct', 'Direct')
            return events if (events and any(p['binding'] for p in stun.since(direct_started))
                              and any(r['candidate_type'] == 'srflx' and r['candidate_address'] == '203.0.113.10'
                                      for r in events)) else None
        wait_for(direct_rtc_witness, 'Direct hostname ICE did not produce actual STUN UDP + mapped srflx candidate',
                 max(.1, 15 - (time.monotonic() - direct_started)))
        report['checks'].append('direct_hostname_rtc_real_udp_and_srflx_positive_control')
        # Direct PC has closed after its final HTTP receipt. No remote SDP is
        # ever installed; the documentation-only mapped IP is not contacted.
        time.sleep(.3)
        vpn_started = time.monotonic()
        start_rtc('routes-a', 10, vpn_started)
        wait_for(lambda: complete_rtc('rtc-vpn', 'A', minimum_seconds=5),
                 'VPN ICE did not genuinely start/complete/close within bounded no-UDP observation',
                 max(.1, 15 - (time.monotonic() - vpn_started)))
        require(not stun.since(vpn_started), 'VPN profile leaked non-proxied UDP to hostname STUN fixture')
        report['coverage']['rtc_dns_udp'] = 'passed'
        report['checks'].append('vpn_hostname_rtc_completed_no_udp_at_least_five_seconds')
        select(client, 'routes-a', 7)
        wait_for(lambda: row(client, 'routes-a', 7).get('route_error'), 'Unknown saved ID was not explicitly unavailable')
        require(not fixture.requests('blocked-unknown'), 'Unknown ID fell back to a network route')
        report['checks'].append('unknown_connection_id_fail_closed')
        # Real UI change and native ACK: route/generation/browser identity are read only after the transaction.
        select(client, 'routes-a', 5)
        before = row(client, 'routes-a', 5)
        apply_ui(client, 'Fixture B', 'routes-a', 5)
        js_wait(client, '!document.querySelector("[data-tab-route-root]")', 'Route UI did not close after ACK', 40)
        def acknowledged_replacement():
            current = row(client, 'routes-a', 5)
            if (current['route'] == connection('B')
                    and current['route_generation'] == before['route_generation'] + 1
                    and current['browser_id'] != before['browser_id']
                    and not current['route_pending'] and not current.get('route_error')):
                return current
            return None
        after = wait_for(acknowledged_replacement,
                         'Native ACK was not followed by the authoritative replacement session row', 40)
        require(after['route'] == connection('B') and after['route_generation'] == before['route_generation'] + 1
                and after['browser_id'] != before['browser_id'] and not after['route_pending'] and not after.get('route_error'),
                'Actual route UI did not commit an acknowledged replacement browser')
        wait_for(lambda: any(r['lane'] == 'B' and r['auth'] == 'B' and r['storage'] == 'B' for r in fixture.records('share-direct')),
                 'Acknowledged route UI change did not reach B profile/network')
        report['coverage']['http_ui'] = 'passed'
        report['checks'].append('actual_route_selector_native_ack_network_and_profile')
        # Protected-page refusal is tested through physical CEF input, then through the real shell menu.
        for identifier, case, offset, witness in [(8, 'draft', 30, 'draft-typed'), (9, 'media', 80, 'media-running')]:
            select(client, 'routes-a', identifier)
            assert_receipt(fixture, case, 'A', 'A', 'A')
            bounds = client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return [r.x,r.y]})()')
            client.mouse(bounds[0] + 50, bounds[1] + offset)
            if case == 'draft':
                if sys.platform == 'win32':
                    import pyautogui
                    pyautogui.write('synthetic unsaved draft', interval=.04)
                else:
                    subprocess.run(['xdotool', 'type', '--clearmodifiers', '--', 'synthetic unsaved draft'], env={**os.environ, 'DISPLAY': device.display}, check=True)
            wait_for(lambda: fixture.records(witness), 'Actual protected page interaction did not occur: ' + case)
            before = row(client, 'routes-a', identifier)
            apply_ui(client, 'Fixture B', 'routes-a', identifier)
            wait_for(lambda: row(client, 'routes-a', identifier).get('route_error'), 'Native route change did not refuse protected ' + case, 40)
            after = row(client, 'routes-a', identifier)
            require(after['route'] == before['route'] and after['browser_id'] == before['browser_id']
                    and after['route_generation'] == before['route_generation'] + 1 and not after['route_pending'],
                    'Refused route attempt changed protected page/profile or lost attempt generation')
            js_wait(client, '!!document.querySelector("[data-tab-route-root] [role=alert]")',
                    'Native refusal absent from actual route UI', 20)
            client.text_button('Отмена')
            report['checks'].append(case + '_route_change_refused_preserves_browser')
        report['coverage']['refusal'] = 'passed'
        # Prepare independent persistent state before the existing owned restart.
        # No original routes-a/routes-b profile is cleared or reseeded here.
        activate_saved(client, 'routes-historical')
        select(client, 'routes-historical', 2)
        assert_receipt(fixture, 'historical-seed', 'B', 'Historical', 'Historical')
        call(client, 'obychnaya_komanda', id='routes-historical', komanda={
             'deystvie': 'pereyti', 'id': 2, 'adres': fixture.url(historical_case)})
        assert_receipt(fixture, historical_case, 'B', 'Historical', 'Historical')
        select(client, 'routes-historical', 1)
        activate_saved(client, 'routes-neighbor')
        assert_receipt(fixture, 'neighbor-seed', 'A', 'Neighbor', 'Neighbor')
        call(client, 'obychnaya_komanda', id='routes-neighbor', komanda={
             'deystvie': 'pereyti', 'id': 1, 'adres': fixture.url(neighbor_case)})
        assert_receipt(fixture, neighbor_case, 'A', 'Neighbor', 'Neighbor')

        def worker_witness(case, existing):
            requests = fixture.requests(case)
            require(all(r['lane'] == 'B' for r in requests), 'Worker owner used A/Direct instead of B')
            status = [r for r in requests if r['path'] == '/worker/page-status']
            proof = [r for r in requests if r['path'] == '/worker/receipt' and r['worker_proof'] and r['worker_event'] == 'message']
            require(not any(r['path'] == '/worker/page-error' for r in requests), 'Actual worker registration/message failed')
            return proof if proof and any(r['worker_existing'] == str(existing).lower() and r['secure'] == 'true' for r in status) else None

        activate_saved(client, 'routes-worker')
        wait_for(lambda: worker_witness('worker-first', False), 'No actual B worker fetch/secure-context positive receipt', 20)
        # Change only the saved client URL. After restart, getRegistration must
        # find the persisted active worker; a fresh registration cannot pass.
        call(client, 'obychnaya_komanda', id='routes-worker', komanda={
             'deystvie': 'pereyti', 'id': 1, 'adres': fixture.worker_url('worker-restored')})
        wait_for(lambda: worker_witness('worker-restored', True), 'Installed B worker could not handle a second native client', 20)

        def gap_state_persisted():
            try:
                h = json.loads(historical_path.read_text(encoding='utf-8'))
                n = json.loads(neighbor_path.read_text(encoding='utf-8'))
                w = json.loads((device.data / 'profili' / 'routes-worker' / 'obychnye.json').read_text(encoding='utf-8'))
                return (h['aktivnaya'] == 1 and h['vkladki'][1]['adres'] == fixture.url(historical_case)
                        and h['vkladki'][1]['route'] == connection('B')
                        and n['vkladki'][0]['adres'] == fixture.url(neighbor_case)
                        and w['vkladki'][0]['adres'] == fixture.worker_url('worker-restored'))
            except (OSError, ValueError, KeyError, IndexError):
                return False
        wait_for(gap_state_persisted, 'Historical/worker non-seed URLs were not persisted before restart', 30)
        def cookie_metadata_snapshot(phase):
            # Diagnostics only. Native post-restart cookieStats and the neighbor
            # server's actual persistent auth are the functional requirements.
            # Never inspect or serialize encrypted/plain cookie token values.
            import contextlib
            import itertools
            import sqlite3
            snapshot = {}
            for owner in ('routes-historical', 'routes-neighbor'):
                profile = device.data / 'profili' / owner
                data = {'profile_exists': profile.is_dir(), 'cookie_files': []}
                try:
                    candidates = [p for p in itertools.islice(profile.rglob('*'), 2000)
                                  if p.is_file() and 'cookie' in p.name.lower()][:16]
                    for database in candidates:
                        entry = {'path': str(database.relative_to(profile)), 'bytes': database.stat().st_size}
                        data['cookie_files'].append(entry)
                        if database.name != 'Cookies':
                            continue
                        with database.open('rb') as stream:
                            entry['sqlite_header'] = stream.read(16) == b'SQLite format 3\0'
                        try:
                            with contextlib.closing(sqlite3.connect(database.resolve().as_uri() + '?mode=ro', uri=True, timeout=.1)) as db:
                                columns = [r[1] for r in db.execute('PRAGMA table_info(cookies)').fetchall()]
                                entry['columns'] = columns
                                if {'host_key', 'name', 'is_persistent'} <= set(columns):
                                    entry['fixture_cookie_count'] = db.execute("SELECT count(*) FROM cookies WHERE host_key IN ('127.0.0.1','.127.0.0.1') AND name='route_auth'").fetchone()[0]
                                    entry['persistent_fixture_cookie_count'] = db.execute("SELECT count(*) FROM cookies WHERE host_key IN ('127.0.0.1','.127.0.0.1') AND name='route_auth' AND is_persistent=1").fetchone()[0]
                        except sqlite3.Error as error:
                            entry['sqlite_error_type'] = type(error).__name__
                            entry['sqlite_error_name'] = getattr(error, 'sqlite_errorname', None)
                except OSError as error:
                    data['filesystem_error_type'] = type(error).__name__
                snapshot[owner] = data
            report.setdefault('cookie_metadata_diagnostics', {})[phase] = snapshot
            return snapshot

        def persistent_cookie_barrier():
            snapshot = cookie_metadata_snapshot('live_before_restart')
            # snapshot reads BOTH owners before these booleans are evaluated.
            both = [any(entry.get('persistent_fixture_cookie_count', 0) > 0
                        for entry in snapshot[owner]['cookie_files'])
                    for owner in ('routes-historical', 'routes-neighbor')]
            return all(both)
        wait_for(persistent_cookie_barrier,
                 'Native persistent cookie writes were not durable before the owned restart', 40)



        def persisted_selected_b(address=None):
            # The native session callback updates RAM before writing disk. A
            # forced process stop must wait for actual persisted route intent.
            try:
                saved = json.loads(session_path.read_text(encoding='utf-8'))
                selected = next(t for t in saved['vkladki'] if t['id'] == 2)
                return saved if (saved['aktivnaya'] == 2 and selected['route'] == connection('B')
                                 and (address is None or selected['adres'] == address)) else None
            except (OSError, ValueError, KeyError, StopIteration, TypeError):
                return None

        # Stop only our owned process tree, reorder IDs, and cold restore selected B.
        select(client, 'routes-a', 2)
        wait_for(persisted_selected_b, 'Selected B route was not persisted before first cold restart', 30)
        cookie_metadata_snapshot('before_restart')
        device.stop_app()
        cookie_metadata_snapshot('after_stop')
        config['vyhody'].reverse()
        write_json(device.data / 'nastrojki.json', config)
        client = device.start()
        restored_at = time.monotonic()
        activate_saved(client, 'routes-a')
        wait_for(lambda: any(r['time'] > restored_at and r['lane'] == 'B' for r in fixture.records('seed-b')), 'Cold restore lost opaque B connection ID')
        require(row(client, 'routes-a', 2)['route'] == connection('B'), 'Restart rebound connection by list index')
        report['checks'].append('restart_opaque_id_survives_connection_reorder')
        # Historical B is on disk but has no Browser in this new engine. Stats
        # and clear must enumerate its closed-gate context without networking.
        historical_restart_at = time.monotonic()
        activate_saved(client, 'routes-historical')
        require(row(client, 'routes-historical', 2)['browser_id'] == 0, 'Historical B opened before clear')
        require(not [r for r in fixture.requests(historical_case) if r['time'] > restored_at],
                'Historical B requested its origin before clear')
        cookie_metadata_snapshot('after_restart_before_stats')
        before_clear = call(client, 'dannye_saytov_statistika', id='routes-historical', timeout=20)
        report['historical_native_cookie_counts'] = {'before_clear': before_clear.get('cookies')}
        require(type(before_clear.get('cookies')) is int and before_clear['cookies'] > 0,
                'No persistent historical cookie existed before the clear operation')
        cleared = call(client, 'ochistit_dannye_saytov', id='routes-historical', cookies=True, cache=False, timeout=20)
        require(cleared.get('cookies') is True and cleared.get('cache') is False, 'Actual cookie clear was not acknowledged')
        after_clear = call(client, 'dannye_saytov_statistika', id='routes-historical', timeout=20)
        report['historical_native_cookie_counts']['after_clear'] = after_clear.get('cookies')
        require(after_clear.get('cookies') == 0, 'Historical native cookies remained after clear')
        require(row(client, 'routes-historical', 2)['browser_id'] == 0, 'Stats/clear opened the historical B Browser')
        require(not [r for r in fixture.requests(historical_case) if r['time'] > restored_at],
                'Closed-gate historical context performed origin traffic during stats/clear')
        select(client, 'routes-historical', 2)
        wait_for(lambda: [r for r in fixture.records(historical_case) if r['time'] > historical_restart_at],
                 'Historical B never loaded after actual cookie clear', 20)
        historical_after = [r for r in fixture.requests(historical_case) if r['time'] > historical_restart_at]
        require(all(r['lane'] == 'B' and r['auth'] is None for r in historical_after),
                'Historical B authentication survived cookie clear or used a wrong lane')
        require(any(r['storage'] == 'Historical' for r in historical_after),
                'Cookies-only API unexpectedly erased localStorage (the UI explicitly excludes it)')
        neighbor_after_at = time.monotonic()
        activate_saved(client, 'routes-neighbor')
        wait_for(lambda: [r for r in fixture.records(neighbor_case) if r['time'] > neighbor_after_at],
                 'Neighbor did not produce a new persistent state witness after historical clear', 20)
        require(all(r['lane'] == 'A' and r['auth'] == 'Neighbor' and r['storage'] == 'Neighbor'
                    for r in fixture.records(neighbor_case) if r['time'] > neighbor_after_at),
                'Clearing one Super affected the neighboring Super cookie/storage profile')
        report['coverage']['historical_cookies'] = 'passed'
        report['checks'].append('historical_unopened_route_cookies_clear_no_network_ls_preserved_neighbor_untouched')
        worker_restored_at = time.monotonic()
        activate_saved(client, 'routes-worker')
        def restored_worker():
            proof = worker_witness('worker-restored', True)
            return [r for r in proof if r['time'] > worker_restored_at] if proof else None
        wait_for(restored_worker, 'Persisted worker produced no actual B fetch after restart', 20)
        with fixture.lock:
            worker_requests = [r for r in fixture.receipts if r['path'].startswith('/worker/')]
            require(worker_requests and all(r['lane'] == 'B' for r in worker_requests),
                    'Worker traffic escaped B before or after restart')
        report['coverage']['worker_ready_context'] = 'passed'
        report['checks'].append('actual_service_worker_fetch_b_only_persisted_registration_after_restart')
        # This positive is genuinely worker-originated, but requires a ready B
        # client. It does NOT test autonomous worker startup before admission.
        report['coverage']['worker_startup_pre_admission'] = 'not_executed'

        # Existing configured B remains selected when its actual transport dies.
        # A fresh URL makes every origin hit (even before page JS) a failure.
        select(client, 'routes-a', 1)
        healthy_at = time.monotonic()
        wait_for(lambda: any(r['time'] > healthy_at and r['lane'] == 'A' for r in fixture.records('seed-a')),
                 'A route was not live before B transport failure', 20)
        select(client, 'routes-a', 2)
        before_transport = row(client, 'routes-a', 2)
        fixture.stop_proxy('B')
        transport_down_at = time.monotonic()
        down_case = 'transport-down-' + str(time.time_ns())
        down_url = fixture.url(down_case)
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': 2, 'adres': down_url})

        def failed_load_settled():
            current = row(client, 'routes-a', 2)
            return current if current['adres'] == down_url and not current['zagruzka'] else None

        after_transport = wait_for(failed_load_settled, 'B transport failure did not settle its real navigation', 40)
        require(after_transport['route'] == connection('B')
                and after_transport['browser_id'] == before_transport['browser_id']
                and after_transport['route_generation'] == before_transport['route_generation'],
                'Transport failure changed B route intent/profile/browser')
        # Keep observing while A makes multiple real requests, catching delayed
        # Direct fallback as well as proving unrelated route continuity.
        wait_for(lambda: len([r for r in fixture.records('seed-a')
                              if r['time'] > transport_down_at and r['lane'] == 'A']) >= 2,
                 'Healthy A page stopped when B transport failed', 20)
        time.sleep(2)
        require(not fixture.requests(down_case), 'Dead configured B transport leaked a request through Direct/another route')
        require(row(client, 'routes-a', 2)['route'] == connection('B'), 'Dead transport silently changed saved B intent')
        report['coverage']['transport_down'] = 'pending_final_no_fallback_check'
        # Preserve down_case's permanent zero-origin invariant. Recovery uses
        # another URL whose first load genuinely fails on the dead B transport.
        recovery_case = 'recover-network-error-' + str(time.time_ns())
        recovery_url = fixture.url(recovery_case)
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': 2, 'adres': recovery_url})
        wait_for(lambda: row(client, 'routes-a', 2)['adres'] == recovery_url
                 and not row(client, 'routes-a', 2)['zagruzka'], 'Native B network-error page did not settle', 40)
        require(not fixture.requests(recovery_case), 'Dead B reached recovery URL before any route change')
        apply_route_ack(client, 'routes-a', 2, 'Fixture A', connection('A'))
        assert_receipt(fixture, recovery_case, 'A', 'A', 'A')
        require(row(client, 'routes-a', 2)['adres'] == recovery_url, 'Network-error recovery silently changed requested URL')
        require(not fixture.requests(down_case), 'Network-error route recovery revisited the permanent no-fallback URL')
        report['checks'].append('native_network_error_actual_route_selector_a_recovers_same_url_auth_storage')
        # Selecting the configured but dead B is still an explicit route
        # transaction. It must ACK intent without sending this URL via A/Direct.
        recovery_main_loads = sum(r['path'] == '/page' for r in fixture.requests(recovery_case))
        apply_route_ack(client, 'routes-a', 2, 'Fixture B', connection('B'))
        wait_for(lambda: row(client, 'routes-a', 2)['adres'] == recovery_url
                 and not row(client, 'routes-a', 2)['zagruzka'], 'Explicit switch back to dead B did not settle', 40)
        # Retired A-page heartbeat fetches may finish after replacement ACK.
        # A fallback navigation would add another origin /page GET, which must
        # never happen even if it reuses the old A lane/auth/profile.
        time.sleep(2)
        require(sum(r['path'] == '/page' for r in fixture.requests(recovery_case)) == recovery_main_loads
                and all(r['lane'] == 'A' for r in fixture.requests(recovery_case)),
                'Switch to unavailable B leaked the recovered URL through another route')
        require(not fixture.requests(down_case), 'Switch back to B broke permanent transport-down no-fallback invariant')
        report['checks'].append('network_error_recovery_can_reselect_unavailable_b_without_direct')
        # Restore the original URL while B is still down, preserving the
        # deleted-ID cold-restore test's unique prior URL and B selection.
        seed_b_url = tabs[1]['adres']
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': 2, 'adres': seed_b_url})
        wait_for(lambda: row(client, 'routes-a', 2)['adres'] == seed_b_url
                 and not row(client, 'routes-a', 2)['zagruzka'], 'B URL restore did not settle while transport stayed down', 40)
        wait_for(lambda: persisted_selected_b(seed_b_url), 'Selected B URL/intent was not persisted while its transport was down', 30)
        wait_for(persisted_selected_b, 'Selected B route was not persisted before deleted-ID restart', 30)
        device.stop_app()
        config['vyhody'] = [c for c in config['vyhody'] if c['id'] != 'lane-B']
        write_json(device.data / 'nastrojki.json', config)
        before_restart = len(fixture.requests('seed-b'))
        client = device.start()
        activate_saved(client, 'routes-a')
        wait_for(lambda: row(client, 'routes-a', 2).get('route_error'), 'Deleted ID was not blocked on cold restore', 40)
        time.sleep(2)
        require(len(fixture.requests('seed-b')) == before_restart and not fixture.requests('blocked-unknown'), 'Removed/unknown ID silently used Direct or stale proxy')
        require(not fixture.requests(down_case), 'Delayed dead-transport fallback reached the origin')
        require(sum(r['path'] == '/page' for r in fixture.requests(recovery_case)) == recovery_main_loads,
                'Delayed switch-back fallback navigated the recovered URL through another route')
        report['coverage']['transport_down'] = 'passed'
        report['checks'].append('configured_transport_down_no_direct_fallback_unaffected_a_live')
        report['checks'].append('deleted_connection_cold_restore_fail_closed')
        with fixture.lock:
            worker_requests = [r for r in fixture.receipts if r['path'].startswith('/worker/')]
            require(worker_requests and all(r['lane'] == 'B' for r in worker_requests),
                    'Delayed worker traffic escaped the selected B lane')
            require(not any(r['time'] > transport_down_at for r in worker_requests),
                    'Worker origin traffic survived the stopped configured B transport')
        report['coverage']['cold_restore'] = 'passed'
        report['completed'] = True
    except Exception as error:
        # No credential/raw server receipts are written to artifacts.
        report['error'] = type(error).__name__ + ': ' + str(error)
        if device and sys.platform == 'win32':
            try:
                report['inspector_on_failure'] = device.startup_diagnostics()
            except Exception as diagnostic_error:
                report['inspector_diagnostic_error'] = type(diagnostic_error).__name__
        if device and device.client:
            try:
                report['shell_text_on_failure'] = device.client.js('document.body.innerText').strip()[:12000]
                report['session_on_failure'] = rows(device.client, 'routes-a')
            except Exception as diagnostic_error:
                report['diagnostic_error'] = type(diagnostic_error).__name__
        if device:
            # Startup/inspector failures need a native screen too, even before
            # a CDP client exists. Capture before terminating the owned app.
            try:
                from PIL import ImageGrab
                (ImageGrab.grab() if sys.platform == 'win32' else ImageGrab.grab(xdisplay=device.display)).save(out / 'failure.png')
            except Exception as screenshot_error:
                report['screenshot_error'] = type(screenshot_error).__name__
        raise
    finally:
        if device:
            device.close()
        if fixture:
            fixture.close()
        if stun:
            stun.close()
        (out / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', required=True)
    parser.add_argument('--cef', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--tls-cert')
    parser.add_argument('--tls-key')
    run(parser.parse_args())
