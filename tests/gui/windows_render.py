"""Windows runtime: real Tauri/WebView2 + CEF and desktop pixel acceptance.
Requires an interactive Windows desktop, websocket-client and Pillow.
"""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
import pathlib
import subprocess
import time
import urllib.parse
import urllib.request
import websocket
from PIL import ImageGrab

parser = argparse.ArgumentParser()
parser.add_argument('--exe', required=True)
parser.add_argument('--cef', required=True)
parser.add_argument('--out', default='.dev-check/windows-render')
parser.add_argument('--port', type=int, default=9224)
args = parser.parse_args()
assert os.name == 'nt', 'Windows desktop is required'
# Use device pixels, matching the rectangles sent by the frontend.
user32 = ctypes.windll.user32
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowW.restype = wintypes.HWND
user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint]
user32.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
user32.SetForegroundWindow.argtypes = [wintypes.HWND]
run = pathlib.Path(args.out).resolve()
run.mkdir(parents=True, exist_ok=True)
cef = pathlib.Path(args.cef).resolve()
env = {**os.environ, 'CEF_PATH': str(cef), 'PATH': str(cef)+os.pathsep+os.environ['PATH'],
       'NEXUSBRO_TEST_DATA': str(run/'data'),
       'WEBVIEW2_USER_DATA_FOLDER': str(run/'webview2'),
       'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS': f'--remote-debugging-port={args.port} --remote-allow-origins=*'}
app = None
ws = None
seq = 0
results = []
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

def js(expression):
    global seq
    seq += 1
    ws.send(json.dumps({'id': seq, 'method': 'Runtime.evaluate', 'params': {
        'expression': expression, 'returnByValue': True, 'awaitPromise': True}}))
    while True:
        response = json.loads(ws.recv())
        if response.get('id') != seq: continue
        assert 'error' not in response, response
        assert 'exceptionDetails' not in response['result'], response
        return response['result']['result'].get('value')

def command(name, **params):
    response = js('window.__TAURI_INTERNALS__.invoke('+json.dumps(name)+','+json.dumps(params)+')')
    if isinstance(response, dict) and 'ok' in response: assert response['ok'], response
    return response

def until(expression, timeout=20):
    deadline = time.monotonic()+timeout
    while time.monotonic() < deadline:
        result = js(expression)
        if result: return result
        time.sleep(.05)
    raise AssertionError(expression)

def page_pixel(stage):
    deadline = time.monotonic()+20
    while time.monotonic() < deadline:
        points = js('(()=>{let r=document.querySelector(".pole").getBoundingClientRect();return [20,r.width/2,r.width-20].map(x=>[(r.x+x)*devicePixelRatio,(r.y+r.height/2)*devicePixelRatio])})()')
        screen_points = []
        for x, y in points:
            point = wintypes.POINT(round(x), round(y))
            assert user32.ClientToScreen(hwnd, ctypes.byref(point))
            screen_points.append((point.x, point.y))
        # The test moves its only window onto the primary display.
        screen = ImageGrab.grab()
        rgb = [screen.getpixel(p)[:3] for p in screen_points]
        if all(color == (24, 85, 65) for color in rgb):
            screen.save(run/f'{stage}.png')
            results.append({'stage': stage, 'screen_rgb': rgb})
            return
        time.sleep(.1)
    screen.save(run/f'{stage}-failure.png')
    raise AssertionError(f'{stage}: actual desktop pixel {rgb}, expected (24,85,65)')

