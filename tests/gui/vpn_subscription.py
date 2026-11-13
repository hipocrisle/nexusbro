"""CEF → HTTP proxy → xray VLESS → local page, with GUI subscription selection.
Run from repository root, after npm/cargo build, under a separate Xvfb.
No user subscriptions or external endpoints are used.
"""
import argparse
import http.server
import json
import os
import pathlib
import socket
import subprocess
import threading
import time
from client import Client
from websocket import WebSocketException

parser = argparse.ArgumentParser()
parser.add_argument('--display', default=':94')
parser.add_argument('--port', type=int, default=9223)
parser.add_argument('--out', default='.dev-check/vpn-subscription.json')
args = parser.parse_args()
root = pathlib.Path.cwd()
run = root / '.dev-check' / f'vpn-{time.time_ns()}'
run.mkdir(parents=True)
uuid = '11111111-2222-3333-4444-555555555555'
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    vless_port = sock.getsockname()[1]
first = f'vless://{uuid}@127.0.0.1:1?security=none&type=tcp#Unavailable'
chosen = f'vless://{uuid}@127.0.0.1:{vless_port}?security=none&type=tcp#Local'

class Fixture(http.server.BaseHTTPRequestHandler):
    removed = False
    hits = 0
    def do_GET(self):
        if self.path == '/sub':
            body = (first + ('\n' + chosen if not type(self).removed else '')).encode()
        elif self.path == '/fail':
            self.send_error(503)
            return
        else:
            type(self).hits += 1
            body = b'<!doctype html><title>ViaSelectedVLESS</title><body style="background:green">Local VPN fixture</body>'
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_args): pass

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
threading.Thread(target=server.serve_forever, daemon=True).start()
base = f'http://127.0.0.1:{server.server_port}'
config = {'log': {'loglevel': 'warning'}, 'inbounds': [{'listen': '127.0.0.1', 'port': vless_port,
    'protocol': 'vless', 'settings': {'clients': [{'id': uuid}], 'decryption': 'none'},
    'streamSettings': {'network': 'tcp', 'security': 'none'}}], 'outbounds': [{'tag': 'blocked', 'protocol': 'blackhole'}, {'tag': 'fixture', 'protocol': 'freedom',
        'settings': {'redirect': f'127.0.0.1:{server.server_port}'}}],
    'routing': {'rules': [{'type': 'field', 'domain': ['full:vpn-fixture.test'], 'outboundTag': 'fixture'}]}}
(run / 'server.json').write_text(json.dumps(config))
xray_log = (run / 'xray.log').open('w')
xray = subprocess.Popen([str(root/'desktop/src-tauri/binaries/xray'), 'run', '-c', str(run/'server.json')], stdout=xray_log, stderr=xray_log)
env = {**os.environ, 'DISPLAY': args.display, 'CEF_PATH': str(root/'desktop/src-tauri/cef'),
    'XDG_DATA_HOME': str(run/'data'), 'XDG_CONFIG_HOME': str(run/'config'), 'XDG_CACHE_HOME': str(run/'cache'),
    'WEBKIT_INSPECTOR_HTTP_SERVER': f'127.0.0.1:{args.port}', 'WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS': '1'}
app = None
client = None
log = None
result = {}

def start():
    global app, client, log
    with socket.socket() as probe:
        assert probe.connect_ex(('127.0.0.1', args.port)) != 0, 'Inspector port is occupied'
    log = (run/f'app-{time.time_ns()}.log').open('w')
    app = subprocess.Popen([str(root/'desktop/src-tauri/target/debug/nexusbro')], env=env, stdout=log, stderr=log)
    for _ in range(200):
        assert app.poll() is None, 'Application exited'
        try:
            client = Client(args.display, args.port)
            break
        except (OSError, ConnectionError, WebSocketException): time.sleep(.1)
    assert client
    wins = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--name', '^NexusBro$'], env=env, text=True).splitlines()
    subprocess.run(['xdotool', 'windowfocus', wins[0]], env=env, check=True)
    client.wait('!!document.querySelector(".niz")')

def stop():
    try: client.js('window.__TAURI_INTERNALS__.invoke("plugin:process|exit",{code:0});true')
    except WebSocketException: pass
    app.wait(timeout=8)
    assert app.returncode == 0
    client.close()
    log.close()
    time.sleep(.3)

