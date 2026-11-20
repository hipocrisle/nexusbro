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

import psutil
import websocket
from client import Client
from route_fixture import Fixture
from stun_fixture import StunFixture
from security_native import Device, call, js_wait, port, require, settings, super_record, wait_for


class WindowsClient(Client):
    """WebView2 inspector reads shell IPC; all UI clicks are physical input."""
    def __init__(self, inspector):
        self.seq = 0
        with urllib.request.urlopen('http://127.0.0.1:' + str(inspector) + '/json/list', timeout=2) as response:
            pages = json.load(response)
        self.ws = None
        for page in pages:
            if page.get('type') != 'page' or not page.get('webSocketDebuggerUrl'):
                continue
            candidate = websocket.create_connection(page['webSocketDebuggerUrl'], timeout=5, suppress_origin=True)
            self.ws = candidate
            try:
                if self.js('!!window.__TAURI_INTERNALS__?.invoke'):
                    return
            except (OSError, RuntimeError, websocket.WebSocketException):
                pass
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


class WindowsDevice:
    def __init__(self, root, exe, cef):
        self.root, self.exe, self.cef = root, exe, cef
        self.data = root / 'data'
        self.data.mkdir(parents=True)
        self.inspector = port()
        self.app = self.client = self.log = None

    def start(self):
        import ctypes
        from pywinauto import Desktop
        ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
        env = {**os.environ, 'NEXUSBRO_TEST_DATA': str(self.data), 'NEXUSBRO_PROBA': '0',
               'CEF_PATH': str(self.cef), 'PATH': str(self.cef) + os.pathsep + os.environ.get('PATH', ''),
               'WEBVIEW2_USER_DATA_FOLDER': str(self.root / 'webview2'),
               'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS': '--remote-debugging-port=' + str(self.inspector)}
        self.log = (self.root / ('app-' + str(time.time_ns()) + '.log')).open('wb')
        self.app = subprocess.Popen([str(self.exe)], env=env, stdout=self.log, stderr=subprocess.STDOUT)

        def ready():
            require(self.app.poll() is None, 'Owned native application exited during startup')
            try:
                self.client = WindowsClient(self.inspector)
                return self.client
            except (OSError, ValueError, websocket.WebSocketException):
                return None
        client = wait_for(ready, 'WebView2 shell inspector did not become available', 45)
        windows = wait_for(lambda: Desktop(backend='uia').windows(process=self.app.pid, visible_only=True),
                           'Owned native Windows desktop window absent', 20)
        window = next((w for w in windows if 'NexusBro' in w.window_text()), windows[0])
        window.restore()
        window.move_window(x=20, y=20, width=1200, height=820)
        window.set_focus()
        return client

    def stop_app(self):
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


def rows(client, owner):
    return call(client, 'obychnye_spisok', id=owner)


def row(client, owner, identifier):
    return next(t for t in rows(client, owner)['vkladki'] if t['id'] == identifier)


def select(client, owner, identifier):
    call(client, 'pereklyuchit', id=owner)
    call(client, 'obychnaya_komanda', id=owner, komanda={'deystvie': 'vybrat', 'id': identifier})
    wait_for(lambda: rows(client, owner)['aktivnaya'] == identifier and not row(client, owner, identifier)['zagruzka'],
             'Native tab did not finish activation', 30)


def choose(client, label):
    client.click('[role="combobox"][aria-label="Выберите путь вкладки"]')
    js_wait(client, '!!document.querySelector(".vybor-popup [role=option]")', 'Route dropdown did not open')
    # Attribute is a selector marker only; never invoke React or synthesize clicks.
    found = client.js('(()=>{const e=Array.from(document.querySelectorAll(".vybor-popup [role=option]")).find(e=>e.textContent.trim()===' + json.dumps(label) + ');if(!e)return false;e.setAttribute("data-route-test-choice","true");e.scrollIntoView({block:"nearest"});return true})()')
    require(found, 'Saved route absent from actual UI: ' + label)
    client.click('[data-route-test-choice="true"]')


