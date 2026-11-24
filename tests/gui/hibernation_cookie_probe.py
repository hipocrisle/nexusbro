#!/usr/bin/env python3
"""Runner-only native exit/cookie prerequisite, not product hibernation PASS.

Encrypted frames and keys exist only on Python/child pipes and in RAM. The
origin validates real Cookie headers; no CDP, CEF, IPC or OS-exit mocks.
"""
import argparse
import base64
import collections
import hashlib
import http.server
import json
import os
import pathlib
import queue
import re
import secrets
import select
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

import psutil
from cryptography import x509
from cryptography.hazmat.primitives import hashes
from route_ca import generate
from route_fixture import Fixture as RouteFixture

MAX_FRAME = 9 * 1024 * 1024
PROBE_STAGES = {'input','context','anchor','open','parse','restore','readback','verify','admission','target','target_close','cookie_capture','seal','anchor_close','flush','complete'}
COOKIE_FIELDS = {'name','value','domain','path','expires','size','httpOnly','secure','session','sameSite','priority','sourceScheme','sourcePort','partitionKey','partitionKeyOpaque'}

def safe_diagnostic(value):
    fields={'expected_count','actual_count','missing_count','extra_count','parse_refused','mismatch_fields'}
    require(isinstance(value,dict) and set(value)==fields,'native_diagnostic_schema_invalid')
    require(all(type(value[k]) is int and 0<=value[k]<=4096 for k in ('expected_count','actual_count','missing_count','extra_count')),'native_diagnostic_counts_invalid')
    require(type(value['parse_refused']) is bool,'native_diagnostic_boolean_invalid')
    require(isinstance(value['mismatch_fields'],list) and len(value['mismatch_fields'])<=len(COOKIE_FIELDS) and
            all(isinstance(k,str) and k in COOKIE_FIELDS for k in value['mismatch_fields']) and
            len(set(value['mismatch_fields']))==len(value['mismatch_fields']),'native_diagnostic_fields_invalid')
    return dict(value)


def require(condition, stage):
    if not condition:
        raise AssertionError(stage)


