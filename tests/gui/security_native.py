"""GitHub-only native security acceptance: two Tauri apps + real CEF + sync server.

No Tauri/HTTP/crypto mocks. Each app has its own D-Bus session, X display,
inspector endpoint and data/config/cache roots. All credentials are generated
synthetic fixtures; only boolean checkpoints enter results.json. Website probes
stay in memory. No deployment or external endpoint is used.
"""
import argparse
import base64
import hashlib
import hmac
import http.server
import json
import os
import pathlib
import re
import secrets
import select
import shutil
import signal
import socket
import sqlite3
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


def cloud_stroke_ink(pixels, foreground):
    """Count actual foreground strokes allowing only correct alpha composition.

    Input is the actual SVG rectangle, excluding the badge border. Its empty
    corner regions establish the native painted background, not a DOM fallback.
    Subpixel SVG strokes need not contain any fully covered foreground pixel.
    """
    from collections import Counter
    pixels = pixels.convert('RGB')
    w, h = pixels.size
    if w < 8 or h < 8:
        return {'ink': 0, 'spread': False, 'background': None}
    corners = [pixels.getpixel((x, y)) for x in (0, 1, w - 2, w - 1)
               for y in (0, 1, h - 2, h - 1)]
    background, frequency = Counter(corners).most_common(1)[0]
    vector = [foreground[i] - background[i] for i in range(3)]
    norm = sum(channel * channel for channel in vector)
    points, alphas = [], []
    if frequency >= 4 and norm >= 144:
        for y in range(h):
            for x in range(w):
                pixel = pixels.getpixel((x, y))
                alpha = sum((pixel[i] - background[i]) * vector[i] for i in range(3)) / norm
                residual = max(abs(background[i] + alpha * vector[i] - pixel[i]) for i in range(3))
                if .45 <= alpha <= 1.01 and residual <= 3:
                    points.append((x, y))
                    alphas.append(alpha)
    spread = bool(points and max(x for x, _ in points) - min(x for x, _ in points) >= 2
                  and max(y for _, y in points) - min(y for _, y in points) >= 2)
    return {'ink': len(points), 'spread': spread, 'background': background,
            'corner_frequency': frequency, 'max_alpha': max(alphas, default=0)}


def check_cloud_stroke_model():
    """Small negative/positive fixtures for the native screenshot classifier."""
    from PIL import Image, ImageDraw
    background, green, red, gray = (32, 34, 37), (86, 189, 131), (238, 114, 121), (173, 187, 202)
    blank = Image.new('RGB', (12, 12), background)
    require(cloud_stroke_ink(blank, green)['ink'] == 0, 'Blank SVG crop was accepted as cloud ink')
    badge = Image.new('RGB', (16, 16), background)
    ImageDraw.Draw(badge).rectangle((0, 0, 15, 15), outline=gray)
    require(cloud_stroke_ink(badge.crop((2, 2, 14, 14)), gray)['ink'] == 0,
            'Badge border was accepted as a gray SVG glyph')
    for foreground in (green, red, gray):
        tile = blank.copy()
        blend = tuple(round(b + .65 * (f - b)) for b, f in zip(background, foreground))
        for point in ((3, 4), (4, 4), (6, 6), (8, 7)):
            tile.putpixel(point, blend)
        found = cloud_stroke_ink(tile, foreground)
        require(found['ink'] >= 3 and found['spread'], 'Actual alpha-composed cloud stroke was rejected')
        for wrong in (green, red, gray):
            if wrong != foreground:
                require(cloud_stroke_ink(tile, wrong)['ink'] == 0,
                        'Wrong cloud stroke hue was accepted')


def totp(secret, timestamp=None):
    """RFC 6238 SHA-1 / six digits, used only for a synthetic test account."""
    key = base64.b32decode(secret.upper() + '=' * (-len(secret) % 8))
    counter = int(time.time() if timestamp is None else timestamp) // 30
    digest = hmac.new(key, counter.to_bytes(8, 'big'), hashlib.sha1).digest()
    offset = digest[-1] & 15
    return str((int.from_bytes(digest[offset:offset + 4], 'big') & 0x7fffffff) % 1_000_000).zfill(6)


def scroll_visible(client, selector):
    require(client.js('(()=>{const e=document.querySelector(' + json.dumps(selector) + ');'
                      'if(!e)return false;e.scrollIntoView({block:"center",inline:"nearest"});return true})()'),
            'Native UI control is missing: ' + selector)
    painted(client)