def apply_ui(client, label):
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
    active_selector = '.obychnaya[data-id="' + str(identifier) + '"][aria-selected="true"]'
    js_wait(client, '!!document.querySelector(' + json.dumps(active_selector) + ')'
            '&&!!document.querySelector(' + json.dumps('button[aria-label="Путь вкладки"]:not(:disabled)') + ')',
            'Actual route selector did not follow the selected native tab', 20)
    before = row(client, owner, identifier)
    apply_ui(client, label)
    js_wait(client, '!document.querySelector("[data-tab-route-root]")', 'Route dialog did not close after native ACK', 40)

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
    report = {'schema': 1, 'completed': False, 'checks': [], 'platform': sys.platform, 'coverage': {'http_ui': 'not_executed', 'https_connect': 'not_executed', 'refusal': 'not_executed', 'cold_restore': 'not_executed', 'transport_down': 'not_executed', 'rtc_dns_udp': 'not_executed'}}
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
        client = device.start()
        call(client, 'pereklyuchit', id='routes-a')
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
        apply_route_ack(client, 'routes-a', blank['id'], 'Fixture B', connection('B'))
        blank_case = 'blank-to-b-' + str(time.time_ns())
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': blank['id'], 'adres': fixture.url(blank_case)})
        assert_receipt(fixture, blank_case, 'B', 'B', 'B')
        report['checks'].append('owned_about_blank_actual_route_selector_b_ack_then_b_auth_storage_network')
        start = time.monotonic()
        wait_for(lambda: any(r['time'] > start for r in fixture.records('seed-a')) and any(r['time'] > start for r in fixture.records('seed-b')),
                 'Two differently routed native pages did not run concurrently', 20)
        report['checks'].append('two_native_pages_concurrent_different_routes')
        call(client, 'pereklyuchit', id='routes-b')
        assert_receipt(fixture, 'other-super', 'A', None, 'empty')
        report['checks'].append('cross_super_profile_isolation')
        call(client, 'pereklyuchit', id='routes-direct')
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
            remaining = lambda: max(.1, 15 - (time.monotonic() - began))
            call(client, 'pereklyuchit', id=owner, timeout=remaining())
            call(client, 'obychnaya_komanda', id=owner,
                 komanda={'deystvie': 'vybrat', 'id': identifier}, timeout=remaining())

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
        apply_ui(client, 'Fixture B')
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
            apply_ui(client, 'Fixture B')
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
        device.stop_app()
        config['vyhody'].reverse()
        write_json(device.data / 'nastrojki.json', config)
        client = device.start()
        restored_at = time.monotonic()
        call(client, 'pereklyuchit', id='routes-a')
        wait_for(lambda: any(r['time'] > restored_at and r['lane'] == 'B' for r in fixture.records('seed-b')), 'Cold restore lost opaque B connection ID')
        require(row(client, 'routes-a', 2)['route'] == connection('B'), 'Restart rebound connection by list index')
        report['checks'].append('restart_opaque_id_survives_connection_reorder')
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
        call(client, 'pereklyuchit', id='routes-a')
        wait_for(lambda: row(client, 'routes-a', 2).get('route_error'), 'Deleted ID was not blocked on cold restore', 40)
        time.sleep(2)
        require(len(fixture.requests('seed-b')) == before_restart and not fixture.requests('blocked-unknown'), 'Removed/unknown ID silently used Direct or stale proxy')
        require(not fixture.requests(down_case), 'Delayed dead-transport fallback reached the origin')
        require(sum(r['path'] == '/page' for r in fixture.requests(recovery_case)) == recovery_main_loads,
                'Delayed switch-back fallback navigated the recovered URL through another route')
        report['coverage']['transport_down'] = 'passed'
        report['checks'].append('configured_transport_down_no_direct_fallback_unaffected_a_live')
        report['checks'].append('deleted_connection_cold_restore_fail_closed')
        report['coverage']['cold_restore'] = 'passed'
        report['completed'] = True
    except Exception as error:
        # No credential/raw server receipts are written to artifacts.
        report['error'] = type(error).__name__ + ': ' + str(error)
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
