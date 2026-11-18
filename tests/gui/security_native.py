"""GitHub-only native security acceptance: two Tauri apps + real CEF + sync server.

No Tauri/HTTP/crypto mocks. Each app has its own D-Bus session, X display,
inspector endpoint and data/config/cache roots. All credentials are generated
synthetic fixtures; only boolean checkpoints enter results.json. Website probes
stay in memory. No deployment or external endpoint is used.
"""
import argparse
import base64
import hashlib
import http.server
import json
import os
import pathlib
import secrets
import select
import shutil
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request

import psutil
import websocket
from client import Client


class NativeFailure(AssertionError):
    pass


def require(condition, message):
    if not condition:
        raise NativeFailure(message)


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait_for(check, message, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(.1)
    raise NativeFailure(message)


def js_wait(client, expression, message, timeout=20):
    return wait_for(lambda: client.js(expression), message, timeout)


def painted(client):
    client.js('window.__nativePainted=false;requestAnimationFrame(()=>requestAnimationFrame(()=>window.__nativePainted=true));true')
    js_wait(client, 'window.__nativePainted', 'Visible browser did not paint its updated interface')


def call(client, command, *, error=False, timeout=40, **args):
    client.js('window.__securityNativeResult=null;window.__TAURI_INTERNALS__.invoke(' +
              json.dumps(command) + ',' + json.dumps(args) +
              ').then(r=>window.__securityNativeResult={r},e=>window.__securityNativeResult={error:String(e)});true')
    result = js_wait(client, 'window.__securityNativeResult', 'Native IPC did not finish: ' + command, timeout)
    if error:
        require('error' in result, 'Native operation unexpectedly accepted: ' + command)
        return result['error']
    require('error' not in result, 'Native IPC rejected ' + command + ': ' + str(result.get('error', '')))
    value = result.get('r')
    if isinstance(value, dict) and 'ok' in value:
        require(value['ok'], 'Native browser operation failed: ' + command)
    return value


def settings(theme, marker):
    return {'tema': theme, 'shrift': 'system', 'razmer_shrifta': 14, 'plotnost': 'udobno',
            'masshtab_stranic': 100, 'panel_shirina': 180, 'panel_svernuta': False,
            'panel_zakladok': False, 'novaya_stranica': 'about:blank', 'poisk': 'duckduckgo',
            'avtoobnovleniya': False, 'apparatnoe_uskorenie': False, 'yazyk_perevoda': 'ru',
            'vyhody': [{'id': marker, 'nazvanie': 'Synthetic local connection',
                        'adres': 'https://' + marker + '.fixture.invalid/subscription', 'proxy_uzel': ''}]}


def super_record(identifier, name, address, *, proxy='', private=False):
    return {'id': identifier, 'nazvanie': name, 'cvet': '#5b9ee8', 'proxy': proxy,
            'proxy_uzel': '', 'adres': address, 'rezat_reklamu': True, 'pustaya': False,
            'privatnaya': private, 'poryadok': 0}


class Device:
    def __init__(self, name, root, exe, cef):
        self.name, self.root, self.exe, self.cef = name, root, exe, cef
        self.data = root / 'data'
        self.data.mkdir(parents=True)
        self.inspector = port()
        self.x = self.app = self.client = None
        self.display = None
        self.logs = []
        self.owned_pids = set()
        self.launches = 0

    def log(self, name):
        stream = (self.root / name).open('wb')
        self.logs.append(stream)
        return stream

    def start(self):
        if self.x is None:
            read_fd, write_fd = os.pipe()
            try:
                self.x = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                                           '1280x900x24', '-nolisten', 'tcp'], pass_fds=(write_fd,),
                                          stdout=self.log('xvfb.log'), stderr=subprocess.STDOUT,
                                          start_new_session=True)
                os.close(write_fd)
                write_fd = None
                ready, _, _ = select.select([read_fd], [], [], 8)
                require(ready, 'Xvfb did not allocate a display')
                number = os.read(read_fd, 32).decode().strip()
                require(number.isdigit(), 'Xvfb display allocation failed')
                self.display = ':' + number
            finally:
                os.close(read_fd)
                if write_fd is not None:
                    os.close(write_fd)
        self.launches += 1
        env = {**os.environ, 'DISPLAY': self.display, 'CEF_PATH': str(self.cef),
               'LD_LIBRARY_PATH': str(self.cef), 'NEXUSBRO_TEST_DATA': str(self.data),
               'NEXUSBRO_PROBA': '0', 'WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS': '1',
               'WEBKIT_INSPECTOR_HTTP_SERVER': '127.0.0.1:' + str(self.inspector),
               'XDG_CONFIG_HOME': str(self.root / 'config'),
               'XDG_DATA_HOME': str(self.root / 'xdg-data'),
               'XDG_CACHE_HOME': str(self.root / 'cache'),
               'NO_PROXY': '127.0.0.1,localhost,::1', 'no_proxy': '127.0.0.1,localhost,::1'}
        env.pop('NEXUSBRO_TEST_TRANSLATE_URL', None)
        if hasattr(self, 'sync_url'):
            env['NEXUSBRO_TEST_SYNC_URL'] = self.sync_url
        self.app = subprocess.Popen(['dbus-run-session', '--', str(self.exe)], env=env,
                                    stdout=self.log('app-' + str(self.launches) + '.log'),
                                    stderr=subprocess.STDOUT, start_new_session=True)
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            require(self.app.poll() is None, 'Native application exited during startup')
            try:
                self.client = Client(display=self.display, port=self.inspector)
                break
            except (ConnectionError, OSError, websocket.WebSocketException):
                time.sleep(.2)
        require(self.client is not None, 'WebKit inspector did not start')
        js_wait(self.client, '!!window.__TAURI_INTERNALS__?.invoke', 'Tauri IPC did not initialize', 20)
        return self.client

    def stop_app(self):
        if self.client:
            try:
                self.client.close()
            except Exception:
                pass
            self.client = None
        if not self.app:
            return
        try:
            process = psutil.Process(self.app.pid)
            children = process.children(recursive=True)
            self.owned_pids.update(p.pid for p in children)
        except psutil.NoSuchProcess:
            children = []
        for child in reversed(children):
            try:
                child.terminate()
            except psutil.NoSuchProcess:
                pass
        try:
            os.killpg(self.app.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            self.app.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(self.app.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.app.wait(timeout=5)
        _, alive = psutil.wait_procs(children, timeout=3)
        for child in alive:
            try:
                child.kill()
            except psutil.NoSuchProcess:
                pass
        psutil.wait_procs(alive, timeout=3)
        self.app = None

    def close(self):
        self.stop_app()
        if self.x:
            self.x.terminate()
            try:
                self.x.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.x.kill()
                self.x.wait(timeout=3)
            self.x = None
        for stream in self.logs:
            stream.close()
        for pid in self.owned_pids:
            # Only private directories belonging to descendants we launched.
            private = pathlib.Path(tempfile.gettempdir()) / ('NexusBro-private-' + str(pid))
            if private.is_dir() and not private.is_symlink():
                shutil.rmtree(private)

    def loaded(self, super_id, *, address=None, tab_id=None, private=None):
        def ready():
            require(self.app.poll() is None, 'Native application exited')
            try:
                session = call(self.client, 'obychnye_spisok', id=super_id)
            except NativeFailure as error:
                if 'ещё запускается' in str(error) or 'еще запускается' in str(error):
                    return None
                raise
            for tab in session.get('vkladki', []):
                if tab['id'] != session.get('aktivnaya') or tab.get('zagolovok') != 'SecurityNativeFixture' or tab.get('zagruzka'):
                    continue
                if address is not None and tab.get('adres') != address:
                    continue
                if tab_id is not None and tab['id'] != tab_id:
                    continue
                if private is not None and bool(tab.get('privatnaya')) != private:
                    continue
                return session
            return None
        return wait_for(ready, 'CEF fixture did not load', 30)

    def navigate(self, super_id, address):
        session = call(self.client, 'obychnye_spisok', id=super_id)
        call(self.client, 'obychnaya_komanda', id=super_id,
             komanda={'deystvie': 'pereyti', 'id': session['aktivnaya'], 'adres': address})
        return self.loaded(super_id, address=address)

    def submit(self):
        c = self.client
        call(c, 'pokazat_dvizhki', vidno=True)
        point = js_wait(c, '(()=>{const e=document.querySelector(".pole");if(!e)return null;const r=e.getBoundingClientRect();return r.width>400&&r.height>220?[r.x+200,r.y+160]:null})()',
                        'CEF content area is too small for trusted submit')
        c.mouse(*point)

    def cipher_files(self):
        paths = [self.data / 'security/browser-vault.v1', self.data / 'security/browser-vault.v1.bak']
        require(all(p.is_file() for p in paths), 'Encrypted vault main/backup missing')
        return paths


def run(args):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise NativeFailure('Native runtime is intentionally restricted to GitHub Actions')
    root = pathlib.Path(args.out).resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    exe, cef, server_binary = map(lambda p: pathlib.Path(p).resolve(), (args.exe, args.cef, args.server))
    require(exe.is_file() and server_binary.is_file() and cef.is_dir(), 'Supply built app/server and CEF resources')
    checks, probes, processes, streams, devices = [], [], [], [], []
    sensitive = set()
    phase = 'startup'
    fixture = None
    failure = None
    server_process = None
    suffix = secrets.token_hex(6)
    master_a, master_b = 'native-master-A-' + suffix, 'native-master-B-' + suffix
    account_password = 'native-account-password-' + suffix
    wrong_master = 'wrong-master-fixture-' + suffix
    login, site_password = 'native-site-login-' + suffix, 'native-site-password-' + suffix
    offer_login, offer_password = 'native-offer-login-' + suffix, 'native-offer-password-' + suffix
    sensitive.update((master_a, master_b, wrong_master, account_password, login, site_password, offer_login, offer_password))
    forms = {'prefill': (login, site_password), 'offer': (offer_login, offer_password),
             'private': ('private-login-' + suffix, 'private-password-' + suffix), 'empty': ('', '')}
    sensitive.update(forms['private'])
    probe_lock = threading.Lock()

    class Page(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            if self.path != '/probe':
                self.send_response(404)
                self.end_headers()
                return
            size = int(self.headers.get('Content-Length', '0'))
            if size > 32768:
                self.send_response(413)
                self.end_headers()
                return
            try:
                data = json.loads(self.rfile.read(size))
                require(isinstance(data, dict), 'Invalid fixture probe')
                data['cookie_header'] = self.headers.get('Cookie', '')
                with probe_lock:
                    probes.append(data)
            except (ValueError, NativeFailure):
                self.send_response(400)
                self.end_headers()
                return
            self.send_response(204)
            self.end_headers()

        def do_GET(self):
            q = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
            path = urllib.parse.urlsplit(self.path).path
            if path == '/favicon.ico':
                self.send_response(204)
                self.end_headers()
                return
            if path == '/v1/health':
                body = b'{"service":"not-the-account-server","protocol":1}'
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if path != '/form':
                self.send_response(404)
                self.end_headers()
                return
            case = q.get('case', [''])[0]
            username, password = forms.get(q.get('mode', ['empty'])[0], ('', ''))
            script = '''const marker=CASE,u=document.querySelector('#login'),p=document.querySelector('#password');
 u.value=LOGIN;p.value=PASSWORD;
 const report=(event,trusted)=>fetch('/probe',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({case:marker,event,trusted:!!trusted,username:u.value,password:p.value})});
 for(const e of [u,p])for(const kind of ['input','change'])e.addEventListener(kind,event=>report(kind,event.isTrusted));
 document.querySelector('form').addEventListener('submit',event=>{event.preventDefault();report('submit',event.isTrusted)});
 report('ready',false);'''.replace('CASE', json.dumps(case)).replace('LOGIN', json.dumps(username)).replace('PASSWORD', json.dumps(password))
            body = ('''<!doctype html><meta charset="utf-8"><title>SecurityNativeFixture</title>
<style>body{margin:0;background:#185541;color:white}form{position:absolute;left:40px;top:40px;width:320px}input{display:block;box-sizing:border-box;width:320px;height:36px;margin:0 0 14px;padding:4px;font-size:16px}button{display:block;box-sizing:border-box;width:320px;height:40px;padding:0;font-size:16px}</style>
<form><input id="login" type="text" autocomplete="username"><input id="password" type="password" autocomplete="current-password"><button type="submit">Synthetic sign in</button></form><a href="/form?case=private-link-opened&amp;mode=empty" style="position:absolute;left:40px;top:250px;color:white">Middle link fixture</a><script>''' + script + '</script>').encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/html;charset=utf-8')
            self.send_header('Cache-Control', 'no-store')
            if q.get('set_cookie') == ['1']:
                self.send_header('Set-Cookie', 'native-source=only-A; Path=/; SameSite=Lax; HttpOnly')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def seen(case, event='ready'):
        with probe_lock:
            return [p for p in probes if p.get('case') == case and p.get('event') == event]

    def checkpoint(name):
        checks.append({name: True})

    def report_ok(report):
        require(not report.get('conflicts') and not report.get('errors'), 'Sync has unresolved conflicts/errors')

    def set_theme(device, theme):
        n = call(device.client, 'nastrojki_poluchit')
        n['tema'] = theme
        call(device.client, 'nastrojki_sohranit', nastrojki=n)

    def check_encrypted(device):
        for path in device.cipher_files():
            raw = path.read_bytes()
            value = json.loads(raw)
            require(value.get('header', {}).get('format') == 'nexusbro-vault', 'Vault envelope format missing')
            require(value.get('header', {}).get('version') == 1, 'Unexpected vault envelope version')
            require(bool(base64.b64decode(value['payload'])) and bool(base64.b64decode(value['wrapped_key'])), 'Vault ciphertext missing')
            for secret in sensitive:
                require(secret.encode() not in raw, 'Plaintext credential found in vault envelope')
                require(secret.encode() not in base64.b64decode(value['payload']), 'Plaintext credential found in decoded payload')
            require(path.stat().st_mode & 0o777 == 0o600, 'Vault file permissions are not private')

    previous_alarm = signal.getsignal(signal.SIGALRM)
    def runtime_expired(_signal, _frame):
        raise NativeFailure('Native acceptance exceeded its eight-minute runtime budget')
    signal.signal(signal.SIGALRM, runtime_expired)
    signal.alarm(480)
    try:
        fixture = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Page)
        threading.Thread(target=fixture.serve_forever, daemon=True).start()
        base = 'http://127.0.0.1:' + str(fixture.server_port)
        sync_root = root / 'sync-server'
        sync_root.mkdir(mode=0o700)
        sync_port = port()
        sync_url = 'http://127.0.0.1:' + str(sync_port)
        stream = (sync_root / 'server.log').open('wb')
        streams.append(stream)
        deployment_key = base64.b64encode(secrets.token_bytes(32)).decode()
        sensitive.add(deployment_key)
        server_process = subprocess.Popen([str(server_binary)], env={**os.environ,
            'NEXUSBRO_SYNC_BIND': '127.0.0.1:' + str(sync_port),
            'NEXUSBRO_SYNC_DB': str(sync_root / 'nexusbro-sync.db'),
            'NEXUSBRO_SYNC_TOTP_KEY': deployment_key,
            'RUST_LOG': 'info'}, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        processes.append(server_process)

        def health():
            require(server_process.poll() is None, 'Isolated sync server exited')
            try:
                with urllib.request.urlopen(sync_url + '/v1/health', timeout=1) as response:
                    result = json.load(response)
                return result.get('service') == 'nexusbro-sync' and result.get('protocol') == 1
            except (OSError, ValueError):
                return False
        wait_for(health, 'Isolated browser sync service did not become healthy', 20)
        checkpoint('isolated_real_sync_server_health')
        a = Device('A', root / 'device-a', exe, cef)
        b = Device('B', root / 'device-b', exe, cef)
        devices.extend((a, b))
        a.sync_url = b.sync_url = sync_url
        for device, theme, marker in [(a, 'grafit', 'only-device-a'), (b, 'svetlaya', 'only-device-b')]:
            (device.data / 'nastrojki.json').write_text(json.dumps(settings(theme, marker)))
        # Same saved record on two devices has different portable metadata and
        # different local proxy values: explicit remote resolution must retain B's proxy.
        saved_a = super_record('retained-super', 'Cloud metadata', 'about:blank', proxy='socks5://127.0.0.1:31001')
        saved_b = super_record('retained-super', 'Keep local until confirmed', 'about:blank', proxy='socks5://127.0.0.1:31002')
        private_saved = super_record('pvt-native-private', 'Never sync private definition', 'about:blank', private=True)
        (a.data / 'sohranennye.json').write_text(json.dumps([saved_a, private_saved]))
        (b.data / 'sohranennye.json').write_text(json.dumps([saved_b]))
        history_b = [{'id': 'local-history-b', 'adres': 'https://local-history.fixture.invalid/', 'nazvanie': 'Local history only',
                      'vremya': int(time.time()), 'papka': '', 'super_id': None}]
        (b.data / 'istoriya.json').write_text(json.dumps(history_b))
        ca, cb = a.start(), b.start()
        require(a.display != b.display and a.inspector != b.inspector, 'Devices do not have independent native environments')
        require(a.app.poll() is None and b.app.poll() is None, 'Two app processes are not simultaneously alive')
        checkpoint('two_real_apps_independent_dbus_display_inspector_data')
        phase = 'vault-create'
        for client, master in [(ca, master_a), (cb, master_b)]:
            require(call(client, 'security_vault', action='status') == {'exists': False, 'unlocked': False}, 'Fresh device vault is not empty')
            value = call(client, 'security_vault', action='create', password=master)
            require(value['exists'] and value['unlocked'], 'Native vault creation failed')
        checkpoint('real_local_vault_create_both_devices')
        phase = 'native-capture'
        normal_id = 'native-credentials'
        first_url = base + '/form?case=capture&mode=prefill'
        call(ca, 'otkryt', nastrojki=super_record(normal_id, 'Native credentials', first_url))
        normal_tab = a.loaded(normal_id, address=first_url)['aktivnaya']
        a.navigate(normal_id, first_url + '&set_cookie=1')
        wait_for(lambda: seen('capture'), 'CEF did not report fixture readiness')
        require(call(ca, 'security_passwords', action='list') == [], 'Page load saved a password without consent')
        draft = call(ca, 'security_passwords', action='capture', data={'super_id': normal_id, 'tab_id': normal_tab})
        require(draft.get('origin') == base and draft.get('username') == login and 'password' not in draft, 'Native capture did not return a safe draft summary')
        require(call(ca, 'security_passwords', action='list') == [], 'Capture saved a password before consent')
        credential = call(ca, 'security_passwords', action='save_draft', data={'draft_id': draft['draft_id'], 'username': login, 'title': 'Native captured credential'})
        require('password' not in credential, 'Saved credential summary exposed a password')
        credential_id = credential['id']
        require(len(call(ca, 'security_passwords', action='list')) == 1, 'Consented capture was not saved')
        check_encrypted(a)
        checkpoint('native_cef_capture_draft_explicit_consent_encrypted_store')
        phase = 'vault-lock-unlock'
        before = {p.name: hashlib.sha256(p.read_bytes()).digest() for p in a.cipher_files()}
        call(ca, 'security_vault', action='lock')
        call(ca, 'security_passwords', action='list', error=True)
        call(ca, 'security_vault', action='unlock', password=wrong_master, error=True)
        require(not call(ca, 'security_vault', action='status')['unlocked'], 'Wrong master password unlocked the vault')
        call(ca, 'security_vault', action='create', password=master_a, error=True)
        require(before == {p.name: hashlib.sha256(p.read_bytes()).digest() for p in a.cipher_files()}, 'Failed unlock/create overwrote vault main or backup')
        call(ca, 'security_vault', action='unlock', password=master_a)
        require(len(call(ca, 'security_passwords', action='list')) == 1, 'Credential did not survive lock/unlock')
        resumed_draft = call(ca, 'security_passwords', action='capture', data={'super_id': normal_id, 'tab_id': normal_tab})
        require(resumed_draft.get('origin') == base and resumed_draft.get('username') == login and 'password' not in resumed_draft, 'Repeated capture after lock/unlock lost the existing isolated context')
        call(ca, 'security_passwords', action='discard_draft', data={'draft_id': resumed_draft['draft_id']})
        checkpoint('existing_isolated_context_reused_after_lock_unlock_without_navigation')
        checkpoint('wrong_password_and_recreate_preserve_main_backup_locked_reads_fail')
        phase = 'native-fill'
        a.navigate(normal_id, base + '/form?case=empty-fill&mode=empty')
        wait_for(lambda: seen('empty-fill'), 'Empty form did not initialize')
        call(ca, 'security_passwords', action='fill', data={'id': credential_id, 'super_id': normal_id, 'tab_id': normal_tab})
        wait_for(lambda: any(p.get('username') == login and p.get('password') == site_password for p in seen('empty-fill', 'change')),
                 'Native fill did not produce the actual DOM values/change events')
        require(seen('empty-fill', 'input') and seen('empty-fill', 'change'), 'Native fill did not dispatch input and change events')
        checkpoint('real_same_origin_fill_and_actual_dom_input_change_events')
        phase = 'cross-origin-rejection'
        other_origin = base.replace('127.0.0.1', 'localhost')
        a.navigate(normal_id, other_origin + '/form?case=other-origin&mode=empty')
        wait_for(lambda: seen('other-origin'), 'Other-origin fixture did not initialize')
        call(ca, 'security_passwords', action='fill', data={'id': credential_id, 'super_id': normal_id, 'tab_id': normal_tab}, error=True)
        require(not any(p.get('password') for p in seen('other-origin', 'input') + seen('other-origin', 'change')),
                'Rejected cross-origin fill still disclosed a password')
        a.submit()
        wait_for(lambda: any(p.get('trusted') for p in seen('other-origin', 'submit')), 'Other-origin DOM could not be inspected through actual submit')
        require(all(not p.get('username') and not p.get('password') for p in seen('other-origin', 'submit')), 'Rejected cross-origin fill changed actual form values')
        checkpoint('other_origin_native_fill_rejected_without_disclosure')
        phase = 'automatic-offer'
        a.navigate(normal_id, base + '/form?case=offer&mode=offer')
        wait_for(lambda: seen('offer'), 'Offer fixture did not initialize')
        barrier = call(ca, 'security_passwords', action='capture', data={'super_id': normal_id, 'tab_id': normal_tab})
        call(ca, 'security_passwords', action='discard_draft', data={'draft_id': barrier['draft_id']})
        time.sleep(.25)
        a.submit()
        wait_for(lambda: any(p.get('trusted') for p in seen('offer', 'submit')), 'Fixture submit was not a real trusted native click')
        offer = js_wait(ca, 'document.querySelector(".password-offer")?.innerText', 'Trusted form submit did not produce the password offer', 20)
        require(base in offer and offer_login in offer and offer_password not in offer, 'Password offer did not show safe origin/login metadata')
        require(len(call(ca, 'security_passwords', action='list')) == 1, 'Trusted submit auto-saved before user consent')
        call(ca, 'fokus_obolochke')
        time.sleep(.25)
        ca.click('.password-offer button')
        wait_for(lambda: len(call(ca, 'security_passwords', action='list')) == 2, 'Offer consent button did not save the credential')
        js_wait(ca, '!document.querySelector(".password-offer")', 'Offer did not dismiss after consent')
        checkpoint('trusted_submit_native_offer_no_autosave_physical_consent')
        phase = 'private-rejection'
        call(ca, 'obychnaya_komanda', id=normal_id, komanda={'deystvie': 'novaya_privatnaya', 'adres': base + '/form?case=private-only&mode=private'})
        private_tab = a.loaded(normal_id, address=base + '/form?case=private-only&mode=private', private=True)['aktivnaya']
        require(private_tab != normal_tab, 'Private tab was not created')
        wait_for(lambda: seen('private-only'), 'Private fixture did not initialize')
        call(ca, 'security_passwords', action='capture', data={'super_id': normal_id, 'tab_id': private_tab}, error=True)
        call(ca, 'security_passwords', action='fill', data={'id': credential_id, 'super_id': normal_id, 'tab_id': private_tab}, error=True)
        point=js_wait(ca, '(()=>{const r=document.querySelector(".pole")?.getBoundingClientRect();return r?[r.x+100,r.y+260]:null})()', 'Private link viewport missing')
        ca.mouse(*point,button=2)
        def private_middle():
            session=call(ca,'obychnye_spisok',id=normal_id)
            target=next((tab for tab in session['vkladki'] if 'private-link-opened' in tab['adres']),None)
            if not target:return None
            require(target.get('privatnaya') and session['aktivnaya']==private_tab,'Middle link lost private mode or opener tab')
            return target
        linked=wait_for(private_middle,'Private middle click did not open a background private tab')
        call(ca,'obychnaya_komanda',id=normal_id,komanda={'deystvie':'zakryt','id':linked['id']})
        checkpoint('private_middle_link_opens_background_same_super_without_losing_privacy')
        a.submit()
        wait_for(lambda: any(p.get('trusted') for p in seen('private-only', 'submit')), 'Private submit fixture did not execute')
        require(all(p.get('username') == forms['private'][0] and p.get('password') == forms['private'][1] for p in seen('private-only', 'submit')), 'Rejected private fill changed actual form values')
        time.sleep(.3)
        require(not ca.js('!!document.querySelector(".password-offer")'), 'Private submit exposed a password offer')
        require(len(call(ca, 'security_passwords', action='list')) == 2, 'Private page saved a credential')
        require(not any('private-only' in row['adres'] or 'private-link-opened' in row['adres'] for row in call(ca, 'biblioteka_spisok', razdel='istoriya')), 'Private URL entered history')
        saved_session = a.data / 'profili' / normal_id / 'obychnye.json'
        require(saved_session.is_file() and 'private-only' not in saved_session.read_text(), 'Private URL entered persisted session')
        call(ca, 'obychnaya_komanda', id=normal_id, komanda={'deystvie': 'vybrat', 'id': normal_tab})
        a.loaded(normal_id, tab_id=normal_tab)
        checkpoint('private_capture_fill_auto_offer_and_history_excluded')
        phase = 'isolated-account'
        call(ca, 'security_account', action='configure', data={'server': base}, error=True)
        require(not call(ca, 'security_account', action='status')['configured'], 'Wrong health marker was accepted as an account service')
        username = 'native-browser-' + suffix
        for client, register in [(ca, True), (cb, False)]:
            require(not call(client, 'security_account', action='status')['configured'], 'Fresh client silently selected an endpoint before consent')
            client.js('window.dispatchEvent(new Event("nexusbro-security"));true')
            js_wait(client, '!!document.querySelector(".security-panel")', 'Security settings did not open')
            client.text_button('Аккаунт')
            js_wait(client, '!!document.querySelector(".security-account-form")', 'Account interface did not render')
            if register:
                painted(client)
                from PIL import ImageGrab
                ImageGrab.grab(xdisplay=a.display).save(root/'account-without-server-entry.png')
            # The user never opens advanced settings or enters a server URL.
            require(not client.js('(()=>{const e=document.querySelector(`[aria-label="Сервер синхронизации NexusBro"]`);return e&&e.getClientRects().length>0})()'), 'Server URL is still required in the ordinary account flow')
            if register:client.text_button('Зарегистрироваться')
            client.fill('[aria-label="Имя аккаунта NexusBro"]', username)
            client.fill('[aria-label="Пароль аккаунта NexusBro"]', account_password)
            if register:client.fill('[aria-label="Повторите пароль аккаунта"]', account_password)
            client.text_button('Создать аккаунт NexusBro' if register else 'Войти в NexusBro')
            wait_for(lambda: call(client, 'security_account', action='status')['logged_in'], 'Account UI did not complete registration/login without server entry', 45)
            if register:
                key=js_wait(client, 'document.querySelector(`[aria-label="Ключ восстановления аккаунта"] .security-emergency`)?.innerText', 'Registration did not present recovery material')
                sensitive.add(key)
            client.click('[aria-label="Закрыть пароли и синхронизацию"]')
        checkpoint('native_ui_register_login_without_server_field_real_service')
        phase = 'first-selected-sync'
        call(ca, 'zakladki_sohranit', adres='https://selected-bookmark.fixture.invalid/', nazvanie='Selected encrypted bookmark', papka='')
        selection_a = {'settings': True, 'bookmarks': True, 'supers': [normal_id, 'retained-super'], 'passwords': [credential_id]}
        call(ca, 'security_sync', action='configure', data={'selection': selection_a})
        private_selection = {**selection_a, 'supers': ['pvt-native-private']}
        call(ca, 'security_sync', action='configure', data={'selection': private_selection}, error=True)
        first_report = call(ca, 'security_sync', action='run', timeout=90)
        report_ok(first_report)
        require(first_report['pushed'] >= 5, 'Selected native records were not actually uploaded')
        checkpoint('selected_settings_bookmarks_supers_password_really_encrypted_uploaded')
        phase = 'new-device-default-off'
        status = call(cb, 'security_sync', action='status')
        require(status['selection'] == {'settings': False, 'bookmarks': False, 'supers': [], 'passwords': []}, 'New account device silently enabled sync categories')
        b_before_settings = call(cb, 'nastrojki_poluchit')
        b_before_supers = call(cb, 'spisok')
        b_before_history = call(cb, 'biblioteka_spisok', razdel='istoriya')
        off_report = call(cb, 'security_sync', action='run', timeout=90)
        report_ok(off_report)
        require(off_report['pushed'] == 0 and off_report['pulled'] == 0, 'All-off sync applied or uploaded records')
        require(call(cb, 'nastrojki_poluchit') == b_before_settings and call(cb, 'spisok') == b_before_supers, 'All-off changed local native data')
        require(call(cb, 'security_passwords', action='list') == [] and call(cb, 'biblioteka_spisok', razdel='zakladki') == [], 'All-off imported passwords/bookmarks')
        checkpoint('new_native_device_all_off_no_upload_or_apply')
        phase = 'wildcard-opt-in-import'
        call(cb, 'security_sync', action='configure', data={'selection': {'settings': True, 'bookmarks': True, 'supers': ['*'], 'passwords': ['*']}})
        imported = call(cb, 'security_sync', action='run', timeout=90)
        require(not imported.get('errors'), 'Native import has application errors')
        require(imported['pulled'] >= 3, 'Wildcard did not import new native records')
        # Fresh-device settings/saved metadata intentionally differ. They must be
        # preserved as conflicts until an explicit owner choice applies remote.
        expected_conflicts = {'nexusbro.v1/settings/global', 'nexusbro.v1/super/retained-super'}
        require(set(imported['conflicts']) <= expected_conflicts, 'Unexpected native import conflict')
        require('nexusbro.v1/settings/global' in imported['conflicts'], 'Differing local settings were overwritten instead of conflicting')
        require(call(cb, 'nastrojki_poluchit')['tema'] == 'svetlaya', 'Pending conflict overwrote local settings')
        for identifier in imported['conflicts']:
            call(cb, 'security_sync', action='resolve', data={'id': identifier, 'choice': 'remote'}, timeout=60)
        settled = call(cb, 'security_sync', action='run', timeout=90)
        report_ok(settled)
        b_settings = call(cb, 'nastrojki_poluchit')
        require(b_settings['tema'] == 'grafit', 'Explicit remote settings choice did not apply')
        require(b_settings['vyhody'] == b_before_settings['vyhody'], 'Sync overwrote device-local connection settings')
        saved = call(cb, 'sohranennye')
        retained = next(row for row in saved if row['id'] == 'retained-super')
        require(retained['nazvanie'] == saved_a['nazvanie'] and retained['proxy'] == saved_b['proxy'], 'Remote metadata replaced a local proxy or failed to apply')
        rows_b = call(cb, 'security_passwords', action='list')
        require(len(rows_b) == 1 and rows_b[0]['id'] == credential_id, 'Unselected credential was synced or selected credential missing')
        require(any(row['adres'] == 'https://selected-bookmark.fixture.invalid/' for row in call(cb, 'biblioteka_spisok', razdel='zakladki')), 'Selected bookmark was not applied')
        require(call(cb, 'biblioteka_spisok', razdel='istoriya') == b_before_history, 'History copied between native devices')
        require(not any(row['id'].startswith('pvt-') for row in call(cb, 'spisok') + saved), 'Private definition copied between devices')
        checkpoint('wildcard_new_record_import_explicit_conflicts_proxy_and_history_exclusion')
        phase = 'imported-password-and-cookie-isolation'
        call(cb, 'pereklyuchit', id=normal_id)
        b.loaded(normal_id, address=first_url)
        session_b = b.navigate(normal_id, base + '/form?case=b-cookie-check&mode=empty')
        wait_for(lambda: seen('b-cookie-check'), 'Second CEF instance did not load fixture')
        require(all('native-source=only-A' not in p.get('cookie_header', '') for p in seen('b-cookie-check')), 'Source cookies copied to second app')
        call(cb, 'security_passwords', action='fill', data={'id': credential_id, 'super_id': normal_id, 'tab_id': session_b['aktivnaya']})
        wait_for(lambda: any(p.get('username') == login and p.get('password') == site_password for p in seen('b-cookie-check', 'change')),
                 'Imported password did not decrypt and fill actual second-device DOM')
        checkpoint('synced_password_decrypts_in_second_real_cef_cookie_profiles_isolated')
        phase = 'concurrent-native-edit-conflict'
        set_theme(a, 'okean')
        set_theme(b, 'svetlaya')
        report_ok(call(ca, 'security_sync', action='run', timeout=90))
        conflict = call(cb, 'security_sync', action='run', timeout=90)
        require('nexusbro.v1/settings/global' in conflict['conflicts'], 'Concurrent native settings edits did not preserve a conflict')
        previous_success = call(cb, 'security_sync', action='status')['last_sync']
        set_theme(b, 'sliva')
        call(cb, 'security_sync', action='resolve', data={'id': 'nexusbro.v1/settings/global', 'choice': 'remote'}, error=True, timeout=60)
        require(call(cb, 'nastrojki_poluchit')['tema'] == 'sliva', 'Stale conflict resolution overwrote a newer local edit')
        require(call(cb, 'security_sync', action='status')['last_sync'] == previous_success, 'Failed stale resolution advanced success state')
        updated = call(cb, 'security_sync', action='run', timeout=90)
        require('nexusbro.v1/settings/global' in updated['conflicts'], 'New local conflict snapshot was not retained')
        call(cb, 'security_sync', action='resolve', data={'id': 'nexusbro.v1/settings/global', 'choice': 'local'}, timeout=60)
        report_ok(call(cb, 'security_sync', action='run', timeout=90))
        report_ok(call(ca, 'security_sync', action='run', timeout=90))
        require(call(ca, 'nastrojki_poluchit')['tema'] == 'sliva', 'Explicit local choice did not propagate without data loss')
        checkpoint('native_conflict_stale_resolution_rejected_local_choice_propagates')
        phase = 'disabled-selection-retains-local-and-cloud'
        off = {'settings': False, 'bookmarks': False, 'supers': [], 'passwords': []}
        call(cb, 'security_sync', action='configure', data={'selection': off})
        old_books = call(cb, 'biblioteka_spisok', razdel='zakladki')
        call(ca, 'zakladki_sohranit', adres='https://not-enabled-on-b.fixture.invalid/', nazvanie='Selected on A only', papka='')
        report_ok(call(ca, 'security_sync', action='run', timeout=90))
        off_report = call(cb, 'security_sync', action='run', timeout=90)
        report_ok(off_report)
        require(off_report['pushed'] == 0 and off_report['pulled'] == 0, 'Disabled selection still transmitted/applied records')
        require(call(cb, 'biblioteka_spisok', razdel='zakladki') == old_books, 'Disabled bookmark category applied a new cloud item')
        require(len(call(cb, 'security_passwords', action='list')) == 1, 'Disabling selection deleted a local credential')
        require(len(call(ca, 'biblioteka_spisok', razdel='zakladki')) == 2, 'Disabling B deleted another device data')
        checkpoint('disabling_categories_never_deletes_other_device_or_applies_remote')
        phase = 'authenticated-selected-delete'
        call(ca, 'security_passwords', action='delete', data={'id': credential_id})
        report_ok(call(ca, 'security_sync', action='run', timeout=90))
        report_ok(call(cb, 'security_sync', action='run', timeout=90))
        require(len(call(cb, 'security_passwords', action='list')) == 1, 'Unselected remote deletion removed local credential')
        call(cb, 'security_sync', action='configure', data={'selection': {**off, 'passwords': ['*']}})
        report_ok(call(cb, 'security_sync', action='run', timeout=90))
        require(call(cb, 'security_passwords', action='list') == [], 'Authenticated selected password tombstone did not apply')
        checkpoint('authenticated_tombstone_applies_only_to_selected_category')
        phase = 'whole-browser-lock-all-windows'
        call(ca,'novoe_okno',id=normal_id,privatnaya=False)
        extra=None
        deadline=time.monotonic()+20
        while time.monotonic()<deadline:
            try:extra=Client(display=a.display,port=a.inspector,page_index=1);break
            except (OSError,ConnectionError,websocket.WebSocketException):time.sleep(.2)
        require(extra is not None,'Additional browser window did not create inspector')
        try:
            js_wait(extra,'!!document.querySelector(".okno")','Additional browser window did not initialize')
            main_xid=subprocess.check_output(['xdotool','search','--onlyvisible','--name','^NexusBro$'],env={**os.environ,'DISPLAY':a.display},text=True).splitlines()[0]
            subprocess.run(['xdotool','windowraise',main_xid,'windowfocus',main_xid],env={**os.environ,'DISPLAY':a.display},check=True)
            a.submit()
            subprocess.run(['xdotool','key','ctrl+shift+l'],env={**os.environ,'DISPLAY':a.display},check=True)
            js_wait(ca,'!!document.querySelector(".browser-lock-screen")','CEF CtrlShiftL did not lock main browser')
            js_wait(extra,'!!document.querySelector(".browser-lock-screen")','CEF CtrlShiftL did not lock additional window')
            call(ca,'spisok',error=True)
            call(extra,'biblioteka_spisok',razdel='istoriya',error=True)
            require(not call(cb,'browser_lock_status')['locked'],'Lock leaked to an independent browser instance')
            painted(ca)
            painted(extra)
            from PIL import ImageGrab
            ImageGrab.grab(xdisplay=a.display).save(root/'browser-locked-all-windows.png')
            subprocess.run(['xdotool','windowraise',main_xid,'windowfocus',main_xid],env={**os.environ,'DISPLAY':a.display},check=True)
            ca.fill('[aria-label="Мастер-пароль для разблокировки браузера"]',master_a)
            ca.text_button('Разблокировать браузер')
            js_wait(extra,'!document.querySelector(".browser-lock-screen")&&!!document.querySelector(".okno")','Unlock did not restore additional browser window')
            # Keep the additional window alive for the shell-lock pass too;
            # the owned app process cleanup below closes it at restart.
        finally:extra.close()
        checkpoint('native_cef_ctrl_shift_l_locks_all_windows_blocks_data_ipc_and_unlock_restores')
        # Lock from the shell keyboard path too; the existing encrypted master is used.
        subprocess.run(['xdotool','windowraise',main_xid,'windowfocus',main_xid],env={**os.environ,'DISPLAY':a.display},check=True)
        call(ca,'fokus_obolochke')
        js_wait(ca,'!document.querySelector(".browser-lock-screen")&&!!document.querySelector(".okno")','Unlock did not restore main browser window')
        js_wait(ca,'(()=>{const e=document.querySelector(`[aria-label="Адрес страницы"]`);return e&&!e.disabled&&e.getBoundingClientRect().width>100})()','Main address input did not become available after unlock')
        painted(ca)
        focused = False
        for focus_attempt in range(3):
            subprocess.run(['xdotool','windowraise',main_xid,'windowfocus',main_xid],env={**os.environ,'DISPLAY':a.display},check=True)
            call(ca,'fokus_obolochke')
            ca.click('[aria-label="Адрес страницы"]')
            until=time.monotonic()+1
            while time.monotonic()<until:
                focused=ca.js('document.hasFocus()&&document.activeElement?.getAttribute("aria-label")==="Адрес страницы"')
                if focused:break
                time.sleep(.05)
            if focused:break
        checks.append({'shell_physical_focus_attempts':focus_attempt+1})
        require(focused,'Physical click did not focus main address input')
        js_wait(ca,'!!document.querySelector(".address-suggestions")','Themed address suggestions did not appear')
        painted(ca)
        from PIL import ImageGrab
        ImageGrab.grab(xdisplay=a.display).save(root/'address-themed-suggestions.png')
        # Bare Xvfb has no window manager to activate the owning shell after
        # foreign CEF child mapping. Select the real native input target, not
        # merely document.activeElement (which can remain set in WebKit).
        subprocess.run(['xdotool','windowfocus',main_xid],env={**os.environ,'DISPLAY':a.display},check=True)
        native_target=subprocess.check_output(['xdotool','getwindowfocus'],env={**os.environ,'DISPLAY':a.display},text=True).strip()
        require(native_target==main_xid,'Physical keyboard target is not the owning browser shell')
        ca.js('window.__nativeKeyEvents=[];window.__nativeKeyProbe=e=>window.__nativeKeyEvents.push({type:e.type,key:e.key,code:e.code,ctrl:e.ctrlKey,shift:e.shiftKey,alt:e.altKey,meta:e.metaKey,trusted:e.isTrusted});window.addEventListener("keydown",window.__nativeKeyProbe,true);window.addEventListener("keyup",window.__nativeKeyProbe,true);true')
        subprocess.run(['xdotool','key','ctrl+shift+l'],env={**os.environ,'DISPLAY':a.display},check=True)
        time.sleep(.1)
        keys=ca.js('window.removeEventListener("keydown",window.__nativeKeyProbe,true);window.removeEventListener("keyup",window.__nativeKeyProbe,true);window.__nativeKeyEvents')
        checks.append({'physical_shell_shortcut_events':keys})
        js_wait(ca,'!!document.querySelector(".browser-lock-screen")','Shell CtrlShiftL did not lock browser')
        ca.fill('[aria-label="Мастер-пароль для разблокировки браузера"]',master_a)
        ca.text_button('Разблокировать браузер')
        js_wait(ca,'!document.querySelector(".browser-lock-screen")&&!!document.querySelector(".okno")','Shell lock did not unlock')
        checkpoint('shell_ctrl_shift_l_uses_same_master_gate')
        phase = 'restart-persistence'
        a.stop_app()
        ca = a.start()
        require(call(ca, 'browser_lock_status')['locked'], 'Existing vault did not require master password on restart')
        js_wait(ca, '!!document.querySelector(".browser-lock-screen")', 'Startup did not show browser lock screen')
        call(ca, 'spisok', error=True)
        call(ca, 'security_passwords', action='list', error=True)
        before_unlock = {p.name: hashlib.sha256(p.read_bytes()).digest() for p in a.cipher_files()}
        ca.fill('[aria-label="Мастер-пароль для разблокировки браузера"]', wrong_master)
        ca.text_button('Разблокировать браузер')
        js_wait(ca, '!!document.querySelector(".browser-lock-error")', 'Wrong startup master did not report an error')
        require(call(ca, 'browser_lock_status')['locked'], 'Wrong master unlocked browser')
        require(before_unlock == {p.name: hashlib.sha256(p.read_bytes()).digest() for p in a.cipher_files()}, 'Wrong browser unlock changed vault files')
        ca.fill('[aria-label="Мастер-пароль для разблокировки браузера"]', master_a)
        ca.text_button('Разблокировать браузер')
        js_wait(ca, '!document.querySelector(".browser-lock-screen")&&!!document.querySelector(".okno")', 'Correct master did not open browser')
        require(call(ca, 'security_vault', action='status')['unlocked'], 'Browser unlock did not unlock password vault')
        checkpoint('startup_browser_master_gate_wrong_password_preserves_ciphertext_correct_unlock_opens_sites')
        require(call(ca, 'security_account', action='status')['logged_in'], 'Encrypted account session did not survive restart')
        remaining = call(ca, 'security_passwords', action='list')
        require(len(remaining) == 1 and remaining[0]['username'] == offer_login, 'Local-only credential did not persist independently')
        require(call(ca, 'security_sync', action='status')['selection'] == selection_a, 'Granular sync selection did not persist')
        checkpoint('restart_locks_key_persists_encrypted_session_selection_local_only_password')
        phase = 'ciphertext-and-log-review'
        for device in devices:
            check_encrypted(device)
        for path in root.rglob('*.log'):
            raw = path.read_bytes()
            require(all(secret.encode() not in raw for secret in sensitive), 'Plaintext credential found in runtime log')
        for path in sync_root.glob('nexusbro-sync.db*'):
            raw = path.read_bytes()
            require(all(secret.encode() not in raw for secret in sensitive), 'Plaintext credential found in server DB/WAL')
            require(b'Native captured credential' not in raw and b'Selected encrypted bookmark' not in raw,
                    'Server stored unencrypted browser record metadata')
        checkpoint('vault_main_backup_runtime_logs_server_database_have_no_plaintext_credentials')
        checkpoint('success')
    except Exception as error:
        if phase == 'whole-browser-lock-all-windows':
            try:
                focus = ca.js('(()=>{const e=document.activeElement,r=document.querySelector(`[aria-label="Адрес страницы"]`)?.getBoundingClientRect();return {has_focus:document.hasFocus(),label:window.__TAURI_INTERNALS__.metadata.currentWindow.label,active_tag:e?.tagName,active_label:e?.getAttribute("aria-label"),address_rect:r?{x:r.x,y:r.y,width:r.width,height:r.height}:null,locked:!!document.querySelector(".browser-lock-screen")}})()')
                checks.append({'shell_focus_diagnostic':focus})
                focus_id=subprocess.check_output(['xdotool','getwindowfocus'],env={**os.environ,'DISPLAY':a.display},text=True).strip()
                checks.append({'native_focus_window':focus_id,'expected_main_window':main_xid})
                for xid in [focus_id,main_xid]:
                    details={}
                    for command in [['xdotool','getwindowname',xid],['xdotool','getwindowpid',xid],['xdotool','getwindowgeometry','--shell',xid]]:
                        r=subprocess.run(command,env={**os.environ,'DISPLAY':a.display},capture_output=True,text=True)
                        details[' '.join(command[:2])]=r.stdout[:5000]
                    checks.append({'native_window_diagnostic':{'xid':xid,'details':details}})
                from PIL import ImageGrab
                ImageGrab.grab(xdisplay=a.display).save(root/'shell-focus-failure.png')
            except Exception:
                checks.append({'shell_focus_diagnostic_unavailable':True})
        message = str(error)
        for secret in sensitive:
            message = message.replace(secret, '[synthetic credential redacted]')
        failure = 'security_native failed during ' + phase + ': ' + message[:1000]
        checks.append({'failed_' + phase.replace('-', '_'): True, 'success': False})
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous_alarm)
        for device in reversed(devices):
            try:
                device.close()
            except Exception:
                checks.append({'native_process_cleanup_failed': True, 'success': False})
                failure = failure or 'Native process cleanup failed'
        for process in processes:
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
        for stream in streams:
            stream.close()
        if fixture:
            fixture.shutdown()
            fixture.server_close()
        for path in root.rglob('*.log'):
            if any(secret.encode() in path.read_bytes() for secret in sensitive):
                checks.append({'runtime_log_plaintext_after_shutdown': True, 'success': False})
                failure = failure or 'Plaintext credential appeared in runtime shutdown log'
        (root / 'results.json').write_text(json.dumps(checks, indent=2) + '\n')
        print(json.dumps(checks))
    if failure:
        raise NativeFailure(failure)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', required=True)
    parser.add_argument('--cef', required=True)
    parser.add_argument('--server', required=True)
    parser.add_argument('--out', required=True)
    run(parser.parse_args())
