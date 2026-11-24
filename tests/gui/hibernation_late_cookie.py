#!/usr/bin/env python3
"""Real final-cookie capture prerequisite. Isolated GitHub desktop only.

A held HTTPS response cannot issue its unpredictable session cookie until the
parent reports its INITIAL prepared snapshot. No controller CDP, forged ACK or
injected lifecycle state is used. A protected pending fetch is a blocker.
"""
import argparse
import http.server
import json
import os
import pathlib
import secrets
import ssl
import sys
import threading
import time
import urllib.parse

from security_native import Device, call, require, super_record, wait_for
from tab_routes import WindowsDevice, click_control, switch_super, rows
from resource_soak import foreground
from route_fixture import Fixture as Routes
from super_hibernation_native import snapshot, identities, gone


PAGE_SCRIPT = """const expected=EXPECTED;
SEED_STORAGE
function report(){fetch('/proof?'+new URLSearchParams({storage:String(localStorage.getItem('late-fixture')===expected)}),{cache:'no-store'}).catch(()=>{});}
addEventListener('load',()=>{report(); START_HELD});
"""


class Fixture:
    def __init__(self, cert, key):
        self.routes = Routes(cert, key)
        self.lock = threading.Lock()
        self.tokens = {lane: secrets.token_hex(24) for lane in ('A', 'B')}
        self.seeded = set()
        self.receipts = []
        self.held_started = threading.Event()
        self.release = threading.Event()
        self.held_sent = threading.Event()
        self.late = None
        self.initial_prepared_observed = False
        self.released_after_prepared = False
        owner = self

        class Origin(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                path = urllib.parse.urlsplit(self.path).path
                query = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
                with owner.routes.lock:
                    lane = owner.routes.peers.get(self.client_address[1], 'Direct')
                cookie = self.headers.get('Cookie', '')
                parsed_cookie = {}
                for part in cookie.split(';'):
                    key, separator, value = part.strip().partition('=')
                    if separator:
                        parsed_cookie[key] = value
                with owner.lock:
                    token = owner.tokens.get(lane)
                    auth = token is not None and secrets.compare_digest(parsed_cookie.get('initial_auth', ''), token)
                    late = owner.late is not None and secrets.compare_digest(parsed_cookie.get('late_auth', ''), owner.late)
                    owner.receipts.append({'page': path == '/page', 'proof': path == '/proof',
                                           'held': path == '/held', 'lane': lane,
                                           'initial': auth, 'late': late,
                                           'storage': query.get('storage') == ['true']})
                if path == '/held':
                    if lane != 'A':
                        self.send_error(403)
                        return
                    owner.held_started.set()
                    if not owner.release.wait(100):
                        self.send_error(408)
                        return
                    with owner.lock:
                        late_value = owner.late
                        permitted = owner.initial_prepared_observed and owner.released_after_prepared and late_value is not None
                    if not permitted:
                        self.send_error(409)
                        return
                    body = b'late cookie delivered'
                    try:
                        self.send_response(200)
                        self.send_header('Set-Cookie', 'late_auth=' + late_value + '; Secure; HttpOnly; SameSite=Lax; Path=/')
                        self.send_header('Content-Type', 'text/plain')
                        self.send_header('Cache-Control', 'no-store')
                        self.send_header('Content-Length', str(len(body)))
                        self.end_headers()
                        self.wfile.write(body)
                        self.wfile.flush()
                        owner.held_sent.set()
                    except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                        pass
                    return
                if path == '/proof':
                    body = b'ok'
                    seeded = False
                elif path == '/page' and lane in owner.tokens:
                    with owner.lock:
                        seeded = lane not in owner.seeded
                        owner.seeded.add(lane)
                    script = PAGE_SCRIPT.replace('EXPECTED', json.dumps(token))
                    script = script.replace('SEED_STORAGE', "localStorage.setItem('late-fixture',expected);" if seeded else '')
                    script = script.replace('START_HELD', "setTimeout(()=>fetch('/held',{cache:'no-store'}).then(report).catch(()=>{}),100);" if seeded and lane == 'A' else '')
                    body = ('<!doctype html><meta charset="utf-8"><title>Final cookie fixture</title>'
                            '<style>body{background:#195541;min-height:900px}</style><script>' + script + '</script>').encode()
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                if seeded:
                    self.send_header('Set-Cookie', 'initial_auth=' + token + '; Secure; HttpOnly; SameSite=Lax; Path=/')
                self.send_header('Content-Type', 'text/html' if path == '/page' else 'text/plain')
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                try:
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                    pass

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Origin)
        self.server.daemon_threads = True
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.routes.ports.add(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = 'https://127.0.0.1:' + str(self.server.server_port) + '/page'

    def release_after_prepared(self):
        require(self.held_started.is_set(), 'Held request never started')
        with self.lock:
            require(self.late is None and not self.release.is_set(), 'Late response was released early')
            self.initial_prepared_observed = True
            self.late = secrets.token_hex(32)  # did not exist at the initial snapshot
            self.released_after_prepared = True
        self.release.set()

    def records(self):
        with self.lock:
            return [dict(r) for r in self.receipts]

    def close(self):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        self.routes.close()


def run(args):
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Isolated GitHub runner required')
    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {'schema': 1, 'completed': False, 'final_snapshot_accepted': False,
              'test_idle_seconds': 5, 'commit_delay_ms': 5000, 'normal_policy_minutes': 30}
    device = fixture = None
    stage = 'setup'
    started = time.monotonic()
    # A real wall-clock deadline: only exits the isolated runner test process.
    def deadline():
        payload = dict(report, completed=False, final_snapshot_accepted=False,
                       failure_stage='wall_clock_deadline', failure_class='TimeoutError')
        (out / 'results.json').write_text(json.dumps(payload, indent=2) + '\n')
        os._exit(124)
    timer = threading.Timer(175, deadline)
    timer.daemon = True
    timer.start()
    previous = {name: os.environ.get(name) for name in
                ('NEXUSBRO_TEST_SUPER_IDLE_SECONDS', 'NEXUSBRO_TEST_HIB_COMMIT_DELAY_MS')}
    code = 1
    try:
        fixture = Fixture(args.tls_cert, args.tls_key)
        root = out / 'private-device'
        exe, cef = pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve()
        device = WindowsDevice(root, exe, cef) if sys.platform == 'win32' else Device('late-cookie', root, exe, cef)
        supers = []
        for index, lane in enumerate(('A', 'B')):
            identifier = 'late-' + lane.lower()
            item = super_record(identifier, 'Final cookie ' + lane, fixture.url,
                                proxy='http://127.0.0.1:' + str(fixture.routes.proxies[lane]))
            item['poryadok'] = index
            supers.append(item)
            folder = device.data / 'profili' / identifier
            folder.mkdir(parents=True)
            (folder / 'obychnye.json').write_text(json.dumps({'vkladki': [{'id': 1, 'adres': fixture.url,
                'zagolovok': 'Fixture', 'otlozhena': True}], 'aktivnaya': 1, 'sleduyushchaya': 2}))
        (device.data / 'vkladki.json').write_text(json.dumps(supers))
        (device.data / 'sohranennye.json').write_text(json.dumps(supers))
        (device.data / 'nastrojki.json').write_text(json.dumps({'avtoobnovleniya': False, 'apparatnoe_uskorenie': False}))
        os.environ['NEXUSBRO_TEST_SUPER_IDLE_SECONDS'] = '5'
        os.environ['NEXUSBRO_TEST_HIB_COMMIT_DELAY_MS'] = '5000'
        client = device.start()
        def activate(identifier):
            foreground(device)
            switch_super(client, identifier)
            foreground(device)
            if client.js('!![...document.querySelectorAll(".pusto button")].find(e=>e.textContent.trim()==="Загрузить вкладку")'):
                click_control(client, '.pusto button', 20)
            wait_for(lambda: snapshot(client).get(identifier, {}).get('phase') == 'Running', 'Engine never running', 25)
        stage = 'initial_native_load'
        activate('late-a')
        wait_for(lambda: fixture.held_started.is_set(), 'Held network request absent', 12)
        wait_for(lambda: any(r['proof'] and r['lane'] == 'A' and r['initial'] and r['storage'] for r in fixture.records()), 'Initial auth/storage receipt absent', 12)
        current = rows(client, 'late-a')
        page = next(r for r in current['vkladki'] if r['id'] == current['aktivnaya'])
        require(not page.get('nazad') and not page.get('vpered'), 'Fixture accidentally created history')
        before = snapshot(client)['late-a']
        require(before['pid'] is not None and before['profile_reserved'] and before['lease_confirmed'], 'Initial writer lease absent')
        owners = identities(before['pid'])
        stage = 'await_initial_prepared_while_response_held'
        activate('late-b')
        def prepared():
            row = snapshot(client)['late-a']
            return row if row['phase'] == 'Preparing' and row.get('prepared_snapshot') is True else None
        wait_for(prepared, 'Pending fetch protected or no initial prepared snapshot; not accepted', 25)
        prepared_observed_at = time.monotonic()
        require(not fixture.release.is_set() and fixture.late is None, 'Late response existed before initial snapshot')
        report['initial_snapshot_observed_before_response_release'] = True
        stage = 'release_new_cookie_after_initial_snapshot'
        fixture.release_after_prepared()
        require(fixture.held_sent.wait(3), 'Held HTTPS response did not complete before commit delay')
        still_prepared = snapshot(client)['late-a']
        require(still_prepared['phase'] == 'Preparing' and still_prepared.get('prepared_snapshot') is True,
                'Cookie response was not delivered during the held initial-snapshot phase')
        report['new_cookie_response_sent_after_initial_snapshot'] = True
        stage = 'await_real_sleep_and_old_tree_exit'
        sleeping = wait_for(lambda: (lambda r: r if r['phase'] == 'Sleeping' else None)(snapshot(client)['late-a']), 'Final snapshot stop not accepted', 35)
        report['prepared_to_sleep_seconds'] = round(time.monotonic() - prepared_observed_at, 3)
        require(sleeping['pid'] is None and not sleeping['profile_reserved'] and not sleeping['lease_confirmed'], 'Sleeping still owns lease')
        wait_for(lambda: gone(owners), 'Old native tree remains alive', 8)
        report['old_tree_gone_before_wake'] = True
        receipts_before = len(fixture.records())
        stage = 'physical_wake_new_generation'
        activate('late-a')
        after = snapshot(client)['late-a']
        require(after['generation'] > before['generation'] and after['pid'] is not None, 'Wake generation did not advance')
        stage = 'origin_header_and_storage_final_snapshot_proof'
        proofs = wait_for(lambda: [r for r in fixture.records()[receipts_before:] if r['proof'] and r['lane'] == 'A'] or None, 'Woken page never reached origin', 18)
        require(all(r['initial'] and r['late'] and r['storage'] for r in proofs), 'Final snapshot lost initial auth, late session cookie or storage')
        receipts = fixture.records()
        require(all(r['lane'] != 'Direct' for r in receipts), 'Configured route used Direct fallback')
        require(all(not r['late'] for r in receipts if r['lane'] == 'B'), 'Late session cookie crossed context route')
        require(any(r['proof'] and r['lane'] == 'B' and r['initial'] and r['storage'] for r in receipts), 'Independent B context proof absent')
        report.update(completed=True, final_snapshot_accepted=True,
                      proof_count=len(proofs), direct_count=0, crossed_cookie_count=0,
                      generation_advanced=True, response_completed_during_initial_prepared_hold=True)
        code = 0
    except Exception as error:
        report.update(failure_stage=stage, failure_class=type(error).__name__)
        if stage == 'await_initial_prepared_while_response_held':
            report['blocker'] = 'pending_fetch_or_preparation_refused'
        if device and device.client:
            try:
                state = snapshot(device.client)
                report['failure_phase_counts'] = {phase: sum(r['phase'] == phase for r in state.values()) for phase in ('Running', 'Preparing', 'Stopping', 'Sleeping', 'Starting')}
            except Exception as diagnostic:
                report['diagnostic_failure_class'] = type(diagnostic).__name__
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
        if device:
            try:
                device.stop_app()
            except Exception as error:
                report.update(completed=False, final_snapshot_accepted=False, cleanup_failure_class=type(error).__name__)
                code = 1
        if fixture:
            try:
                fixture.close()
            except Exception as error:
                report.update(completed=False, final_snapshot_accepted=False, fixture_cleanup_failure_class=type(error).__name__)
                code = 1
        timer.cancel()
        report['wall_seconds'] = round(time.monotonic() - started, 3)
        (out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'completed': report['completed'], 'final_snapshot_accepted': report['final_snapshot_accepted']}))
    return code


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('exe', 'cef', 'out', 'tls-cert', 'tls-key'):
        parser.add_argument('--' + name, required=True)
    sys.exit(run(parser.parse_args()))
