"""Windows screen acceptance using physical mouse/keyboard, without CDP."""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
import pathlib
import re
import subprocess
import time
import urllib.parse
import pyautogui as gui
from PIL import ImageGrab

p = argparse.ArgumentParser()
p.add_argument('--exe', required=True)
p.add_argument('--cef', required=True)
p.add_argument('--out', required=True)
p.add_argument('--baseline', action='store_true')
a = p.parse_args()
assert os.name == 'nt'
u = ctypes.windll.user32
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
u.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
u.FindWindowW.restype = wintypes.HWND
u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
u.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
u.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint]
u.SetForegroundWindow.argtypes = [wintypes.HWND]
run = pathlib.Path(a.out).resolve()
assert not run.exists(), 'Use a fresh output directory'
run.mkdir(parents=True)
cef = pathlib.Path(a.cef).resolve()
env = {**os.environ, 'CEF_PATH': str(cef), 'PATH': str(cef)+os.pathsep+os.environ['PATH'],
       'NEXUSBRO_TEST_DATA': str(run/'data'), 'WEBVIEW2_USER_DATA_FOLDER': str(run/'webview2'), 'NEXUSBRO_PROBA': '1'}
results = []
data = run/'data'
if a.baseline:
    assert os.environ.get('GITHUB_ACTIONS') == 'true', 'Baseline uses an ephemeral Windows runner only'
    data = pathlib.Path(os.environ['LOCALAPPDATA'])/'NexusBro'
    assert not data.exists(), 'Baseline requires a fresh Windows account'
app = None
container = None

def rect(h):
    r = wintypes.RECT()
    assert u.GetWindowRect(h, ctypes.byref(r))
    return r

def origin():
    pt = wintypes.POINT()
    assert u.ClientToScreen(hwnd, ctypes.byref(pt))
    return pt