def wait_page(id):
    for _ in range(200):
        tabs = client.command('obychnye_spisok', id=id)
        if tabs and any(t['zagolovok'] == 'ViaSelectedVLESS' for t in tabs['vkladki']): return
        time.sleep(.05)
    raise AssertionError(tabs)

try:
    start()
    client.click('.niz button:first-child')
    client.wait('!!document.querySelector(".dialog")')
    client.fill('.dialog input', 'VPN fixture')
    client.fill('.dialog label:nth-of-type(2) input', base+'/sub')
    client.click('.zagruzit-uzly')
    client.wait('document.querySelectorAll(".uzel-podpiski option").length===3')
    client.click('.uzel-podpiski')
    subprocess.run(['xdotool', 'key', 'End', 'Return'], env=env, check=True)
    client.wait('document.querySelector(".uzel-podpiski").value==='+json.dumps(chosen))
    client.fill('.dialog label:nth-of-type(4) input', 'http://vpn-fixture.test/page')
    client.click('.dialog .glavnaya')
    client.wait('!document.querySelector(".dialog")')
    id = client.command('spisok')[0]['id']
    client.wait('!!document.querySelector(".navigaciya input") && !document.querySelector(".navigaciya input").disabled')
    client.fill('.navigaciya input', 'http://vpn-fixture.test/page', enter=True)
    wait_page(id)
    record = json.loads((run/'data/NexusBro/vkladki.json').read_text())[0]
    assert record['proxy'] == base+'/sub' and record['proxy_uzel'] == chosen, record
    profile = run/'data/NexusBro/profili'/id
    actual = json.loads((profile/'xray.json').read_text())
    assert actual['outbounds'][0]['settings']['vnext'][0]['port'] == vless_port
    assert Fixture.hits > 0
    assert 'tcp:vpn-fixture.test:80' in (run/'xray.log').read_text()
    result['physical_selection_and_real_cef_vless_page'] = True
    stop()
    start()
    wait_page(id)
    result['selection_persisted_across_restart'] = True
    client.click('.vkladka', 3)
    client.text_button('Настройки')
    client.wait('!!document.querySelector(".uzel-podpiski")')
    assert client.js('document.querySelector(".uzel-podpiski").value') == chosen
    client.click('.zagruzit-uzly')
    client.wait('document.querySelectorAll(".uzel-podpiski option").length===3')
    assert client.js('document.querySelector(".uzel-podpiski").value') == chosen
    client.text_button('Отмена')
    client.wait('!document.querySelector(".zatemnenie")')
    result['edit_dialog_preserves_selection_after_refresh'] = True
    # The selected node disappears: never silently connect to the first node.
    client.command('zakryt', id=id, sohranit=True)
    Fixture.removed = True
    client.click('.niz button:nth-child(2)')
    client.wait('!!document.querySelector(".stroka-menyu")')
    client.click('.stroka-menyu')
    client.wait('document.body.innerText.includes("выбранный узел исчез")', timeout=15)
    for proc in pathlib.Path('/proc').iterdir():
        if not proc.name.isdigit(): continue
        try:
            cmd = (proc/'cmdline').read_bytes()
            assert not (str(profile).encode() in cmd and (b'--nb-dvizhok' in cmd or b'xray.json' in cmd)), cmd
        except OSError: pass
    result['removed_node_fails_without_fallback_or_process_leak'] = True
    # Network failure remains in the dialog and does not block cancel.
    client.click('.niz button:first-child')
    client.fill('.dialog label:nth-of-type(2) input', base+'/fail')
    client.click('.zagruzit-uzly')
    client.wait('!!document.querySelector(".dialog [role=alert]")')
    client.text_button('Отмена')
    client.wait('!document.querySelector(".zatemnenie")')
    result['subscription_error_and_cancel'] = True
    stop()
    result['success'] = True
finally:
    if app and app.poll() is None:
        app.terminate()
        try: app.wait(timeout=8)
        except subprocess.TimeoutExpired: app.kill(); app.wait()
    xray.terminate(); xray.wait(timeout=5); xray_log.close()
    server.shutdown()
    pathlib.Path(args.out).write_text(json.dumps(result, ensure_ascii=False, indent=2)+'\n')
    print(json.dumps(result, ensure_ascii=False))