def visible_qr_uri(client, device):
    """Decode actual native screen pixels, entirely in memory; never save the QR."""
    from PIL import ImageGrab
    decoder = pathlib.Path(__file__).resolve().parents[2] / 'desktop/node_modules/jsqr/dist/jsQR.js'
    require(decoder.is_file(), 'Install locked desktop devDependencies before native QR acceptance')
    selector = 'canvas[data-testid="totp-qr"]'
    scroll_visible(client, selector)
    rect = client.js('(()=>{const e=document.querySelector(' + json.dumps(selector) + ');'
                     'const r=e.getBoundingClientRect(),s=getComputedStyle(e);'
                     'return {x:r.x,y:r.y,w:r.width,h:r.height,k:devicePixelRatio||1,'
                     'visible:s.visibility==="visible"&&s.display!=="none"&&r.top>=0&&r.bottom<=innerHeight,'
                     'role:e.getAttribute("role"),label:e.getAttribute("aria-label")}})()')
    require(rect and rect['visible'] and rect['w'] >= 160 and rect['h'] >= 160,
            'Enrollment QR is not fully visible at scannable size')
    require(rect['role'] == 'img' and rect['label'] == 'QR-код для приложения-аутентификатора',
            'Enrollment QR lacks an accessible image label')
    label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    origin = client.command('plugin:window|inner_position', label=label)
    box = (round(origin['x'] + rect['x'] * rect['k']), round(origin['y'] + rect['y'] * rect['k']),
           round(origin['x'] + (rect['x'] + rect['w']) * rect['k']),
           round(origin['y'] + (rect['y'] + rect['h']) * rect['k']))
    pixels = ImageGrab.grab(xdisplay=device.display).crop(box).convert('RGBA')
    # Keep both the decoder bundle and RGBA payload off the WebKit inspector:
    # large Runtime.evaluate frames can reset its connection. Node receives
    # synthetic screen pixels only over stdin and returns the URI over a private
    # captured pipe; no QR, provisioning URI, or key is written to artifacts.
    program = ('const fs=require("fs"),decode=require(process.argv[1]);'
               'const image=JSON.parse(fs.readFileSync(0,"utf8"));'
               'const bytes=new Uint8ClampedArray(Buffer.from(image.pixels,"base64"));'
               'const qr=decode(bytes,image.width,image.height,{inversionAttempts:"dontInvert"});'
               'process.stdout.write(JSON.stringify({data:qr?.data||null}));')
    try:
        decoded = subprocess.run(['node', '-e', program, str(decoder)],
                                 input=json.dumps({'width': pixels.width, 'height': pixels.height,
                                                   'pixels': base64.b64encode(pixels.tobytes()).decode('ascii')}),
                                 capture_output=True, text=True, timeout=15)
        require(decoded.returncode == 0 and len(decoded.stdout) <= 8192,
                'Local QR decoder process did not complete')
        value = json.loads(decoded.stdout).get('data')
    except Exception:
        raise NativeFailure('Local decoder could not inspect visible enrollment QR') from None
    require(isinstance(value, str) and value.startswith('otpauth://totp/'),
            'Visible native enrollment QR does not decode as a TOTP provisioning URI')
    return value


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
                                           '1280x900x24', '-nolisten', 'tcp', '-noreset'], pass_fds=(write_fd,),
                                          stdout=self.log('xvfb.log'), stderr=subprocess.STDOUT,
                                          start_new_session=True)
                os.close(write_fd)
                write_fd = None
                ready, _, _ = select.select([read_fd], [], [], 25)
                require(ready, 'Xvfb did not allocate a display within 25 seconds (exit=' +
                        str(self.x.poll()) + ')')
                number = os.read(read_fd, 32).decode().strip()
                require(number.isdigit(), 'Xvfb display allocation failed (exit=' + str(self.x.poll()) + ')')
                self.display = ':' + number
                # Keep this owned X server across GTK probes/restarts. A reset
                # must not try writing a second display number into the closed
                # allocation pipe (observed fatal Xvfb error in native run).
                deadline=time.monotonic()+5
                ready=False
                while time.monotonic()<deadline:
                    require(self.x.poll() is None,'Owned Xvfb exited before GTK launch')
                    probe=subprocess.run(['xdotool','getdisplaygeometry'],env={**os.environ,'DISPLAY':self.display},capture_output=True,text=True,timeout=2)
                    if probe.returncode==0 and probe.stdout.split()==['1280','900']:
                        ready=True;break
                    time.sleep(.05)
                require(ready,'Owned Xvfb did not accept a real X11 client before GTK launch')
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

    def cloud_refresh(client):
        # Same notification as security.ts mutations; the App reads real IPC
        # state. No injected selection, receipt, or synthetic cloud status.
        client.js('window.dispatchEvent(new Event("nexusbro-security-state-changed"));true')

    def cloud_state(client, *, selected, receipt=False, saved_state=None):
        status = call(client, 'security_sync', action='status')
        native_state = {'local': 'off', 'pending': 'pending', 'selected': 'ok',
                        'error': 'error', 'conflict': 'conflict'}[selected]
        require(('native-credentials' in status['selection']['supers']) == (selected != 'local'),
                'Cloud assertion does not match persisted native selection')
        require(status.get('super_states', {}).get('native-credentials', {}).get('state') == native_state,
                'Per-record native ACK state does not justify the displayed cloud state')
        require(status.get('sync_state', {}).get('state') == ('error' if native_state == 'conflict' else native_state),
                'Aggregate native status does not justify the cloud header state')
        require('pvt-cloud-ui' not in status['super_states'] and 'pvt-native-private' not in status['super_states'],
                'Private definitions entered the authoritative cloud receipt map')
        js_wait(client, 'document.querySelector(`[data-super-id="native-credentials"]`)?.dataset.cloudState===' +
                json.dumps(selected), 'Super cloud state did not refresh from native status')
        require(client.js('(()=>{const e=document.querySelector(`[data-super-id="native-credentials"]`);'
                          'const group=e.closest("[data-cloud-group]")?.dataset.cloudGroup;'
                          'return group===' + json.dumps('local' if selected == 'local' else 'cloud') +
                          ('||(!group&&!document.querySelector(`[data-cloud-group="cloud"]`))' if selected == 'local' else '') + '})()'),
                'Selected Super is in the wrong cloud/local group')
        require(client.js('(()=>{const ordinary=document.querySelector(`[data-super-id="local-cloud-ui"]`),'
                          'privateRow=document.querySelector(`[data-super-id="pvt-cloud-ui"]`);'
                          'return [ordinary,privateRow].every(e=>e&&e.dataset.cloudState==="local"&&'
                          'e.closest("[data-cloud-group]")?.dataset.cloudGroup==="local")&&'
                          '!ordinary.closest("[data-super-row]").querySelector(".super-cloud-badge")&&'
                          '!privateRow.closest("[data-super-row]").querySelector(".super-cloud-badge")})()'),
                'Unselected or private Super was presented as cloud-selected')
        if selected != 'local':
            saved_state = saved_state or selected
            js_wait(client, 'document.querySelector(`[data-super-id="retained-super"][data-cloud-saved="true"]`)?.dataset.cloudState===' +
                    json.dumps(saved_state), 'Selected saved Super did not reflect its independent receipt state')
            require(client.js('(()=>{const rows=Array.from(document.querySelectorAll("[data-super-id]"));'
                              'return new Set(rows.map(e=>e.dataset.superId)).size===rows.length&&'
                              '!document.querySelector(`[data-super-id="pvt-native-private"]`)})()'),
                    'Cloud group duplicated open/saved IDs or included a private saved definition')
        if receipt:
            require(status.get('last_sync'), 'UI cloud receipt has no native successful account sync')
            js_wait(client, '!!document.querySelector(`[data-testid="cloud-last-account-sync"]`)',
                    'Sidebar did not render the real account sync receipt')
            require(client.js('document.querySelector(`[data-testid="cloud-last-account-sync"]`).dataset.lastSync') ==
                    str(status['last_sync']), 'Displayed account receipt differs from persisted native last_sync')
            require(client.js('(()=>{const e=document.querySelector(`[data-testid="cloud-last-account-sync"]`);'
                              'return e.getBoundingClientRect().height<=32&&e.textContent.trim()==="Облако"&&'
                              'e.title.includes("синхронизац")})()'),
                    'Cloud header is not one compact row with its date confined to tooltip metadata')

    def cloud_click_sync(client, *, device=None, selector='[data-testid="cloud-last-account-sync"] button.super-cloud-heading-icon', expected='selected'):
        selection = call(client, 'security_sync', action='status')['selection']
        active = call(client, 'aktivnaya')
        open_ids = [row['id'] for row in call(client, 'spisok')]
        require(client.js('!document.querySelector(".security-panel,.security-overlay")'),
                'Cloud click should be accepted without opening security settings')
        client.click(selector)
        js_wait(client, '(()=>{const e=document.querySelector(`[data-testid="cloud-last-account-sync"]`);'
                'return e?.dataset.cloudState===' + json.dumps(expected) +
                '&&!e.querySelector("button").disabled})()', 'Cloud click did not complete with honest native status', 90)
        status = call(client, 'security_sync', action='status')
        require(status['selection'] == selection, 'Cloud click silently changed sync selection')
        require(call(client, 'aktivnaya') == active and [row['id'] for row in call(client, 'spisok')] == open_ids,
                'Cloud click switched a Super or started a dormant engine')
        require(client.js('!document.querySelector(".security-panel,.security-overlay")'),
                'Cloud click opened security settings instead of running synchronization')
        if device is not None:
            visible_cef(client, device, 'cloud-click-keeps-cef-' + expected)
        return status.get('report', {})

    def cloud_painted(client, device, identifier, kind):
        from PIL import ImageGrab
        check_cloud_stroke_model()
        # A physical cloud click leaves the pointer over a long native title.
        # GTK can paint that tooltip over the next row's SVG. Move the real
        # pointer to empty CEF content before requiring unobstructed glyph ink.
        label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        origin = client.command('plugin:window|inner_position', label=label)
        point = client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();'
                          'return {x:r.x+r.width*.85,y:r.y+r.height*.8,k:devicePixelRatio||1}})()')
        subprocess.run(['xdotool', 'mousemove', str(round(origin['x'] + point['x'] * point['k'])),
                        str(round(origin['y'] + point['y'] * point['k']))],
                       env={**os.environ, 'DISPLAY': device.display}, check=True)
        painted(client)
        value = client.js('(()=>{const e=document.querySelector(' +
                          json.dumps('[data-super-row="' + identifier + '"] .super-cloud-badge') + ');'
                          'const svg=e?.querySelector("svg");if(!svg)return null;const r=svg.getBoundingClientRect();'
                          'return {x:r.x,y:r.y,w:r.width,h:r.height,k:devicePixelRatio||1,'
                          'rgb:getComputedStyle(svg).color.match(/\\d+/g).slice(0,3).map(Number)}})()')
        require(value and value['w'] >= 10 and value['h'] >= 10, 'Native cloud status icon is not visible')
        red, green, blue = value['rgb']
        require({'green': green > red * 1.15 and green > blue * 1.05,
                 'red': red > green * 1.25 and red > blue * 1.15,
                 'gray': max(red, green, blue) - min(red, green, blue) <= 55}[kind],
                'Cloud status icon does not use the expected ' + kind + ' color')
        label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        origin = client.command('plugin:window|inner_position', label=label)
        box = (round(origin['x'] + value['x'] * value['k']), round(origin['y'] + value['y'] * value['k']),
               round(origin['x'] + (value['x'] + value['w']) * value['k']),
               round(origin['y'] + (value['y'] + value['h']) * value['k']))
        # DOM/layout RAF can complete before the window compositor paints a
        # collapsed sidebar. Require the same actual colored ink, with a bounded
        # wait for native presentation rather than accepting computed color alone.
        deadline = time.monotonic() + 6
        ink = {'ink': 0}
        while time.monotonic() < deadline:
            pixels = ImageGrab.grab(xdisplay=device.display).crop(box).convert('RGB')
            ink = cloud_stroke_ink(pixels, value['rgb'])
            if ink['ink'] >= 3 and ink['spread']:
                return
            time.sleep(.1)
        pixels.save(root / ('cloud-' + identifier + '-' + kind + '-paint-failure.png'))
        require(False, 'Native cloud icon color exists in DOM but was not painted on screen: ' +
                json.dumps({'identifier': identifier, 'kind': kind, 'crop': box, 'color': value['rgb'], 'ink': ink}))

    def cloud_layouts(client, device, stage, kind='green'):
        from PIL import ImageGrab
        for collapsed in (False, True):
            desired = '.okno.panel-svernuta' if collapsed else '.okno:not(.panel-svernuta)'
            if not client.js('!!document.querySelector(' + json.dumps(desired) + ')'):
                client.click('[aria-label="' + ('Свернуть левую панель' if collapsed else 'Развернуть левую панель') + '"]')
            js_wait(client, '!!document.querySelector(' + json.dumps(desired) + ')', 'Panel did not change layout')
            painted(client)
            require(client.js('(()=>{const e=document.querySelector(`[data-super-row="native-credentials"] .super-cloud-badge`);'
                              'if(!e)return false;const r=e.getBoundingClientRect(),s=getComputedStyle(e);'
                              'return r.width>=10&&r.height>=10&&r.top>=0&&r.bottom<=innerHeight&&'
                              's.display!=="none"&&s.visibility!=="hidden"&&Number(s.opacity)>0&&!!e.querySelector("svg")})()'),
                    'Cloud marker is not visible in ' + ('collapsed' if collapsed else 'expanded') + ' panel')
            cloud_painted(client, device, 'native-credentials', kind)
            require(client.js('!document.querySelector(`[data-super-row="local-cloud-ui"] .super-cloud-badge`)'),
                    'Local Super has an unwanted cloud control')
            require(client.js('(()=>{const collapsed=' + json.dumps(collapsed) + ';'
                              'const ids=["native-credentials","retained-super","local-cloud-ui","pvt-cloud-ui"];'
                              'return ids.every(id=>{const row=document.querySelector(`[data-super-id="${id}"]`);'
                              'if(!row)return false;const full=row.querySelector(".imya"),short=row.querySelector(".kratkoe-imya");'
                              'if(!full||!short||short.textContent!==Array.from(full.textContent).slice(0,3).join(""))return false;'
                              'const name=collapsed?short:full,r=name.getBoundingClientRect(),style=getComputedStyle(name);'
                              'return r.width>0&&r.height>0&&r.top>=0&&r.bottom<=innerHeight&&'
                              'style.display!=="none"&&style.visibility!=="hidden"&&Number(style.opacity)>0})})()'),
                    'Super names are not visible/full or three-letter in the expected panel layout')
            require(client.js('(()=>{const row=document.querySelector(`[data-super-id="pvt-cloud-ui"]`);'
                              'const lock=row?.querySelector(".super-private-badge");if(!lock)return false;'
                              'const r=lock.getBoundingClientRect(),style=getComputedStyle(lock);'
                              'return row.dataset.cloudState==="local"&&!row.querySelector(".super-cloud-badge")&&'
                              'r.width>=8&&r.height>=8&&style.display!=="none"&&style.visibility!=="hidden"&&'
                              'Number(style.opacity)>0&&!!lock.querySelector("svg")})()'),
                    'Private Super lost its lock or acquired a sync warning')
            label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
            origin = client.command('plugin:window|inner_position', label=label)
            bounds = client.js('(()=>{const r=document.querySelector(".reika").getBoundingClientRect();'
                               'return {x:r.x,y:r.y,w:r.width,h:r.height,k:devicePixelRatio||1}})()')
            box = (round(origin['x'] + bounds['x'] * bounds['k']), round(origin['y'] + bounds['y'] * bounds['k']),
                   round(origin['x'] + (bounds['x'] + bounds['w']) * bounds['k']),
                   round(origin['y'] + (bounds['y'] + bounds['h']) * bounds['k']))
            ImageGrab.grab(xdisplay=device.display).crop(box).save(root / (stage + ('-collapsed.png' if collapsed else '-expanded.png')))
        # Leave the shell as it was before these checks; settings are also a
        # selected sync category, so changing layout must not poison later runs.
        client.click('[aria-label="Развернуть левую панель"]')
        js_wait(client, '!!document.querySelector(".okno:not(.panel-svernuta)")', 'Expanded panel did not restore')

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

    def visible_cef(client, device, stage):
        from PIL import ImageGrab
        label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        title=client.command('plugin:window|title',label=label)
        require(isinstance(title,str) and title.startswith('NexusBro'),'Native shell title is unavailable: '+stage)
        native=subprocess.check_output(['xdotool','search','--onlyvisible','--name','^'+re.escape(title)+'$'],env={**os.environ,'DISPLAY':device.display},text=True).splitlines()[0]
        origin=client.command('plugin:window|inner_position',label=label)
        deadline=time.monotonic()+12
        colors=[]
        while time.monotonic()<deadline:
            subprocess.run(['xdotool','windowraise',native],env={**os.environ,'DISPLAY':device.display},check=True)
            area=client.js('(()=>{const r=document.querySelector(".pole")?.getBoundingClientRect();return r&&{x:r.x,y:r.y,w:r.width,h:r.height,k:devicePixelRatio||1}})()')
            require(area and area['w']>400 and area['h']>200,'Native CEF viewport is missing: '+stage)
            frame=ImageGrab.grab(xdisplay=device.display)
            points=[(round(origin['x']+(area['x']+area['w']*x)*area['k']),round(origin['y']+(area['y']+area['h']*y)*area['k'])) for x,y in [(.55,.55),(.7,.7),(.85,.8)]]
            colors=[frame.getpixel(point)[:3] for point in points]
            if all(color==(24,85,65) for color in colors):
                frame.save(root/(stage+'.png'))
                checks.append({stage:True,'native_cef_rgb':colors})
                return
            time.sleep(.1)
        frame.save(root/(stage+'-failure.png'))
        raise NativeFailure('Native CEF page did not repaint: '+stage+' '+str(colors))

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
        server_environment = {**os.environ,
            'NEXUSBRO_SYNC_BIND': '127.0.0.1:' + str(sync_port),
            'NEXUSBRO_SYNC_DB': str(sync_root / 'nexusbro-sync.db'),
            'NEXUSBRO_SYNC_TOTP_KEY': deployment_key,
            'RUST_LOG': 'info'}
        server_log = stream
        server_process = subprocess.Popen([str(server_binary)], env=server_environment,
                                          stdout=server_log, stderr=subprocess.STDOUT, start_new_session=True)
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
        # Dormant synthetic rows exercise cloud grouping without launching more
        # CEF processes. Neither row is selected for sync, including the private one.
        (a.data / 'vkladki.json').write_text(json.dumps([
            super_record('local-cloud-ui', 'Local cloud UI fixture', 'about:blank'),
            super_record('pvt-cloud-ui', 'Private cloud UI fixture', 'about:blank', private=True)]))
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
        phase = 'native-updater-visible-sites'
        from updater_native import verify_updater
        verify_updater(ca, a, visible_cef, checks)
        phase = 'native-capture'
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
        phase = 'repeated-origin-capture'
        # Reproduce the observed post-origin-transition failure without retrying
        # a failed capture or weakening origin/context validation.
        for transition in range(3):
            a.navigate(normal_id, other_origin + '/form?case=transition-other-' + str(transition) + '&mode=empty')
            marker='transition-offer-' + str(transition)
            a.navigate(normal_id, base + '/form?case=' + marker + '&mode=offer')
            wait_for(lambda marker=marker: seen(marker), 'Transition fixture did not initialize')
            draft=call(ca,'security_passwords',action='capture',data={'super_id':normal_id,'tab_id':normal_tab})
            require(draft.get('origin')==base and draft.get('username')==offer_login,'Origin transition captured the wrong document')
            call(ca,'security_passwords',action='discard_draft',data={'draft_id':draft['draft_id']})
        checkpoint('repeated_real_origin_transitions_preserve_native_capture_context')
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
        phase = 'native-visible-qr-enrollment'
        ca.js('window.dispatchEvent(new Event("nexusbro-security"));true')
        js_wait(ca, '!!document.querySelector(".security-panel")', 'Security settings did not reopen for 2FA')
        ca.text_button('Аккаунт')
        js_wait(ca, '!!document.querySelector(".security-account-status")', 'Logged-in account interface did not render')
        require(not call(ca, 'security_account', action='status')['totp_enabled'], 'Fresh account already has 2FA enabled')
        ca.js('Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()==="Настроить двухфакторную аутентификацию")?.scrollIntoView({block:"center"});true')
        painted(ca)
        ca.text_button('Настроить двухфакторную аутентификацию')
        js_wait(ca, '!!document.querySelector(`canvas[data-testid="totp-qr"]`)', 'Enrollment did not display its local QR')
        uri = visible_qr_uri(ca, a)
        sensitive.add(uri)
        parsed = urllib.parse.urlsplit(uri)
        options = urllib.parse.parse_qs(parsed.query)
        secret = options.get('secret', [''])[0]
        if secret:
            sensitive.add(secret)
        require(parsed.scheme == 'otpauth' and parsed.netloc == 'totp' and secret,
                'QR provisioning URI is incomplete')
        require(options.get('issuer') == ['NexusBro'] and username in urllib.parse.unquote(parsed.path),
                'Visible QR identifies the wrong issuer or synthetic account')
        require(options.get('algorithm', ['SHA1']) == ['SHA1'] and options.get('digits', ['6']) == ['6']
                and options.get('period', ['30']) == ['30'], 'Unsupported provisioning parameters')
        manual_details = 'details:has([data-testid="totp-manual-key"])'
        scroll_visible(ca, manual_details + ' summary')
        ca.click(manual_details + ' summary')
        js_wait(ca, '!!document.querySelector(' + json.dumps(manual_details + '[open]') + ')',
                'Manual setup key did not open through the native UI')
        manual_key = ca.js('document.querySelector(`[data-testid="totp-manual-key"]`)?.innerText?.replace(/\\s/g,"")')
        require(manual_key == secret, 'Scanned QR and visible manual setup key disagree')
        require(not call(ca, 'security_account', action='status')['totp_enabled'],
                'Displaying the QR enabled 2FA without verification')
        # Enter a real time-based code through the actual native input, not IPC.
        # Stay clear of the period boundary while xdotool types and HTTP verifies.
        if time.time() % 30 > 25:
            time.sleep(30 - time.time() % 30 + .2)
        code = totp(secret)
        sensitive.add(code)
        scroll_visible(ca, '[aria-label="Код подтверждения 2FA"]')
        ca.fill('[aria-label="Код подтверждения 2FA"]', code)
        ca.js('Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()==="Подтвердить и включить 2FA")?.scrollIntoView({block:"center"});true')
        painted(ca)
        ca.text_button('Подтвердить и включить 2FA')
        wait_for(lambda: call(ca, 'security_account', action='status')['totp_enabled'],
                 'Real server did not verify the code from the visible QR', 30)
        js_wait(ca, 'document.querySelector(".security-account-status")?.textContent.includes("Двухфакторная аутентификация: включена")',
                'Account UI did not reflect successful real TOTP verification')
        js_wait(ca, '!document.querySelector(`[data-testid="totp-qr"]`)', 'Verified QR stayed on screen')
        backup_codes = ca.js('Array.from(document.querySelectorAll(".security-backup-codes code"),e=>e.textContent)')
        require(backup_codes, 'Real TOTP verification did not issue recovery codes')
        sensitive.update(backup_codes)
        ca.js('Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()==="Резервные коды сохранены")?.scrollIntoView({block:"center"});true')
        painted(ca)
        ca.text_button('Резервные коды сохранены')
        ca.click('[aria-label="Закрыть пароли и синхронизацию"]')
        checkpoint('native_displayed_qr_screen_pixels_decoded_real_totp_ui_verified')
        phase = 'mfa-revokes-other-device'
        b_off_selection = {'settings': False, 'bookmarks': False, 'supers': [], 'passwords': []}
        require(call(cb, 'security_sync', action='status')['selection'] == b_off_selection,
                'Second device sync selection changed before MFA reauthentication')
        expired = call(cb, 'security_sync', action='run', error=True, timeout=90)
        require('ACCOUNT_SESSION_EXPIRED' in expired,
                'Enabling MFA did not revoke the other real device session')
        cb.js('window.dispatchEvent(new Event("nexusbro-security"));true')
        js_wait(cb, '!!document.querySelector(".security-panel")', 'Second device security settings did not reopen')
        cb.text_button('Аккаунт')
        # Expired-session handling normally clears the native cached session.
        # If the UI still has its earlier snapshot, use its real logout action.
        if call(cb, 'security_account', action='status')['logged_in']:
            js_wait(cb, '!!document.querySelector(".security-account-status")', 'Cached account interface did not render')
            cb.text_button('Выйти из аккаунта')
        js_wait(cb, '!!document.querySelector(".security-account-form")', 'Revoked device did not offer real account login')
        cb.fill('[aria-label="Имя аккаунта NexusBro"]', username)
        cb.fill('[aria-label="Пароль аккаунта NexusBro"]', account_password)
        scroll_visible(cb, '[aria-label="Код 2FA для входа"]')
        cb.fill('[aria-label="Код 2FA для входа"]', backup_codes[0])
        cb.text_button('Войти в NexusBro')
        wait_for(lambda: call(cb, 'security_account', action='status')['logged_in'],
                 'Revoked device could not log in through UI with its real recovery code', 45)
        require(call(cb, 'security_account', action='status')['totp_enabled'],
                'Second device login lost the enabled MFA state')
        require(call(cb, 'security_sync', action='status')['selection'] == b_off_selection,
                'MFA reauthentication silently opted the second device into sync')
        cb.click('[aria-label="Закрыть пароли и синхронизацию"]')
        checkpoint('mfa_revokes_other_device_and_real_recovery_login')
        phase = 'first-selected-sync'
        cloud_refresh(ca)
        cloud_state(ca, selected='local')
        call(ca, 'zakladki_sohranit', adres='https://selected-bookmark.fixture.invalid/', nazvanie='Selected encrypted bookmark', papka='')
        selection_a = {'settings': True, 'bookmarks': True, 'supers': [normal_id, 'retained-super'], 'passwords': [credential_id]}
        call(ca, 'security_sync', action='configure', data={'selection': selection_a})
        cloud_refresh(ca)
        cloud_state(ca, selected='pending')
        private_selection = {**selection_a, 'supers': ['pvt-native-private']}
        call(ca, 'security_sync', action='configure', data={'selection': private_selection}, error=True)
        first_report = cloud_click_sync(ca, device=a)
        report_ok(first_report)
        require(first_report['pushed'] >= 5, 'Selected native records were not actually uploaded')
        cloud_refresh(ca)
        cloud_state(ca, selected='selected', receipt=True)
        cloud_layouts(ca, a, 'cloud-selection-after-real-sync')
        checkpoint('native_cloud_local_private_pending_selected_real_account_receipt_both_panel_layouts')
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
        if 'nexusbro.v1/super/retained-super' in imported['conflicts']:
            from PIL import ImageGrab
            # This warning must come from the real native import conflict; do
            # not manufacture a status merely to exercise the sidebar glyph.
            cloud_refresh(cb)
            js_wait(cb, 'document.querySelector(`[data-super-id="retained-super"][data-cloud-saved="true"]`)?.dataset.cloudState==="conflict"',
                    'Real imported Super conflict did not reach the sidebar')
            painted(cb)
            require(cb.js('(()=>{const row=document.querySelector(`[data-super-id="retained-super"]`);'
                          'const badge=row?.closest(`[data-super-row]`)?.querySelector(`.super-cloud-badge[data-cloud-state="conflict"]`);'
                          'if(!badge)return false;const r=badge.getBoundingClientRect(),style=getComputedStyle(badge);'
                          'return row.closest("[data-cloud-group]")?.dataset.cloudGroup==="cloud"&&'
                          'r.width>=10&&r.height>=10&&r.top>=0&&r.bottom<=innerHeight&&'
                          'style.display!=="none"&&style.visibility!=="hidden"&&Number(style.opacity)>0&&'
                          '!!badge.querySelector("svg")})()'),
                    'Real conflict warning glyph is not visible in the cloud group')
            ImageGrab.grab(xdisplay=b.display).save(root / 'cloud-real-conflict-before-resolve.png')
            checkpoint('native_sidebar_real_conflict_warning_preserved')
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
            def extra_ready():
                try:return call(extra,'obychnye_spisok',id=normal_id)
                except NativeFailure as error:
                    if 'ещё запускается' in str(error):return None
                    raise
            extra_session=wait_for(extra_ready,'Additional browser session did not initialize')
            call(extra,'obychnaya_komanda',id=normal_id,komanda={'deystvie':'pereyti','id':extra_session['aktivnaya'],'adres':base+'/form?case=extra-lock&mode=offer'})
            visible_cef(ca,a,'main-page-before-browser-lock')
            visible_cef(extra,a,'extra-page-before-browser-lock')
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
            visible_cef(ca,a,'main-page-after-browser-unlock')
            visible_cef(extra,a,'extra-page-after-browser-unlock')
            for restored_client in [ca,extra]:
                session=call(restored_client,'obychnye_spisok',id=normal_id)
                address=next(tab['adres'] for tab in session['vkladki'] if tab['id']==session['aktivnaya'])
                marker=urllib.parse.parse_qs(urllib.parse.urlsplit(address).query)['case'][0]
                previous=len(seen(marker,'submit'))
                label=restored_client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
                title=restored_client.command('plugin:window|title',label=label)
                native=subprocess.check_output(['xdotool','search','--onlyvisible','--name','^'+re.escape(title)+'$'],env={**os.environ,'DISPLAY':a.display},text=True).splitlines()[0]
                subprocess.run(['xdotool','windowraise',native],env={**os.environ,'DISPLAY':a.display},check=True)
                point=restored_client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return [r.x+200,r.y+160]})()')
                restored_client.mouse(*point)
                wait_for(lambda:len(seen(marker,'submit'))>previous,'Restored CEF renderer did not handle a physical form submit')
            checkpoint('all_native_cef_views_repaint_and_accept_trusted_input_after_browser_unlock')
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
        require(call(ca, 'security_account', action='status')['totp_enabled'], 'Verified 2FA did not survive native app restart')
        remaining = call(ca, 'security_passwords', action='list')
        require(len(remaining) == 1 and remaining[0]['username'] == offer_login, 'Local-only credential did not persist independently')
        require(call(ca, 'security_sync', action='status')['selection'] == selection_a, 'Granular sync selection did not persist')
        cloud_state(ca, selected='selected', receipt=True)
        cloud_layouts(ca, a, 'cloud-selection-after-restart')
        checkpoint('native_cloud_selection_real_receipt_and_panel_markers_survive_restart')
        checkpoint('restart_locks_key_persists_encrypted_session_selection_local_only_password')
        phase = 'cloud-record-ack-is-not-account-date'
        old_receipt = call(ca, 'security_sync', action='status')['last_sync']
        edited_super = next(row for row in call(ca, 'spisok') if row['id'] == normal_id)
        edited_super['nazvanie'] = 'Native metadata changed after authenticated ACK'
        call(ca, 'izmenit', nastrojki=edited_super)
        cloud_refresh(ca)
        cloud_state(ca, selected='pending', saved_state='selected', receipt=True)
        require(call(ca, 'security_sync', action='status')['last_sync'] == old_receipt,
                'Local Super edit fabricated a new account receipt')
        painted(ca)
        cloud_painted(ca, a, normal_id, 'gray')
        changed_report = cloud_click_sync(ca, device=a, selector='[data-super-row="native-credentials"] button.super-cloud-badge')
        report_ok(changed_report)
        require(changed_report['pushed'] >= 1, 'Changed portable Super was not actually sent to the server')
        cloud_refresh(ca)
        cloud_state(ca, selected='selected', receipt=True)
        checkpoint('native_local_super_hash_change_is_pending_despite_historical_account_receipt_actual_exchange_restores_green')
        phase = 'cloud-disable-saved-moves-local-without-delete'
        local_before = {name: (a.data / name).read_bytes() for name in ('vkladki.json', 'sohranennye.json')}
        saved_before = call(ca, 'sohranennye')
        user_id = call(ca, 'security_account', action='status')['user_id']

        def remote_saved_record():
            # Read only this test's private DB. Opaque ciphertext/revision stays
            # in memory; never inspect or contact a production service.
            with sqlite3.connect('file:' + str(sync_root / 'nexusbro-sync.db') + '?mode=ro', uri=True) as database:
                return database.execute('SELECT ciphertext,rev,deleted FROM items WHERE user_id=? AND item_id=?',
                                        (user_id, 'nexusbro.v1/super/retained-super')).fetchone()

        remote_before = remote_saved_record()
        require(remote_before and remote_before[2] == 0, 'Selected saved Super has no live authenticated server record')
        disabled_selection = {**selection_a, 'supers': [normal_id]}
        call(ca, 'security_sync', action='configure', data={'selection': disabled_selection})
        cloud_refresh(ca)
        js_wait(ca, '(()=>{const e=document.querySelector(`[data-super-id="retained-super"]`);'
                    'return e?.dataset.cloudSaved==="true"&&e.dataset.cloudState==="local"&&'
                    'e.closest("[data-cloud-group]")?.dataset.cloudGroup==="local"})()',
                'Disabled saved Super disappeared instead of moving to the local group')
        painted(ca)
        require(ca.js('!document.querySelector(`[data-super-row="retained-super"] .super-cloud-badge`)'),
                'Disabled local saved Super retained a cloud button')
        report_ok(call(ca, 'security_sync', action='run', timeout=90))
        require(call(ca, 'sohranennye') == saved_before and
                all((a.data / name).read_bytes() == raw for name, raw in local_before.items()),
                'Disabling sync changed or removed local Super data')
        require(remote_saved_record() == remote_before,
                'Disabling sync deleted or changed the opaque server record')
        require(not any(row['id'] == 'retained-super' for row in call(ca, 'spisok')),
                'Moving a disabled saved Super started its CEF engine')
        call(ca, 'security_sync', action='configure', data={'selection': selection_a})
        report_ok(call(ca, 'security_sync', action='run', timeout=90))
        cloud_refresh(ca)
        cloud_state(ca, selected='selected', receipt=True)
        checkpoint('native_disabled_closed_saved_super_remains_local_no_cloud_no_delete_reenable_actual_ack_green')
        phase = 'cloud-real-network-error'
        server_process.terminate()
        try:
            server_process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server_process.kill()
            server_process.wait(timeout=3)
        cloud_click_sync(ca, device=a, expected='error')
        require(call(ca, 'security_account', action='status')['logged_in'], 'Network outage unexpectedly revoked the account session')
        cloud_refresh(ca)
        cloud_state(ca, selected='error', receipt=True)
        cloud_layouts(ca, a, 'cloud-real-network-error', kind='red')
        checkpoint('native_real_stopped_isolated_server_exchange_is_red_without_false_green_from_old_receipts')
        phase = 'cloud-network-error-persists-after-restart'
        a.stop_app()
        ca = a.start()
        js_wait(ca, '!!document.querySelector(".browser-lock-screen")', 'Restarted app did not require its existing master password')
        ca.fill('[aria-label="Мастер-пароль для разблокировки браузера"]', master_a)
        ca.text_button('Разблокировать браузер')
        js_wait(ca, '!document.querySelector(".browser-lock-screen")&&!!document.querySelector(".okno")',
                'Master password did not unlock the app after the real network error')
        cloud_state(ca, selected='error', receipt=True)
        cloud_layouts(ca, a, 'cloud-real-network-error-after-restart', kind='red')
        checkpoint('native_failed_exchange_red_persists_across_real_app_restart')
        phase = 'cloud-same-server-recovery'
        server_process = subprocess.Popen([str(server_binary)], env=server_environment,
                                          stdout=server_log, stderr=subprocess.STDOUT, start_new_session=True)
        processes.append(server_process)
        wait_for(health, 'Same isolated server DB and deployment key did not restart', 20)
        report_ok(cloud_click_sync(ca, device=a, selector='[data-super-row="retained-super"] button.super-cloud-badge'))
        cloud_refresh(ca)
        cloud_state(ca, selected='selected', receipt=True)
        cloud_layouts(ca, a, 'cloud-real-network-recovered')
        checkpoint('native_same_db_and_secrets_authenticated_server_recovery_restores_green')
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
        # Keep only fixed bridge stage/reason metadata. Profiles and arbitrary
        # engine output may contain test secrets and are not copied wholesale.
        bridge=[]
        for device in devices:
            for logfile in device.data.glob('profili/*/cef*.log'):
                for line in logfile.read_text(errors='replace').splitlines():
                    found=re.search(r'NEXUSBRO_PASSWORD_BRIDGE refused stage=(\d+) setup=(true|false) reason=([a-z0-9-]+)',line)
                    if found:bridge.append({'device':device.name,'stage':int(found[1]),'setup':found[2]=='true','reason':found[3]})
        if bridge:checks.append({'native_credential_bridge_failure_metadata':bridge[-32:]})
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