class CookieFixture:
    def __init__(self, cert, key):
        self.routes = RouteFixture(cert, key)
        self.lock = threading.Lock()
        self.receipts = []
        self.current = None
        self.counter = 0
        self.tokens = {name: secrets.token_hex(20) for name in ('a', 'b', 'bad')}
        owner = self

        class Origin(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                parsed = urllib.parse.urlsplit(self.path)
                query = urllib.parse.parse_qs(parsed.query)
                name = query.get('case', [''])[0]
                mode = query.get('mode', [''])[0]
                cycle = query.get('cycle', [''])[0]
                if name not in ('a', 'b', 'bad', 'empty') or mode not in ('seed', 'check', 'empty'):
                    self.send_error(404); return
                names = ('session_lax', 'session_strict', 'secure_session', 'persist', 'domain_auth', 'path_auth', 'partitioned_session')
                token = owner.tokens.get(name, '')
                cookies = dict(pair.strip().split('=', 1) for pair in self.headers.get('Cookie', '').split(';') if '=' in pair)
                # CONNECT tunnels preserve TLS byte-for-byte; their proxy lane
                # is proven by the registered upstream TCP source port, not an
                # HTTP header that cannot be injected into encrypted traffic.
                with owner.routes.lock:
                    lane = owner.routes.peers.get(self.client_address[1], 'Direct')
                with owner.lock:
                    active = owner.current
                    owner.receipts.append({'cycle': cycle, 'proof': parsed.path == '/scoped/proof',
                                           'before_admission': active is None or active['cycle'] != cycle or not active['admitted'],
                                           'cookie_matches': {key: cookies.get(key) == token for key in names},
                                           'cookie_present': any(key in cookies for key in names),
                                           'expired_absent': 'expired_auth' not in cookies,
                                           'storage_matches': query.get('storage') == ['true'],
                                           'http_only_hidden': query.get('hidden') == ['true'],
                                           'via_proxy': lane == 'A'})
                if parsed.path == '/scoped/proof':
                    body = b'/* real parser-blocking receipt */'
                    content_type = 'application/javascript'
                else:
                    # A blocking parser script completes before the native load
                    # callback. Unlike fetch/beacon, it cannot race immediate close.
                    storage = 'localStorage.setItem("probe",' + json.dumps(token) + ');' if mode == 'seed' else ''
                    expected = json.dumps(token if name != 'empty' else None)
                    proof = '/scoped/proof?' + urllib.parse.urlencode({'case': name, 'mode': mode, 'cycle': cycle})
                    body = ('<!doctype html><meta charset="utf-8"><script>' + storage +
                            'const ok=localStorage.getItem("probe")===' + expected + ';'
                            'const hidden=document.cookie.length===0;'
                            'document.write("<script src=\\\""+' + json.dumps(proof) +
                            '+"&storage="+ok+"&hidden="+hidden+"\\\"><\\/script>");</script>').encode()
                    content_type = 'text/html'
                self.send_response(200)
                self.send_header('Content-Type', content_type)
                self.send_header('Cache-Control', 'no-store')
                if mode == 'seed' and parsed.path != '/scoped/proof':
                    if name == 'bad':
                        self.send_header('Set-Cookie', 'unspecified=' + token + '; HttpOnly; Path=/')
                    else:
                        for key_ in names:
                            cookie = key_ + '=' + token + '; HttpOnly; SameSite=' + ('Strict' if key_ == 'session_strict' else 'None' if key_ == 'partitioned_session' else 'Lax')
                            cookie += '; Path=' + ('/scoped' if key_ == 'path_auth' else '/')
                            if key_ in ('secure_session','partitioned_session'): cookie += '; Secure'
                            if key_ == 'partitioned_session': cookie += '; Partitioned'
                            if key_ == 'persist': cookie += '; Max-Age=86400'
                            if key_ == 'domain_auth': cookie += '; Domain=127.0.0.1'
                            self.send_header('Set-Cookie', cookie)
                        self.send_header('Set-Cookie', 'expired_auth=' + token + '; HttpOnly; SameSite=Lax; Path=/; Max-Age=2')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                try: self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError): pass

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Origin)
        self.server.daemon_threads = True
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.routes.ports.add(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def begin(self, name, mode, restore):
        with self.lock:
            self.counter += 1
            cycle = str(self.counter)
            self.current = {'cycle': cycle, 'admitted': not restore}
        return cycle, 'https://127.0.0.1:' + str(self.server.server_port) + '/scoped/page?' + urllib.parse.urlencode({'case': name, 'mode': mode, 'cycle': cycle})

    def admit(self, cycle):
        with self.lock:
            require(not any(r['cycle'] == cycle for r in self.receipts), 'origin_requested_before_native_readback_admission')
            require(self.current['cycle'] == cycle, 'admission_cycle_changed')
            self.current['admitted'] = True

    def result(self, cycle):
        with self.lock: return [dict(r) for r in self.receipts if r['cycle'] == cycle]

    def close(self):
        self.server.shutdown(); self.server.server_close(); self.thread.join(timeout=5)
        self.routes.close()


class NativeRunner:
    def __init__(self, exe, cef, root, fixture):
        self.exe, self.cef, self.root, self.fixture = exe, cef, root, fixture
        self.x = None
        self.trusted_thumb = None
        self.env = {**os.environ, 'CEF_PATH': str(cef), 'NEXUSBRO_TEST_DATA': str(root / 'data'), 'NEXUSBRO_PROBA': '0'}
        self.exit_receipts = []
        self.last_probe = {}
        if sys.platform == 'win32':
            self.env['PATH'] = str(cef) + os.pathsep + os.environ.get('PATH', '')
        else:
            self.env.update(LD_LIBRARY_PATH=str(cef), HOME=str(root / 'home'), XDG_CONFIG_HOME=str(root / 'config'), XDG_CACHE_HOME=str(root / 'cache'))
            read_fd, write_fd = os.pipe()
            try:
                self.x = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1024x768x24', '-nolisten', 'tcp', '-noreset'], pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
                os.close(write_fd); write_fd = None
                require(bool(select.select([read_fd], [], [], 20)[0]), 'owned_xvfb_start_timeout')
                number = os.read(read_fd, 32).decode().strip()
                require(number.isdigit(), 'owned_xvfb_invalid_display')
                self.env['DISPLAY'] = ':' + number
            finally:
                os.close(read_fd)
                if write_fd is not None: os.close(write_fd)

    def trust(self, ca):
        if sys.platform == 'win32':
            self.trusted_thumb = x509.load_pem_x509_certificate(ca.read_bytes()).fingerprint(hashes.SHA1()).hex().upper()
            script = self.root / 'trust.ps1'
            script.write_text("param([string]$Certificate,[string]$ExpectedThumbprint)\n" +
                r"$location='Cert:\LocalMachine\Root'; Import-Certificate -FilePath $Certificate -CertStoreLocation $location -Confirm:$false -ErrorAction Stop | Out-Null" + "\n" +
                r"$target=Join-Path $location $ExpectedThumbprint; if (!(Test-Path -LiteralPath $target)) { throw 'Fixture CA not present after import' }" + "\n")
            subprocess.run(['powershell', '-NoProfile', '-NonInteractive', '-File', str(script), '-Certificate', str(ca),
                            '-ExpectedThumbprint', self.trusted_thumb], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
        else:
            database = self.root / 'home' / '.pki' / 'nssdb'; database.mkdir(parents=True)
            subprocess.run(['certutil', '-N', '-d', 'sql:' + str(database), '--empty-password'], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
            subprocess.run(['certutil', '-A', '-d', 'sql:' + str(database), '-n', 'NexusBro probe CA', '-t', 'C,,', '-i', str(ca)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)

    def launch(self, profile, context_key, scope, key, sealed, name, mode, *, expected_refusal=False):
        profile.mkdir(parents=True, exist_ok=True)
        bootstrap = self.root / ('route-' + profile.name + '.json')
        bootstrap.write_text(json.dumps({'inherit': {'key': context_key, 'proxy': 'http://127.0.0.1:' + str(self.fixture.routes.proxies['A']), 'allow_non_proxied_udp': False}, 'bindings': []}))
        cycle, url = self.fixture.begin(name, mode, sealed is not None)
        plan = {'key': key, 'scope': scope, 'sealed': sealed, 'url': url}
        process = subprocess.Popen([str(self.exe), '--nb-dvizhok', '--nb-cookie-probe', '--user-data-dir=' + str(profile), '--nb-route-bootstrap=' + str(bootstrap)], env=self.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        owned = {(process.pid, psutil.Process(process.pid).create_time())}
        self.last_probe = {'ordinal': int(cycle), 'restore_requested': sealed is not None, 'frames_received': 0, 'readback': False, 'final': False, 'shutdown': False}
        frames = queue.Queue()
        stopped = threading.Event()

        def read_stdout():
            try:
                total = 0
                while True:
                    line = process.stdout.readline(MAX_FRAME + 1)
                    if not line: break
                    total += len(line)
                    if len(line) > MAX_FRAME or total > MAX_FRAME * 2:
                        frames.put(('failure', None)); return
                    if line.startswith(b'NB-PROBE '):
                        value = json.loads(line[len(b'NB-PROBE '):])
                        require(isinstance(value, dict), 'native_frame_invalid')
                        frames.put(('frame', value))
                frames.put(('eof', None))
            except Exception:
                frames.put(('failure', None))
            finally:
                stopped.set()

        reader = threading.Thread(target=read_stdout, daemon=True); reader.start()
        readback = False; final = None; shutdown = False
        try:
            payload = json.dumps(plan, separators=(',', ':')).encode() + b'\n'
            require(len(payload) <= MAX_FRAME, 'input_frame_exceeded_bound')
            process.stdin.write(payload); process.stdin.flush()
            # Do NOT close stdin: native restore must await explicit ADMIT.
            deadline = time.monotonic() + 70
            eof = False
            while not eof and time.monotonic() < deadline:
                try:
                    root = psutil.Process(process.pid)
                    owned.update((p.pid, p.create_time()) for p in root.children(recursive=True))
                except psutil.NoSuchProcess: pass
                try: kind, value = frames.get(timeout=.05)
                except queue.Empty: continue
                require(kind != 'failure', 'native_output_frame_invalid')
                if kind == 'eof': eof = True; continue
                self.last_probe['frames_received'] += 1
                if set(value)=={'stage'}:
                    require(isinstance(value['stage'],str) and value['stage'] in PROBE_STAGES,'native_progress_stage_invalid')
                    self.last_probe['stage_count']=self.last_probe.get('stage_count',0)+1
                    require(self.last_probe['stage_count']<=128,'native_progress_count_exceeded')
                    self.last_probe['last_stage']=value['stage']
                    continue
                if set(value)=={'diagnostic'}:
                    require('cookie_diagnostic' not in self.last_probe,'duplicate_native_diagnostic')
                    self.last_probe['cookie_diagnostic']=safe_diagnostic(value['diagnostic'])
                    continue
                if value.get('readback_before_navigation') is True:
                    require(sealed is not None and not readback, 'unexpected_or_duplicate_readback')
                    self.fixture.admit(cycle)
                    process.stdin.write(b'ADMIT\n'); process.stdin.flush()
                    readback = True
                    self.last_probe['readback'] = True
                elif 'sealed' in value:
                    require(final is None and value.get('all_browsers_closed') is True and value.get('cookie_flush_completed') is True,
                            'native_final_close_flush_missing')
                    require(sealed is None or readback, 'native_restore_skipped_readback')
                    require(isinstance(value['sealed'], dict), 'native_sealed_frame_invalid')
                    final = value['sealed']
                    self.last_probe['final'] = True
                elif value.get('cef_shutdown_completed') is True:
                    require(final is not None and not shutdown, 'native_shutdown_before_final_or_duplicate')
                    shutdown = True
                    self.last_probe['shutdown'] = True
                else: raise AssertionError('unknown_native_frame')
            require(eof and process.wait(timeout=10) is not None, 'native_exit_deadline')
            reader.join(timeout=3)
            require(not reader.is_alive(), 'native_pipe_reader_did_not_exit')
            exit_deadline = time.monotonic() + 6
            while True:
                alive = []
                for pid, created in owned:
                    try:
                        p = psutil.Process(pid)
                        if p.create_time() == created and p.is_running(): alive.append(p)
                    except psutil.NoSuchProcess: pass
                if not alive or time.monotonic() >= exit_deadline: break
                time.sleep(.05)
            require(not alive, 'owned_native_tree_survived_exit')
            self.exit_receipts.append({'native_process_exited': True, 'owned_descendants_gone': True,
                                       'readback_admission_barrier': readback, 'all_browsers_closed': final is not None,
                                       'cookie_flush_completed': final is not None, 'cef_shutdown_completed': shutdown})
            if expected_refusal:
                require(process.returncode != 0 and final is None and not shutdown, 'unsupported_cookie_schema_falsely_accepted')
            else:
                require(process.returncode == 0 and final is not None and shutdown, 'native_probe_failed')
            return final, self.fixture.result(cycle)
        finally:
            self.last_probe['native_exit_code'] = process.poll()
            actual_receipts = self.fixture.result(cycle)
            self.last_probe['origin_requests'] = len(actual_receipts)
            self.last_probe['origin_proofs'] = sum(bool(r['proof']) for r in actual_receipts)
            self.last_probe['origin_before_admission_count'] = sum(r['before_admission'] for r in actual_receipts)
            self.last_probe['origin_proxy_verified_count'] = sum(r['via_proxy'] for r in actual_receipts)
            with self.fixture.routes.lock:
                self.last_probe['proxy_connect_count'] = len(self.fixture.routes.tunnels)
            logfile = profile / 'cef.log'
            if logfile.is_file():
                raw = logfile.read_text(errors='replace')
                self.last_probe['native_log_flags'] = {
                    'initialized': 'cef: initialized' in raw,
                    'probe_failed': 'Native cookie prerequisite failed' in raw,
                    'missing_parent': 'нет --nb-roditel' in raw,
                    'bootstrap_refused': 'безопасный маршрут не подготовлен' in raw,
                    'proxy_preference_refused': 'proxy preference was rejected' in raw}
            if process.poll() is None:
                # A failed test may clean up a whole owned tree. This can never
                # create a success receipt or authorize a replacement writer.
                try:
                    children = psutil.Process(process.pid).children(recursive=True)
                    for child in children: child.kill()
                    process.kill(); psutil.wait_procs(children, timeout=5)
                except psutil.NoSuchProcess: pass
                process.wait(timeout=10)
            process.stdin.close(); process.stdout.close()
            reader.join(timeout=3)

    def close(self):
        if self.trusted_thumb:
            script = self.root / 'untrust.ps1'
            script.write_text("param([string]$Thumbprint)\n" +
                r"$target=Join-Path 'Cert:\LocalMachine\Root' $Thumbprint; if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Force -ErrorAction Stop }; if (Test-Path -LiteralPath $target) { throw 'Fixture CA still installed after cleanup' }" + "\n")
            subprocess.run(['powershell', '-NoProfile', '-NonInteractive', '-File', str(script), '-Thumbprint', self.trusted_thumb], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
            self.trusted_thumb = None
        if self.x:
            self.x.terminate(); self.x.wait(timeout=5)


def check_receipts(receipts, *, authenticated, storage):
    require(receipts and any(r['proof'] for r in receipts), 'native_origin_or_blocking_storage_proof_missing')
    require(all(not r['before_admission'] and r['via_proxy'] for r in receipts), 'origin_before_admission_or_proxy_bypass')
    proofs = [r for r in receipts if r['proof']]
    require(all(r['storage_matches'] == storage and r['http_only_hidden'] for r in proofs), 'native_storage_or_httponly_visibility_failed')
    if authenticated:
        require(all(all(r['cookie_matches'].values()) for r in receipts), 'native_cookie_header_roundtrip_failed')
        require(all(r['expired_absent'] for r in receipts), 'expired_cookie_resurrected')
    else:
        require(all(not r['cookie_present'] for r in receipts), 'other_context_or_empty_jar_received_auth')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', required=True); parser.add_argument('--cef', required=True); parser.add_argument('--out', required=True)
    args = parser.parse_args()
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'ephemeral_runner_required')
    out = pathlib.Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    report = {'schema': 1, 'completed': False, 'product_hibernation_accepted': False, 'platform': sys.platform,
              'checks': [], 'coverage': {'transport': 'real_proxy_and_trusted_https', 'session_cookie_restore': 'not_executed'}}
    runner = fixture = None; code = 1; stage = 'setup'; private = None
    try:
        exe, cef = pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve()
        require(exe.is_file() and cef.is_dir(), 'native_files_missing')
        report['exe_sha256'] = hashlib.sha256(exe.read_bytes()).hexdigest()
        private = tempfile.mkdtemp(prefix='nb-cookie-prerequisite-')
        root = pathlib.Path(private)
        certs = generate(root / 'tls')
        fixture = CookieFixture(certs / 'server.crt', certs / 'server.key')
        runner = NativeRunner(exe, cef, root, fixture); runner.trust(certs / 'ca.crt')
        report['tls_fixture_trust_verified'] = True
        keys = {name: base64.b64encode(secrets.token_bytes(32)).decode() for name in ('a', 'b', 'empty', 'bad')}
        context_keys = {name: secrets.token_hex(32) for name in keys}
        scopes = {name: {'profile_key': secrets.token_hex(32), 'context_key': context_keys[name], 'generation': 1, 'attempt': 1} for name in keys}
        def launch(name, mode, sealed=None, **extra):
            return runner.launch(root / ('profile-' + name), context_keys[name], scopes[name], keys[name], sealed, name, mode, **extra)
        stage = 'empty-cookie-native-roundtrip'
        empty, _ = launch('empty', 'empty')
        _, receipts = launch('empty', 'empty', empty)
        check_receipts(receipts, authenticated=False, storage=True)
        report['checks'].append('empty_cookie_jar_readback_before_explicit_admission_and_real_native_tree_exit')
        stage = 'session-secure-persistent-path-domain-cookie-native-capture'
        first, first_receipts = launch('a', 'seed')
        require(any(r['proof'] for r in first_receipts), 'seed_storage_proof_missing')
        time.sleep(3)  # Real expiry while engine has already exited.
        stage = 'session-secure-persistent-native-restore'
        _, receipts = launch('a', 'check', first)
        check_receipts(receipts, authenticated=True, storage=True)
        report['checks'].append('new_native_process_server_headers_session_lax_strict_secure_persistent_path_domain_CHIPS_and_expiry_storage_preserved')
        stage = 'second-context-isolation'
        _, receipts = launch('b', 'empty')
        check_receipts(receipts, authenticated=False, storage=False)
        second, _ = launch('b', 'seed')
        time.sleep(3)
        _, receipts = launch('b', 'check', second)
        check_receipts(receipts, authenticated=True, storage=True)
        _, receipts = launch('a', 'check', first)
        check_receipts(receipts, authenticated=True, storage=True)
        report['checks'].append('separate_native_profile_context_keys_do_not_copy_session_auth_and_preserve_original_on_return')
        stage = 'cross-context-sealed-transplant-refusal'
        _, receipts = runner.launch(root / 'profile-b', context_keys['b'], scopes['b'], keys['a'], first,
                                    'b', 'check', expected_refusal=True)
        require(not receipts, 'wrong_context_envelope_reached_network_before_refusal')
        report['checks'].append('cross_context_sealed_envelope_refused_before_any_origin_request')
        stage = 'unspecified-samesite-session-refusal'
        _, receipts = launch('bad', 'seed', expected_refusal=True)
        require(any(r['proof'] for r in receipts), 'unsupported_cookie_never_reached_native_fixture')
        report['checks'].append('unspecified_samesite_session_cookie_refused_without_sealed_success_or_shutdown_ack')
        report['native_exit_receipts'] = runner.exit_receipts
        report['coverage']['session_cookie_restore'] = 'passed'
        report['coverage']['first_party_CHIPS_session'] = 'server_header_roundtrip_required_and_passed'
        report['completed'] = True; code = 0
        runner.close(); runner = None
        fixture.close(); fixture = None
    except Exception as error:
        report.update(failure_stage=stage, failure_class=type(error).__name__)
        # All AssertionError codes are fixed literals owned by this harness.
        # Never serialize subprocess errors/native frames or arbitrary strings.
        allowed = {'native_probe_failed','native_output_frame_invalid','native_frame_invalid','unexpected_or_duplicate_readback',
                   'native_final_close_flush_missing','native_restore_skipped_readback','native_sealed_frame_invalid',
                   'native_shutdown_before_final_or_duplicate','unknown_native_frame','native_exit_deadline',
                   'native_pipe_reader_did_not_exit','owned_native_tree_survived_exit','unsupported_cookie_schema_falsely_accepted',
                   'origin_requested_before_native_readback_admission','admission_cycle_changed','native_origin_or_blocking_storage_proof_missing',
                   'origin_before_admission_or_proxy_bypass','native_storage_or_httponly_visibility_failed','native_cookie_header_roundtrip_failed',
                   'expired_cookie_resurrected','other_context_or_empty_jar_received_auth','native_progress_stage_invalid',
                   'native_progress_count_exceeded','duplicate_native_diagnostic','native_diagnostic_schema_invalid',
                   'native_diagnostic_counts_invalid','native_diagnostic_boolean_invalid','native_diagnostic_fields_invalid'}
        report['failure_code'] = str(error) if type(error) is AssertionError and str(error) in allowed else 'unclassified'
        if runner: report['last_native_probe'] = runner.last_probe
    finally:
        for name, obj in [('native', runner), ('fixture', fixture)]:
            if obj:
                try: obj.close()
                except Exception as error:
                    report.setdefault('cleanup_failure_classes', {})[name] = type(error).__name__
                    report['completed'] = False; code = 1
        if private:
            shutil.rmtree(private, ignore_errors=True)
        (out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'completed': report['completed'], 'product_hibernation_accepted': False}))
    return code


if __name__ == '__main__':
    sys.exit(main())