def pixels(stage):
    deadline = time.monotonic()+25
    while time.monotonic() < deadline:
        r = rect(container)
        screen = ImageGrab.grab()
        points = [(x, (r.top+r.bottom)//2) for x in [r.left+20, (r.left+r.right)//2, r.right-20]]
        rgb = [screen.getpixel(point)[:3] for point in points]
        if all(c == (24,85,65) for c in rgb):
            screen.save(run/f'{stage}.png')
            results.append({'stage': stage, 'rgb': rgb})
            return
        time.sleep(.1)
    screen.save(run/f'{stage}-failure.png')
    results.append({'stage': stage, 'rgb': rgb, 'success': False})
    raise AssertionError(f'{stage}: pixels={rgb}, expected (24,85,65)')

try:
    app = subprocess.Popen([str(pathlib.Path(a.exe).resolve())], env=env)
    deadline = time.monotonic()+40
    journal = data/'zhurnal.log'
    while time.monotonic() < deadline:
        assert app.poll() is None, f'Application exited: {app.returncode}'
        hwnd = u.FindWindowW(None, 'NexusBro')
        if hwnd and journal.exists():
            matches = re.findall(r'proba: запуск ок, окно (\d+)', journal.read_text(encoding='utf-8'))
            if matches:
                container = int(matches[-1]); break
        time.sleep(.1)
    assert container, 'Native CEF container was not created'
    sw, sh = ImageGrab.grab().size
    width, height = min(1280,sw-40), min(820,sh-60)
    assert width >= 900 and height >= 560
    assert u.SetWindowPos(hwnd, None, 20,20,width,height,0x0040)
    u.SetForegroundWindow(hwnd)
    time.sleep(1)
    r = rect(container)
    # Address bar immediately above the actual CEF container.
    gui.click(r.left+180, r.top-15)
    gui.hotkey('ctrl','a')
    fixture = 'data:text/html,'+urllib.parse.quote('<!doctype html><title>RenderFixture</title><style>html,body{margin:0;background:#185541;width:100%;height:100%}</style>')
    gui.write(fixture, interval=.003)
    gui.press('enter')
    if a.baseline:
        try:
            pixels('initial')
        except AssertionError:
            session = json.loads((data/'profili/proba/obychnye.json').read_text(encoding='utf-8'))
            assert any(t['zagolovok']=='RenderFixture' and not t['zagruzka'] for t in session['vkladki']), session
            assert results[-1]['rgb'] == [(17,19,26)]*3, results
            results.append({'baseline_black_reproduced': True, 'loaded_title': 'RenderFixture'})
            raise SystemExit(0)
        raise AssertionError('Baseline renders: this run did not reproduce the original defect')
    pixels('initial')
    session = json.loads((data/'profili/proba/obychnye.json').read_text(encoding='utf-8'))
    assert any(t['zagolovok']=='RenderFixture' for t in session['vkladki']), session
    # Open a real shell dialog over the native engine and cancel with keyboard.
    pt = origin(); client = wintypes.RECT()
    assert u.GetClientRect(hwnd, ctypes.byref(client))
    gui.click(pt.x+22, pt.y+client.bottom-46)
    time.sleep(.5)
    ImageGrab.grab().save(run/'dialog.png')
    gui.click(pt.x+15,pt.y+90)
    pixels('after-dialog')
    assert u.SetWindowPos(hwnd, None,20,20,max(900,width-120),max(560,height-100),0x0040)
    time.sleep(.5)
    pixels('after-resize')
    pt = origin(); r = rect(container)
    gui.click(r.left-18, pt.y+24)
    time.sleep(.5)
    assert rect(container).left < r.left, 'Sidebar collapse did not reach native CEF'
    pixels('after-sidebar-collapse')
    # All close gestures reach the same CEF lifecycle, including the last tab.
    def session():
        return json.loads((data/'profili/proba/obychnye.json').read_text(encoding='utf-8'))
    def wait_tabs(n):
        deadline=time.monotonic()+12
        while time.monotonic()<deadline:
            assert app.poll() is None, 'Closing a tab exited NexusBro'
            current=session()
            if len(current['vkladki'])==n: return current
            time.sleep(.1)
        raise AssertionError(current)
    def new_tab():
        gui.hotkey('ctrl','t'); wait_tabs(2); time.sleep(.3)
    r=rect(container)
    new_tab()
    gui.click(r.left+95,r.top-70,button='middle')
    wait_tabs(1)
    results.append({'middle_close_keeps_application':True})
    new_tab()
    gui.click(r.left+269,r.top-70)
    wait_tabs(1)
    results.append({'cross_close_keeps_application':True})
    new_tab()
    gui.click(r.left+95,r.top-70,button='right')
    time.sleep(.4)
    gui.click(r.left+175,r.top+180)
    wait_tabs(1)
    results.append({'context_menu_close_keeps_application':True})

    # Closing the final ordinary tab creates a fresh blank tab.
    before=session()['vkladki'][0]['id']
    gui.hotkey('ctrl','w')
    deadline=time.monotonic()+12
    while time.monotonic()<deadline:
        assert app.poll() is None
        current=session()
        if len(current['vkladki'])==1 and current['vkladki'][0]['id']!=before: break
        time.sleep(.1)
    assert current['vkladki'][0]['id']!=before and current['vkladki'][0]['adres']=='about:blank', current
    results.append({'last_close_recreates_blank_keeps_application':True})
    # Native focus: Ctrl+L, Ctrl+U, then F12 must target the page.
    gui.hotkey('ctrl','l');time.sleep(.3);gui.write(fixture,interval=.001);gui.press('enter');pixels('before-source')
    r=rect(container);gui.click(r.left+80,r.top+100);gui.hotkey('ctrl','u');wait_tabs(2)
    deadline=time.monotonic()+12
    while time.monotonic()<deadline:
        source=session()
        if any(t['adres'].startswith('view-source:') and not t['zagruzka'] for t in source['vkladki']):break
        time.sleep(.1)
    assert any(t['adres'].startswith('view-source:') for t in source['vkladki']),source
    ImageGrab.grab().save(run/'source-tab.png');gui.hotkey('ctrl','w');wait_tabs(1)
    results.append({'source_opens_in_browser_tab':True})
    gui.click(r.left+80,r.top+100);gui.press('f12')
    u.GetWindowTextW.argtypes=[wintypes.HWND,wintypes.LPWSTR,ctypes.c_int]
    u.IsWindowVisible.argtypes=[wintypes.HWND]
    def devtools_windows():
        found=[]
        @ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
        def enum(h,_):
            text=ctypes.create_unicode_buffer(512);u.GetWindowTextW(h,text,512)
            if u.IsWindowVisible(h) and any(s in text.value.lower() for s in ['devtools','developer','инструмент']):found.append(int(h))
            return True
        u.EnumWindows(enum,0);return found
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        tools=devtools_windows()
        if tools:break
        assert app.poll() is None;time.sleep(.1)
    assert tools,'F12 did not open a visible DevTools window'
    ImageGrab.grab().save(run/'devtools.png')
    u.SetForegroundWindow(tools[0]);gui.hotkey('alt','f4');time.sleep(.5);u.SetForegroundWindow(hwnd)
    results.append({'native_f12_visible_devtools':True})
    # Physical HTML5 reorder in WebView2 (native drag handler must be disabled).
    gui.hotkey('ctrl','t');wait_tabs(2);gui.hotkey('ctrl','t');current=wait_tabs(3);time.sleep(.3)
    order=[t['id'] for t in current['vkladki']]
    gui.moveTo(r.left+619,r.top-70);gui.mouseDown();gui.moveTo(r.left+159,r.top-70,duration=.7);gui.mouseUp()
    deadline=time.monotonic()+12
    while time.monotonic()<deadline:
        current=session()
        if [t['id'] for t in current['vkladki']]==[order[2],order[0],order[1]]:break
        time.sleep(.1)
    assert [t['id'] for t in current['vkladki']]==[order[2],order[0],order[1]],current
    results.append({'physical_ordinary_drag_reorders':True})

    results.append({'success': True})
finally:
    ImageGrab.grab().save(run/'final-screen.png')
    if app:
        app.terminate()
        try: app.wait(timeout=10)
        except subprocess.TimeoutExpired: app.kill(); app.wait()
    if a.baseline and data.exists():
        import shutil
        # Preserve only diagnostics, never locked Chromium cookie/cache databases.
        files = [data/'zhurnal.log', *data.glob('profili/*/cef*.log'), *data.glob('profili/*/obychnye.json')]
        for source in files:
            if not source.is_file(): continue
            target = run/'data'/source.relative_to(data)
            target.parent.mkdir(parents=True, exist_ok=True)
            try: shutil.copyfile(source, target)
            except OSError as error: results.append({'diagnostic_copy_error': str(error)})
    (run/'results.json').write_text(json.dumps(results,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(results))