assert not (run/'data').exists(), 'Use a fresh output directory for every run'
try:
    app = subprocess.Popen([str(pathlib.Path(args.exe).resolve())], env=env)
    deadline = time.monotonic()+60
    while time.monotonic() < deadline:
        assert app.poll() is None, f'Application exited {app.returncode}'
        try:
            targets = json.load(opener.open(f'http://127.0.0.1:{args.port}/json/list', timeout=2))
            (run/'cdp-targets.json').write_text(json.dumps(targets, indent=2), encoding='utf-8')
            target = next(t for t in targets if t['type'] == 'page' and 'webSocketDebuggerUrl' in t and ('tauri' in t.get('url', '') or t.get('title') == 'NexusBro'))
            ws = websocket.create_connection(target['webSocketDebuggerUrl'], timeout=15, http_no_proxy=['127.0.0.1', 'localhost'], suppress_origin=True)
            if js('!!window.__TAURI_INTERNALS__'): break
            ws.close(); ws = None
        except (OSError, StopIteration, websocket.WebSocketException) as error:
            (run/'cdp-last-error.txt').write_text(str(error), encoding='utf-8')
        time.sleep(.2)
    assert ws, 'WebView2 CDP did not start'
    until('!!document.querySelector(".pole")')
    hwnd = user32.FindWindowW(None, 'NexusBro')
    assert hwnd
    screen_width, screen_height = ImageGrab.grab().size
    width, height = min(1280, screen_width-40), min(820, screen_height-60)
    assert width >= 900 and height >= 560, 'Windows desktop is too small for the application'
    assert user32.SetWindowPos(hwnd, None, 20, 20, width, height, 0x0040)
    user32.SetForegroundWindow(hwnd)
    command('nastrojki_sohranit', nastrojki={
        'tema': 'temnaya', 'poisk': 'google', 'novaya_stranica': 'about:blank',
        'panel_shirina': 180, 'panel_svernuta': False, 'avtoobnovleniya': False, 'apparatnoe_uskorenie': False})
    fixture = 'data:text/html,'+urllib.parse.quote('<!doctype html><title>RenderFixture</title><style>html,body{margin:0;background:#185541;width:100%;height:100%}</style>')
    command('otkryt', nastrojki={'id': 'win-render-proba', 'nazvanie': 'Render fixture', 'proxy': '',
        'adres': fixture, 'pustaya': True, 'rezat_reklamu': False})
    until('document.body.innerText.includes("RenderFixture")')
    page_pixel('initial')
    (run/'before-dialog.txt').write_text(command('diagnostika'), encoding='utf-8')
    # Capture through the actual diagnostics button, before it hides the engine.
    js('document.querySelector("button[title=Диагностика]").click();true')
    until('!!document.querySelector(".diagnostika-okno")')
    text = js('document.querySelector(".diagnostika-okno pre").textContent')
    (run/'dialog.txt').write_text(text, encoding='utf-8')
    assert 'скрыты для диалога: false' in text, text
    js('Array.from(document.querySelectorAll(".diagnostika-okno button")).find(b=>b.textContent.trim()==="Закрыть").click();true')
    until('!document.querySelector(".diagnostika-okno")')
    page_pixel('after-dialog')
    assert user32.SetWindowPos(hwnd, None, 20, 20, max(900, width-120), max(560, height-100), 0x0040)
    time.sleep(.3)
    page_pixel('after-resize')
    js('document.querySelector(".svernut-panel").click();true')
    time.sleep(.3)
    page_pixel('after-sidebar-collapse')
    results.append({'success': True})
finally:
    try:
        status = subprocess.run(['powershell', '-NoProfile', '-Command', 'Get-CimInstance Win32_Process | Where-Object { $_.Name -match "nexusbro|msedgewebview2" } | Select-Object ProcessId,ParentProcessId,Name,CommandLine | Format-List; netstat -ano | Select-String "'+str(args.port)+'"'], capture_output=True, text=True, timeout=15)
        (run/'processes.txt').write_text(status.stdout+status.stderr, encoding='utf-8')
    except Exception as error: results.append({'process_info_error': str(error)})
    try: ImageGrab.grab().save(run/'final-screen.png')
    except Exception as error: results.append({'capture_error': str(error)})
    if ws:
        try: (run/'final-diagnostics.txt').write_text(command('diagnostika'), encoding='utf-8')
        except Exception as error: results.append({'diagnostics_error': str(error)})
        try: js('window.__TAURI_INTERNALS__.invoke("plugin:process|exit",{code:0})')
        except Exception: pass
        ws.close()
    if app:
        try: app.wait(timeout=10)
        except subprocess.TimeoutExpired: app.kill(); app.wait()
    (run/'results.json').write_text(json.dumps(results, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
    print(json.dumps(results))
