"""Windows screen acceptance using physical mouse/keyboard, without CDP."""
from origin_request import canonicalize
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
import http.server
import threading
import socket
import psutil
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
u.GetWindow.argtypes = [wintypes.HWND,wintypes.UINT]
u.GetWindow.restype = wintypes.HWND
run = pathlib.Path(a.out).resolve()
assert not run.exists(), 'Use a fresh output directory'
run.mkdir(parents=True)
cef = pathlib.Path(a.cef).resolve()
env = {**os.environ, 'CEF_PATH': str(cef), 'PATH': str(cef)+os.pathsep+os.environ['PATH'],
       'NEXUSBRO_TEST_DATA': str(run/'data'), 'WEBVIEW2_USER_DATA_FOLDER': str(run/'webview2'), 'NEXUSBRO_PROBA': '1','NEXUSBRO_TEST_DOWNLOAD_DIR':str(run/'downloads')}
# Искусственные профили: приватные реальные браузерные данные runner не читаются.
import sqlite3
import_root=run/'import-profiles';env['NEXUSBRO_TEST_IMPORT_ROOT']=str(import_root)
chrome_file=import_root/'Chrome/User Data/Default/Bookmarks';chrome_file.parent.mkdir(parents=True)
chrome_file.write_text(json.dumps({'roots':{'bookmark_bar':{'name':'Bookmarks bar','type':'folder','children':[{'type':'url','name':'Imported Chrome','url':'https://import-fixture.test/chrome'},{'type':'folder','name':'Nested','children':[{'type':'url','name':'Вложенная','url':'https://import-fixture.test/nested'}]}]}}}),encoding='utf-8')
chrome_original=chrome_file.read_bytes()
firefox_file=import_root/'Firefox/Profiles/fixture.default/places.sqlite';firefox_file.parent.mkdir(parents=True)
firefox_source=sqlite3.connect(firefox_file);firefox_source.execute('PRAGMA journal_mode=WAL');firefox_source.executescript("CREATE TABLE moz_places(id INTEGER PRIMARY KEY,url TEXT);CREATE TABLE moz_bookmarks(id INTEGER PRIMARY KEY,parent INTEGER,title TEXT,type INTEGER,fk INTEGER,guid TEXT,position INTEGER);INSERT INTO moz_places VALUES(1,'https://import-fixture.test/firefox');INSERT INTO moz_bookmarks VALUES(1,0,'Toolbar',2,NULL,'toolbar_____',0);INSERT INTO moz_bookmarks VALUES(2,1,'Imported Firefox',1,1,'fixture_____',0);");firefox_source.commit()
results = []
hits = {}
subscription = b''
ad_reports=[]
translation_reports=[]
storage_reports=[]
class Fixture(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.path = canonicalize(self.path)
        path=urllib.parse.urlsplit(self.path).path
        hits[path] = hits.get(path,0)+1
        if path=='/download-fixture':
            body=b'NexusBro download fixture\n'*256
            self.send_response(200);self.send_header('Content-Type','application/octet-stream');self.send_header('Content-Disposition','attachment; filename="nexusbro-fixture.txt"');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body);return
        if path=='/storage-result':storage_reports.append(urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query,keep_blank_values=True))
        if path=='/translate':
            q=urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query).get("q",[""])[0]
            body=json.dumps([[[q.replace("Regular news","Переведённые новости"),q,None,None]]],ensure_ascii=False).encode()
            self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body);return
        if path=='/translation-result':translation_reports.append(urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query))
        if path=='/sub':
            self.send_response(200);self.send_header('Content-Length',str(len(subscription)));self.end_headers();self.wfile.write(subscription);return
        if path=='/ad-result':
            ad_reports.append(urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query))
        if path=='/storage':
            body=b'''<!doctype html><title>StorageFixture</title><style>html,body{margin:0;background:#185541;width:100%;height:100%}</style><p>Findneedle Findneedle</p><script>const q=new URLSearchParams(location.search);let seen=localStorage.getItem('marker')||'';let cookie=document.cookie;if(q.has('set')){localStorage.setItem('marker',q.get('set'));document.cookie='marker='+q.get('set')+';path=/;SameSite=Lax'}fetch('/storage-result?'+new URLSearchParams({case:q.get('case'),seen,cookie}));</script>'''
        elif path.startswith('/ad-fixture'):
            body=b'''<!doctype html><title>AdsFixture</title><style>html,body{margin:0;background:#185541;width:100%;height:100%}.browser-promo,.advert{position:fixed;top:0;width:100%;height:60px;background:red}</style><main id="news">Regular news</main><a id="sponsored" style="display:block"><span style="display:block">Lexus RX 2023, auto.ru sponsored product</span><span style="display:block">&#1056;&#1045;&#1050;&#1051;&#1040;&#1052;&#1040;</span></a><div id="browser-ad" class="browser-promo">Download: &#1057;&#1082;&#1072;&#1095;&#1072;&#1081; Chrome</div><aside id="yandex-ad" class="advert">Yandex ad</aside><img src="/ads/banner.png"><script>
new MutationObserver(()=>fetch('/translation-result?text='+encodeURIComponent(document.getElementById('news').textContent)+'&path='+encodeURIComponent(location.pathname))).observe(document.getElementById('news'),{subtree:true,characterData:true,childList:true});
setTimeout(async()=>{let blocked=false;try{await fetch('/ads/fetch-promo')}catch(e){blocked=true}await fetch('/control?utm_source=news');const p=new URLSearchParams({path:location.pathname,sponsored:String(!!document.getElementById('sponsored')),browser:String(!!document.getElementById('browser-ad')),ad:String(!!document.getElementById('yandex-ad')),news:String(!!document.getElementById('news')),blocked:String(blocked)});await fetch('/ad-result?'+p);document.title='AdsFixtureVerified'},1500)
</script>'''
        else:
            body=b'<!doctype html><title>RenderFixture</title><style>html,body{margin:0;background:#185541;width:100%;height:100%}a{position:absolute;left:40px;color:white;font:16px sans-serif}</style><a style="top:40px" href="/middle-target">Middle link fixture</a><a style="top:90px" href="/context-target">Context link fixture</a>'
            if path in ['/middle-target','/context-target']:body=body.replace(b'RenderFixture',b'LinkTargetFixture')
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    def log_message(self,*_):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Fixture)
threading.Thread(target=server.serve_forever,daemon=True).start()
base=f'http://127.0.0.1:{server.server_port}'

data = run/'data'
if a.baseline:
    assert os.environ.get('GITHUB_ACTIONS') == 'true', 'Baseline uses an ephemeral Windows runner only'
    data = pathlib.Path(os.environ['LOCALAPPDATA'])/'NexusBro'
    assert not data.exists(), 'Baseline requires a fresh Windows account'
vpn_server=None
app = None
container = None

def recovery_focus_role(focus_pid, app_pid, family_pids, process_name, cmdline):
    """Чистая классификация роли клавиатурного фокуса для квитанции.
    Наружу — только роль из {webkit,cef,shell,none,foreign}; никаких PID,
    заголовков, путей или аргументов процессов."""
    if not focus_pid:
        return 'none'
    if focus_pid == app_pid:
        return 'shell'
    if focus_pid not in family_pids:
        return 'foreign'
    if 'webview' in (process_name or '').lower():
        return 'webkit'
    if any(str(arg).startswith('--type=') or arg == '--nb-dvizhok' for arg in (cmdline or [])):
        return 'cef'
    return 'shell'


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
    # The native CEF container is not a shell-layout anchor: a deferred tab
    # may leave its rectangle behind the lazy-page overlay. Locate the real
    # address control through read-only UIA, then use ordinary physical input.
    if a.baseline:
        gui.click(r.left+180, r.top-15)
    else:
        from pywinauto import Desktop
        from comtypes import COMError
        initial_window=Desktop(backend='uia').window(handle=hwnd)
        initial_address=None;deadline=time.monotonic()+15
        while time.monotonic()<deadline:
            try:
                candidates=[e for e in initial_window.descendants() if e.element_info.name=='Адрес страницы'
                            and e.element_info.control_type in ('Edit','ComboBox') and e.is_visible() and e.is_enabled()]
            except (COMError,KeyError):
                candidates=[]
            if len(candidates)==1:initial_address=candidates[0];break
            time.sleep(.1)
        assert initial_address is not None,'Initial native address control unavailable or ambiguous'
        initial_address.click_input()
        # Тест-предусловие: РЕАЛЬНЫЙ клавиатурный фокус именно в этом поле
        # (тот же UIA-API, что :284/:636). Без фокуса ввод не отправляется.
        u.GetForegroundWindow.argtypes = []
        u.GetForegroundWindow.restype = wintypes.HWND
        u.GetAsyncKeyState.argtypes = [ctypes.c_int]
        u.GetAsyncKeyState.restype = wintypes.SHORT
        u.GetWindowThreadProcessId.argtypes = [wintypes.HWND,ctypes.POINTER(wintypes.DWORD)]
        u.GetWindowThreadProcessId.restype = wintypes.DWORD
        focus_deadline=time.monotonic()+3;focus_ready=False
        while time.monotonic()<focus_deadline:
            try:
                if initial_address.element_info.element.CurrentHasKeyboardFocus:
                    focus_ready=True;break
            except Exception:pass
            time.sleep(.05)
        entry={'focus_ready':focus_ready,'interval':0.02}
        try:
            app_pid=wintypes.DWORD()
            u.GetWindowThreadProcessId(hwnd,ctypes.byref(app_pid))
            family={app_pid.value}
            try:
                parent=psutil.Process(app_pid.value)
                family|={child.pid for child in parent.children(recursive=True)}
            except (psutil.NoSuchProcess,psutil.AccessDenied):
                pass
            fg=u.GetForegroundWindow();fg_pid=wintypes.DWORD()
            if fg:u.GetWindowThreadProcessId(fg,ctypes.byref(fg_pid))
            entry['owned_foreground']=bool(fg) and fg_pid.value in family
            entry['ctrl_released']=(u.GetAsyncKeyState(0x11)&0x8000)==0
            entry['shift_released']=(u.GetAsyncKeyState(0x10)&0x8000)==0
        except Exception:
            entry['focus_facts']='unavailable'
        if not focus_ready:
            results.append({'initial_address_entry':entry})
            raise AssertionError('Initial address control did not take keyboard focus')
        if (entry.get('owned_foreground') is not True or entry.get('ctrl_released') is not True
                or entry.get('shift_released') is not True):
            results.append({'initial_address_entry':entry})
            raise AssertionError('Initial physical input ownership or released modifiers unproven')
    gui.hotkey('ctrl','a')
    fixture = base+'/page'
    # ОДИН humanlike ввод (20мс/символ вместо нереалистичных 333 симв/с);
    # критерий exact не ослаблен, повторной печати нет.
    gui.write(fixture, interval=.003 if a.baseline else .02)
    if not a.baseline:
        deadline=time.monotonic()+3
        while time.monotonic()<deadline and initial_address.iface_value.CurrentValue!=fixture:time.sleep(.05)
        actual_value=initial_address.iface_value.CurrentValue
        try:
            from windows_input_receipt import classify_value
            entry['value_check']=classify_value(actual_value,fixture)
            entry['owned_input_focus']=True
            if actual_value!=fixture:
                try:
                    fg=u.GetForegroundWindow();fg_pid=wintypes.DWORD()
                    if fg:u.GetWindowThreadProcessId(fg,ctypes.byref(fg_pid))
                    entry['focus_after_failed_input']={
                        'field_has_focus':bool(initial_address.element_info.element.CurrentHasKeyboardFocus),
                        'owned_foreground':bool(fg) and fg_pid.value in family,
                        'ctrl_released':(u.GetAsyncKeyState(0x11)&0x8000)==0,
                        'shift_released':(u.GetAsyncKeyState(0x10)&0x8000)==0}
                except Exception:entry['focus_after_failed_input']={'available':False}
                time.sleep(1)
                try:
                    entry['late_exact']=initial_address.iface_value.CurrentValue==fixture
                except Exception:
                    entry['late_exact']=False
        except Exception:
            entry['value_check']='unavailable'
        results.append({'initial_address_entry':entry})
        assert actual_value==fixture,'Initial physical address entry failed'
        results.append({'initial_address_native_uia':True,'physical_entry_verified':True})
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
    # UI Automation locates controls; actual input is mouse clicks in WebView2.
    from pywinauto import Desktop
    from comtypes import COMError
    win=Desktop(backend="uia").window(handle=hwnd)
    def uia_descendants(control):
        # WebView2 can expose a node with no control type while removing a
        # dialog. pywinauto 0.6.9 raises KeyError(None) for this transient node.
        # Never substitute an empty tree: every required control still must exist.
        for attempt in range(5):
            try:return control.descendants()
            except (COMError,KeyError) as error:
                if isinstance(error,KeyError) and error.args!=(None,):raise
                if attempt==4:raise
                time.sleep(.15)

    def ui(name, kinds=None, timeout=30):
        # ⛔ Новые управления WebView2 регистрируются в UIA не мгновенно, а
        # после нашей же перерисовки дерево может ещё показывать прошлое
        # состояние. Поэтому ищем дольше и с паузой, а в ошибке печатаем, что
        # реально видно — по этому видно, на каком шаге остановились.
        deadline=time.monotonic()+timeout
        videno=set();scrolled=0
        while time.monotonic()<deadline:
            candidates=uia_descendants(win)
            for native_popup in Desktop(backend='uia').windows(title='NexusBro — меню'):
                candidates.extend(uia_descendants(native_popup))
            allowed=kinds+(['MenuItem'] if kinds and any(k in kinds for k in ['Button','ListItem']) else []) if kinds else None
            items=[x for x in candidates if x.element_info.name==name and (not allowed or x.element_info.control_type in allowed)]
            for x in items:
                # WebView2 reports clipped form controls visible. Scroll BEFORE
                # the coordinate click; otherwise its center can hit the backdrop.
                try:x.iface_scroll_item.ScrollIntoView()
                except Exception:pass
                try:
                    if x.is_visible() and x.is_enabled():
                        first=x.rectangle();time.sleep(.1)
                        if first==x.rectangle():return x
                except COMError:
                    continue
            if name in ['Отмена','Создать','Сохранить'] and scrolled<2 and time.monotonic()>deadline-28:
                r=rect(hwnd);gui.moveTo((r.left+r.right)//2,r.bottom-150);gui.scroll(-4);scrolled+=1
            videno={x.element_info.name for x in uia_descendants(win) if x.element_info.name}
            time.sleep(.25)
        raise AssertionError('UIA control missing: '+name+' | видно: '+', '.join(sorted(videno)[:40]))
    def super_button(name, timeout=15):
        # The owner window's actual accessible button, not a sidebar row number:
        # cloud/local headings and collapsed labels can change its position.
        deadline=time.monotonic()+timeout
        visible=[]
        while time.monotonic()<deadline:
            matches=[];visible=[]
            for control in uia_descendants(win):
                if control.element_info.control_type!='Button':continue
                label=control.element_info.name
                try:
                    if not control.is_visible() or not control.is_enabled():continue
                    visible.append(label)
                    if label not in (name,name+' — приватная супер-вкладка'):continue
                    try:control.iface_scroll_item.ScrollIntoView()
                    except Exception:pass
                    box=control.rectangle();owner=win.rectangle()
                    x=(box.left+box.right)//2;y=(box.top+box.bottom)//2
                    if box.right>box.left and box.bottom>box.top and owner.left<=x<owner.right and owner.top<=y<owner.bottom:
                        matches.append(control)
                except COMError:continue
            if len(matches)==1:return matches[0]
            assert len(matches)<2,('Ambiguous Super button in owner window',name,len(matches))
            time.sleep(.1)
        raise AssertionError(('Super button missing in owner window',name,visible))
    def focus_ui(name, kinds):
        from comtypes import COMError
        for attempt in range(3):
            try:
                control=ui(name,kinds,timeout=2)
                control.click_input();control.set_focus()
                deadline=time.monotonic()+1
                while time.monotonic()<deadline:
                    control=ui(name,kinds,timeout=1)
                    if control.element_info.element.CurrentHasKeyboardFocus:return control
                    time.sleep(.05)
            except (COMError,AssertionError):
                time.sleep(.1)
        raise AssertionError('Native control did not receive keyboard focus: '+name)

    def themed_popup(stage):
        # UIA may merge an owned WebView popup into the owner's accessible
        # tree; enumerate actual HWND ownership rather than top-level UIA roots.
        menus=[]
        @ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
        def collect_menu(h,_):
            pid=wintypes.DWORD();u.GetWindowThreadProcessId(h,ctypes.byref(pid))
            if pid.value==app.pid and u.IsWindowVisible(h) and int(h)!=int(hwnd) and u.GetWindow(h,4)==hwnd:menus.append(int(h))
            return True
        u.EnumWindows(collect_menu,0)
        assert len(menus)==1,('Expected one actual owned popup HWND',menus)
        # GetWindowRect includes an invisible DWM resize/shadow frame even for
        # this undecorated popup. Its left+3 can sample the owner's Super text
        # behind the popup instead of HTML (observed in collapsed-sidebar CI).
        # Locate the actual native client surface, keeping all three samples
        # in its background gutter and away from the rounded top/bottom edge.
        client=wintypes.RECT();assert u.GetClientRect(menus[0],ctypes.byref(client))
        origin=wintypes.POINT(client.left,client.top)
        assert u.ClientToScreen(menus[0],ctypes.byref(origin))
        cw=client.right-client.left;ch=client.bottom-client.top
        assert cw>20 and ch>20,('Owned popup client is too small',cw,ch)
        points=[(origin.x+3,y) for y in [origin.y+8,origin.y+ch//2,origin.y+ch-9]]
        screen=ImageGrab.grab()
        colors=[screen.getpixel(point)[:3] for point in points]
        assert all(max(rgb)<180 for rgb in colors),('Menu did not inherit dark NexusBro palette',colors)
        assert not Desktop(backend='uia').windows(class_name='#32768'),'Unexpected system menu instead of themed popup'
        screen.save(run/(stage+'-themed.png'))
        results.append({'themed_menu':stage,'background_rgb':colors,'background_points':points,
                        'client_rect':[origin.x,origin.y,cw,ch],'owned_html_popup':True})
        return menus[0]
    ui('Варианты новой вкладки',['Button']).click_input()
    ui('Новая приватная вкладка',['MenuItem'])
    themed_popup('new-tab')
    pixels('new-tab-menu-keeps-document')
    gui.press('escape')
    super_button('Проба').click_input(button='right')
    ui('Настройки',['MenuItem'])
    themed_popup('super-context')
    pixels('super-menu-keeps-document')
    gui.press('escape')
    results.append({'native_new_tab_and_super_menus_keep_cef_visible':True})
    def link_session():
        # The app atomically replaces this profile file while we poll it; a
        # transient Windows sharing violation (PermissionError, errno 13 /
        # winerror 32) is retried inside the existing bounded loops. Every
        # other error (missing file, malformed JSON, permissions policy)
        # stays fatal on purpose.
        try:
            return json.loads((data/'profili/proba/obychnye.json').read_text(encoding='utf-8'))
        except PermissionError:
            return None
    deadline=time.monotonic()+15
    source_session=None
    while time.monotonic()<deadline:
        source_session=link_session()
        if source_session is not None:break
        time.sleep(.1)
    assert source_session is not None,'Profile session file stayed transiently locked past the bounded deadline'
    source_tab=source_session['aktivnaya']
    r=rect(container);gui.click(r.left+100,r.top+50,button='middle')
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        linked=link_session()
        if linked is not None and any(t['adres'].endswith('/middle-target') and not t['zagruzka'] for t in linked['vkladki']):break
        time.sleep(.1)
    assert linked is not None,'Profile session file stayed transiently locked past the bounded deadline'
    assert any(t['adres'].endswith('/middle-target') for t in linked['vkladki']),'Middle click did not create link tab'
    assert linked['aktivnaya']==source_tab,'Middle click unexpectedly replaced or activated the source tab'
    ui('LinkTargetFixture',['TabItem']).click_input(button='middle');time.sleep(.3)
    r=rect(container);gui.click(r.left+100,r.top+100,button='right')
    ui('Открыть ссылку в новой вкладке',['MenuItem'])
    themed_popup('page-link-context')
    ui('Открыть ссылку в новой вкладке',['MenuItem']).click_input()
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        linked=link_session()
        if linked is not None and any(t['adres'].endswith('/context-target') and not t['zagruzka'] for t in linked['vkladki']):break
        time.sleep(.1)
    assert linked is not None,'Profile session file stayed transiently locked past the bounded deadline'
    assert any(t['adres'].endswith('/context-target') for t in linked['vkladki']),'Context menu did not create link tab'
    assert linked['aktivnaya']==source_tab,'Context open replaced or unexpectedly activated the source tab'
    ui('LinkTargetFixture',['TabItem']).click_input(button='middle');time.sleep(.3)
    results.append({'native_middle_link_and_themed_context_open_background_tabs':True})

    # Open a real shell dialog over the native engine and cancel with keyboard.
    pt = origin(); client = wintypes.RECT()
    assert u.GetClientRect(hwnd, ctypes.byref(client))
    ui('Новая супер-вкладка',['Button']).click_input()
    time.sleep(.5)
    ImageGrab.grab().save(run/'dialog.png')
    ui('Отмена',['Button']).click_input()
    pixels('after-dialog')
    assert u.SetWindowPos(hwnd, None,20,20,max(900,width-120),max(560,height-100),0x0040)
    time.sleep(.5)
    pixels('after-resize')
    r = rect(container)
    ui('Свернуть левую панель',['Button']).click_input()
    time.sleep(.5)
    assert rect(container).left < r.left, 'Sidebar collapse did not reach native CEF'
    pixels('after-sidebar-collapse')
    # All close gestures reach the same CEF lifecycle, including the last tab.
    def session():
        # Windows может кратко блокировать файл во время записи CEF.
        # Повторяем только чтение снимка; ошибки проверок не скрываем.
        deadline = time.monotonic() + 2
        while True:
            try:
                return json.loads((data/'profili/proba/obychnye.json').read_text(encoding='utf-8'))
            except (PermissionError, FileNotFoundError, json.JSONDecodeError):
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.05)
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
    ui('Закрыть вкладку',['MenuItem','Button']).click_input()
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
        u.EnumChildWindows(wintypes.HWND(container),enum,0);return found
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        tools=devtools_windows()
        if tools:break
        assert app.poll() is None;time.sleep(.1)
    assert tools,'F12 did not create docked DevTools inside the page container'
    u.GetParent.argtypes=[wintypes.HWND];u.GetParent.restype=wintypes.HWND
    assert u.GetParent(tools[0])==container,(tools,container)
    assert rect(tools[0]).right-rect(tools[0]).left < rect(container).right-rect(container).left
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        r_tools=rect(tools[0]);panel=ImageGrab.grab().crop((r_tools.left+8,r_tools.top+8,r_tools.right-8,r_tools.bottom-8)).convert('RGB')
        colors=panel.getcolors(panel.width*panel.height)
        if colors and len(colors)>80:break
        time.sleep(.15)
    assert colors and len(colors)>80,'DevTools remained a blank panel'
    # Раздвигаем настоящую границу между HWND, а не меняем React range.
    before=rect(tools[0]);border=before.left-3;y=(before.top+before.bottom)//2
    gutter_rgb=ImageGrab.grab().getpixel((border,y))[:3]
    assert max(gutter_rgb)<180,('DevTools gutter flashes white in dark theme',gutter_rgb)
    results.append({'native_devtools_dark_gutter':True,'rgb':gutter_rgb})
    gui.moveTo(border,y);gui.mouseDown();gui.moveTo(border-110,y,duration=.6);gui.mouseUp();time.sleep(.4)
    after=rect(tools[0]);splitter_drag_ok=after.left<before.left-60
    results.append({'native_devtools_boundary_drag':splitter_drag_ok,'before':before.left,'after':after.left})
    ImageGrab.grab().save(run/'devtools.png')
    gui.click(rect(container).left+80,rect(container).top+100);gui.press('f12');time.sleep(.5);assert app.poll() is None;assert not devtools_windows();u.SetForegroundWindow(hwnd)
    results.append({'native_f12_docked_devtools':True,'devtools_close_keeps_browser':True})
    r=rect(container);gui.click(r.left+80,r.top+100);gui.hotkey('ctrl','shift','delete')
    ui('Период очистки истории',['ComboBox']).click_input()
    ui('За всё время',['ListItem','Button']).click_input()
    ui('Период очистки истории',['ComboBox']).click_input()
    assert ui('За всё время',['ListItem','Button']).is_selected(),'Clear period option was not selected'
    gui.press('escape')
    results.append({'native_clear_period_dropdown_mouse_selection':True})
    ui('Отмена',['Button']).click_input()
    results.append({'native_ctrl_shift_delete_dialog_from_cef':True})
    # Physical HTML5 reorder in WebView2 (native drag handler must be disabled).
    gui.hotkey('ctrl','t');focus_after_tools=True
    try:wait_tabs(2)
    except AssertionError:
        # Не скрываем ошибку фокуса: итог остаётся красным, но независимые
        # проверки продолжаем, чтобы увидеть остальные проблемы за один прогон.
        focus_after_tools=False;r=rect(container);gui.click(r.left+80,r.top+100);gui.hotkey('ctrl','t');wait_tabs(2)
    results.append({'native_focus_after_devtools':focus_after_tools})
    gui.hotkey('ctrl','t');current=wait_tabs(3);time.sleep(.3)
    order=[t['id'] for t in current['vkladki']]
    tab_controls=sorted([x for x in uia_descendants(win) if x.element_info.control_type=='TabItem' and x.is_visible() and x.rectangle().top<rect(container).top],key=lambda x:x.rectangle().left)
    assert len(tab_controls)==3,[(x.element_info.name,str(x.rectangle())) for x in tab_controls]
    source_rect=tab_controls[2].rectangle();target_rect=tab_controls[0].rectangle()
    gui.moveTo((source_rect.left+source_rect.right)//2,(source_rect.top+source_rect.bottom)//2);gui.mouseDown()
    gui.moveTo((target_rect.left+target_rect.right)//2,(target_rect.top+target_rect.bottom)//2,duration=.7);gui.mouseUp()
    deadline=time.monotonic()+12
    while time.monotonic()<deadline:
        current=session()
        if [t['id'] for t in current['vkladki']]==[order[2],order[0],order[1]]:break
        time.sleep(.1)
    assert [t['id'] for t in current['vkladki']]==[order[2],order[0],order[1]],current
    results.append({'physical_ordinary_drag_reorders':True})

    deadline=time.monotonic()+10
    while time.monotonic()<deadline and any(x.element_info.name=='Настройки браузера' for x in uia_descendants(win)):time.sleep(.1)
    ui('Настройки NexusBro',['Button']).click_input()
    for dropdown_attempt in range(3):
        ui('Шрифт интерфейса',['ComboBox','Button']).click_input()
        try:
            ui('Arial',['ListItem','Button'],timeout=3).click_input()
            break
        except AssertionError:
            if dropdown_attempt==2:raise
    ui('Сохранить',['Button']).click_input()
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        cfg=json.loads((data/'nastrojki.json').read_text(encoding='utf-8'))
        if cfg['shrift']=='arial':break
        time.sleep(.1)
    assert cfg['shrift']=='arial',cfg
    deadline=time.monotonic()+10
    while time.monotonic()<deadline and any(x.element_info.name=='Настройки браузера' for x in uia_descendants(win)):time.sleep(.1)
    ui('Настройки NexusBro',['Button']).click_input()
    ui('Шрифт интерфейса',['ComboBox','Button']).click_input()
    ui('Системный',['ListItem','Button']).click_input()
    ui('Сохранить',['Button']).click_input()
    results.append({'physical_settings_dropdown_mouse_selection':True})
    # Restore all URLs without any HTTP request, then wake exactly one by click.
    app.terminate();app.wait(timeout=10);time.sleep(.5)
    records=json.loads((data/'vkladki.json').read_text(encoding='utf-8'))
    for record in records:record['pustaya']=False
    (data/'vkladki.json').write_text(json.dumps(records),encoding='utf-8')
    saved=session()
    for i,t in enumerate(saved['vkladki']):t['adres']=base+f'/sleep/{i}';t['zagolovok']=f'Sleeping {i}'
    (data/'profili/proba/obychnye.json').write_text(json.dumps(saved),encoding='utf-8')
    journal.unlink(missing_ok=True)
    app=subprocess.Popen([str(pathlib.Path(a.exe).resolve())],env={**env,'NEXUSBRO_PROBA':'0'})
    deadline=time.monotonic()+40
    while time.monotonic()<deadline:
        assert app.poll() is None
        if journal.exists():
            matches=re.findall(r'proba: запуск ок, окно (\d+)',journal.read_text(encoding='utf-8'))
            if matches:
                container=int(matches[-1]);break
        time.sleep(.1)
    time.sleep(1)
    current=session();assert all(t.get('otlozhena') for t in current['vkladki']),current
    assert not any(path.startswith('/sleep/') for path in hits),hits
    hwnd=u.FindWindowW(None,'NexusBro');assert u.SetWindowPos(hwnd,None,20,20,width,height,0x0040);u.SetForegroundWindow(hwnd);time.sleep(.5);r=rect(container)
    win=Desktop(backend="uia").window(handle=hwnd)
    ui('Загрузить вкладку',['Button'])
    ImageGrab.grab().save(run/'deferred-start.png')
    gui.click(r.left+159,r.top-70)
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        current=session()
        if hits.get('/sleep/0') and not current['vkladki'][0]['otlozhena']:break
        time.sleep(.1)
    assert hits.get('/sleep/0') and not hits.get('/sleep/1') and not hits.get('/sleep/2'),hits
    assert all(t.get('otlozhena') for t in current['vkladki'][1:]),current
    pixels('after-lazy-click')
    # Plus is immediately after the third tab, not at the far right edge.
    win=Desktop(backend="uia").window(handle=hwnd)
    ui('Новая обычная вкладка',['Button']).click_input()
    wait_tabs(4)
    results.append({'restart_zero_page_requests_until_click':True,'click_wakes_one_only':True,'plus_next_to_last_tab_physical_click':True})
    # Настоящий VLESS через sidecar приложения: без консоли, с защитой при смене узла.
    xray=pathlib.Path(a.exe).resolve().parent/'binaries/xray.exe'
    assert xray.exists(),xray
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0));vless_port=sock.getsockname()[1]
    uid='11111111-1111-4111-8111-111111111111'
    local=f'vless://{uid}@127.0.0.1:{vless_port}?encryption=none&security=none&type=tcp#Local%20working'
    dead=f'vless://{uid}@127.0.0.1:1?encryption=none&security=none&type=tcp#Unavailable'
    subscription=(dead+'\n'+local+'\n').encode()
    config={'log':{'loglevel':'debug'},'inbounds':[{'listen':'127.0.0.1','port':vless_port,'protocol':'vless','settings':{'clients':[{'id':uid}],'decryption':'none'}}],
            'outbounds':[{'tag':'deny','protocol':'blackhole'},{'tag':'fixture','protocol':'freedom','settings':{'redirect':f'127.0.0.1:{server.server_port}','finalRules':[{'action':'allow','ip':['127.0.0.1'],'port':str(server.server_port)}]}}],
            'routing':{'rules':[{'type':'field','domain':['full:vpn-fixture.test','full:yandex.ru'],'outboundTag':'fixture'},
                                # Per-context proxying intentionally includes loopback.
                                # Permit only our owned origin through this real VPN;
                                # all other destinations retain the default deny.
                                {'type':'field','ip':['127.0.0.1'],'port':str(server.server_port),
                                 'network':'tcp','outboundTag':'fixture'}]}}
    config_file=run/'local-vless.json';config_file.write_text(json.dumps(config),encoding='utf-8')
    server_log=(run/'vless-server.txt').open('w',encoding='utf-8')
    vpn_server=subprocess.Popen([str(xray),'run','-c',str(config_file)],creationflags=subprocess.CREATE_NO_WINDOW,stdout=server_log,stderr=server_log)
    time.sleep(.5);assert vpn_server.poll() is None,'Local VLESS server exited'
    def consoles():
        found=set()
        @ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
        def enum(h,_):
            text=ctypes.create_unicode_buffer(256);u.GetClassNameW(h,text,256)
            if u.IsWindowVisible(h) and text.value=='ConsoleWindowClass':found.add(int(h))
            return True
        u.EnumWindows(enum,0);return found
    old_consoles=consoles()
    # Перезапуск тестового приложения задаёт локальный HTTP endpoint для проверки VPN.
    app.terminate();app.wait(timeout=10);time.sleep(.5)
    cfg=json.loads((data/'nastrojki.json').read_text(encoding='utf-8'))
    cfg['vyhody']=[{'id':'fixture','nazvanie':'Local subscription','adres':base+'/sub','proxy_uzel':''}]
    (data/'nastrojki.json').write_text(json.dumps(cfg),encoding='utf-8')
    env['NEXUSBRO_TEST_CONNECTIVITY_URL']='http://vpn-fixture.test/probe'
    env['NEXUSBRO_TEST_TRANSLATE_URL']='http://vpn-fixture.test/translate'
    journal.unlink(missing_ok=True)
    app=subprocess.Popen([str(pathlib.Path(a.exe).resolve())],env={**env,'NEXUSBRO_PROBA':'0'})
    def new_container(previous=None):
        deadline=time.monotonic()+45
        while time.monotonic()<deadline:
            assert app.poll() is None
            if journal.exists():
                matches=re.findall(r'proba: запуск ок, окно (\d+)',journal.read_text(encoding='utf-8'))
                if matches and int(matches[-1])!=previous:return int(matches[-1])
            time.sleep(.2)
        (run/'vpn-diagnostic.json').write_text(json.dumps({'hits':hits,'server_exit':vpn_server.poll(),'uia':[x.element_info.name for x in uia_descendants(win)]},ensure_ascii=False,indent=2),encoding='utf-8')
        raise AssertionError('CEF did not restart')
    container=new_container();hwnd=u.FindWindowW(None,'NexusBro');assert u.SetWindowPos(hwnd,None,20,20,width,height,0x0040);win=Desktop(backend='uia').window(handle=hwnd);u.SetForegroundWindow(hwnd);time.sleep(.5)
    def edit_super():
        super_button('Проба').click_input(button='right')
        # Wait for the context item, then verify its actual native owner before
        # clicking. The unrelated global "Настройки NexusBro" is not a match.
        ui('Настройки',['MenuItem'],timeout=10)
        popup_hwnd=themed_popup('super-settings-context')
        # UIA may merge the popup's virtual nodes into the owner tree. Match
        # the actual menu rectangle instead of assuming a separate UIA root.
        settings=ui('Настройки',['MenuItem'],timeout=10)
        item=settings.rectangle();popup=rect(popup_hwnd)
        x=(item.left+item.right)//2;y=(item.top+item.bottom)//2
        assert popup.left<=x<popup.right and popup.top<=y<popup.bottom,('Settings is outside owned context popup',item,popup)
        settings.click_input()
        ui('Открыть сразу',['Edit'],timeout=10)
    def fill_start_url(url):
        # Focus the actual native edit control, then type real keys and read back.
        # WebView2 ScrollItem/IsOffscreen ignore the sticky footer's occlusion.
        fields=[x for x in uia_descendants(win) if x.element_info.name=='Открыть сразу' and x.element_info.control_type=='Edit' and x.is_enabled()]
        assert fields,'No start URL edit control'
        field=fields[0];field.set_focus();deadline=time.monotonic()+3
        while time.monotonic()<deadline and not field.element_info.element.CurrentHasKeyboardFocus:time.sleep(.1)
        assert field.element_info.element.CurrentHasKeyboardFocus,'Start URL did not receive keyboard focus'
        gui.hotkey('ctrl','a');gui.write(url,interval=.001)
        deadline=time.monotonic()+3;actual=''
        while time.monotonic()<deadline:
            actual=field.iface_value.CurrentValue
            if actual==url:return
            time.sleep(.1)
        raise AssertionError(('Start URL was not entered before Create',actual,url))
    def choose_subscription_node(name):
        # UIA ScrollItem is unavailable for some WebView2 buttons. Use the same
        # physical wheel action a user needs for this deliberately scrollable form.
        r=rect(hwnd);gui.moveTo((r.left+r.right)//2,(r.top+r.bottom)//2);gui.scroll(-2);time.sleep(.3)
        ui('Узел подписки',['ComboBox','Button']).click_input();ui(name,['ListItem','Button']).click_input()

    edit_super()
    ui('Сохранённое подключение',['ComboBox','Button']).click_input();ui('Local subscription',['ListItem','Button']).click_input()
    choose_subscription_node('Local working')
    gui.press('tab',presses=5,interval=.1);time.sleep(.3)
    previous=container;ui('Сохранить',['Button']).click_input();container=new_container(previous)
    assert hits.get('/probe'),hits
    assert not consoles()-old_consoles,'Xray created a visible console'
    def vpn_page(path,host='vpn-fixture.test'):
        ui('Адрес страницы',['Edit','ComboBox']).click_input();gui.hotkey('ctrl','a');time.sleep(.2);gui.write('http://'+host+'/'+path,interval=.002);gui.press('enter')
        deadline=time.monotonic()+15
        while time.monotonic()<deadline and not hits.get('/'+path):time.sleep(.1)
        assert hits.get('/'+path),hits
        pixels(path)
    vpn_page('vpn-working')
    old_records=json.loads((data/'vkladki.json').read_text(encoding='utf-8'))
    assert old_records[0]['proxy_uzel']==local,old_records
    edit_super();choose_subscription_node('Unavailable')
    gui.press('tab',presses=5,interval=.1);time.sleep(.3);ui('Сохранить',['Button']).click_input()
    deadline=time.monotonic()+20
    message='Выбранный узел не прошёл проверку HTTP(S). Прежние настройки и подключение сохранены. Выберите другой узел или повторите проверку.'
    while time.monotonic()<deadline:
        if any(x.element_info.name==message for x in uia_descendants(win)):break
        time.sleep(.2)
    else:raise AssertionError('Failed node did not produce the expected error')
    assert json.loads((data/'vkladki.json').read_text(encoding='utf-8'))==old_records
    assert not consoles()-old_consoles
    ui('Отмена',['Button']).click_input();vpn_page('vpn-after-rejected-node')
    # Убиваем только принадлежащий тестовому приложению Xray: watchdog обязан восстановить.
    children=[c for c in psutil.Process(app.pid).children(recursive=True) if c.name().lower()=='xray.exe']
    assert len(children)==1,children
    children[0].terminate();children[0].wait(timeout=10)
    previous=container;container=new_container(previous)
    vpn_page('vpn-after-recovery')
    assert not consoles()-old_consoles
    results.append({'vless_actual_http_route':True,'no_xray_console':True,'failed_node_keeps_previous_connection':True,'dead_tunnel_auto_recovers':True})
    # Реальные callbacks CEF: путь ресурса, DOM-плашки, F5 и меню перевода.
    vpn_page('ad-fixture','yandex.ru')
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and not ad_reports:time.sleep(.1)
    assert ad_reports and ad_reports[-1]['sponsored']==['false'] and ad_reports[-1]['browser']==['false'] and ad_reports[-1]['ad']==['false'] and ad_reports[-1]['news']==['true'] and ad_reports[-1]['blocked']==['true'],ad_reports
    assert not hits.get('/ads/banner.png') and not hits.get('/ads/fetch-promo'),hits
    for keys,stage in [(('f5',),'native_f5'),(('ctrl','f5'),'native_ctrl_f5')]:
        before_reports=len(ad_reports);before=hits.get('/ad-fixture',0);r=rect(container);gui.click(r.left+80,r.top+180)
        if len(keys)==1:gui.press(keys[0])
        else:gui.hotkey(*keys)
        deadline=time.monotonic()+15
        while time.monotonic()<deadline and hits.get('/ad-fixture',0)<=before:time.sleep(.1)
        assert hits.get('/ad-fixture',0)>before,stage
        deadline=time.monotonic()+15
        while time.monotonic()<deadline and len(ad_reports)<=before_reports:time.sleep(.1)
        assert len(ad_reports)>before_reports,ad_reports
        results.append({stage:True})
    # Перевод текущего документа через реальный CEF callback и native proxy.
    old_count=len(session()['vkladki']);r=rect(container);gui.click(r.left+80,r.top+180,button='right');time.sleep(.4)
    ImageGrab.grab().save(run/'translation-menu.png');gui.press('end');gui.press('up',presses=2);gui.press('enter')
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and not any(r.get('text')==['Переведённые новости'] for r in translation_reports):time.sleep(.1)
    assert any(r.get('text')==['Переведённые новости'] for r in translation_reports),translation_reports
    translated=session();assert len(translated['vkladki'])==old_count and next(t for t in translated['vkladki'] if t['id']==translated['aktivnaya'])['adres']=='http://yandex.ru/ad-fixture',translated
    ui('Автоперевод этого сайта',['CheckBox']).click_input()
    ui('Адрес страницы',['Edit','ComboBox']).click_input();gui.hotkey('ctrl','a');gui.write('http://yandex.ru/ad-fixture-next',interval=.002);gui.press('enter')
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and not any(r.get('path')==['/ad-fixture-next'] and r.get('text')==['Переведённые новости'] for r in translation_reports):time.sleep(.1)
    assert any(r.get('path')==['/ad-fixture-next'] and r.get('text')==['Переведённые новости'] for r in translation_reports),translation_reports
    ui('Показать оригинал и выключить',['Button']).click_input()
    deadline=time.monotonic()+10
    while time.monotonic()<deadline and not any(r.get('text')==['Regular news'] for r in translation_reports):time.sleep(.1)
    assert any(r.get('text')==['Regular news'] for r in translation_reports),translation_reports
    translates_before=hits.get('/translate',0)
    ui('Адрес страницы',['Edit','ComboBox']).click_input();gui.hotkey('ctrl','a');gui.write('http://yandex.ru/ad-fixture-after-cancel',interval=.002);gui.press('enter')
    deadline=time.monotonic()+15
    while time.monotonic()<deadline and not any(r.get('path')==['/ad-fixture-after-cancel'] for r in ad_reports):time.sleep(.1)
    assert any(r.get('path')==['/ad-fixture-after-cancel'] for r in ad_reports),ad_reports
    time.sleep(1);assert hits.get('/translate',0)==translates_before
    results.append({'native_auto_translation_same_site_navigation':True,'native_original_disables_auto':True})
    results.append({'native_translation_same_document_and_original':True,'translation_uses_super_proxy':hits.get('/translate',0)>0,'cef_ad_resources_cancelled':True,'cef_browser_and_yandex_banners_removed':True,'regular_news_preserved':True})
    # Выключатель защиты должен отключать и ResourceRequestHandler, и DOM-скрипт.
    app.terminate();app.wait(timeout=10);time.sleep(.5)
    records=json.loads((data/'vkladki.json').read_text(encoding='utf-8'))
    for record in records:record['rezat_reklamu']=False
    (data/'vkladki.json').write_text(json.dumps(records),encoding='utf-8');journal.unlink(missing_ok=True)
    app=subprocess.Popen([str(pathlib.Path(a.exe).resolve())],env={**env,'NEXUSBRO_PROBA':'0'})
    container=new_container();hwnd=u.FindWindowW(None,'NexusBro');assert u.SetWindowPos(hwnd,None,20,20,width,height,0x0040);win=Desktop(backend='uia').window(handle=hwnd);u.SetForegroundWindow(hwnd);time.sleep(.5)
    # При выключенной защите красная плашка ожидаема: проверяем DOM через ответ фикстуры.
    ui('Адрес страницы',['Edit','ComboBox']).click_input();gui.hotkey('ctrl','a');gui.write('http://yandex.ru/ad-fixture-off',interval=.002);gui.press('enter')
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and not any(r.get('path')==['/ad-fixture-off'] for r in ad_reports):time.sleep(.1)
    off=[r for r in ad_reports if r.get('path')==['/ad-fixture-off']]
    assert off and off[-1]['sponsored']==['true'] and off[-1]['browser']==['true'] and off[-1]['ad']==['true'] and off[-1]['blocked']==['false'],ad_reports
    assert hits.get('/ads/banner.png') and hits.get('/ads/fetch-promo'),hits
    results.append({'cef_ad_switch_off_preserves_banners_and_requests':True})
    # Find, history/bookmarks/downloads and multiwindow storage use native CEF.
    env['NEXUSBRO_TEST_DOWNLOAD_DIR']=str(run/'downloads')
    def storage(case,value=None,origin='http://vpn-fixture.test'):
        params={'case':case}
        if value is not None:params['set']=value
        url=origin+'/storage?'+urllib.parse.urlencode(params)
        # A newly opened private tab may finish mounting after the menu click.
        # Prove native keyboard focus and the exact entered URL before Enter,
        # matching the verified download-navigation pattern below. These are
        # bounded input attempts, not retries of the storage/privacy outcome.
        for input_attempt in range(3):
            address=focus_ui('Адрес страницы',['Edit','ComboBox'])
            gui.hotkey('ctrl','a');gui.write(url,interval=.001)
            deadline=time.monotonic()+3
            while time.monotonic()<deadline and address.iface_value.CurrentValue!=url:time.sleep(.05)
            if address.iface_value.CurrentValue==url:break
        assert address.iface_value.CurrentValue==url,('Storage URL was not entered',case,address.iface_value.CurrentValue)
        gui.press('enter')
        deadline=time.monotonic()+15
        while time.monotonic()<deadline:
            report=next((r for r in storage_reports if r.get('case')==[case]),None)
            if report:return report
            time.sleep(.1)
        raise AssertionError(('No storage report',case,storage_reports))
    storage('normal-main','normal-marker')
    # Подсказка должна выбираться клавишами в настоящем WebView2 над CEF.
    ui('Адрес страницы',['Edit','ComboBox']).click_input();gui.hotkey('ctrl','a');gui.write('vpn-fixture.test/storage?case=normal-m',interval=.01);time.sleep(.8)
    ImageGrab.grab().save(run/'address-suggestions.png')
    gui.press('down');gui.press('enter');gui.press('enter');time.sleep(.7)
    active=session();actual=next(t['adres'] for t in active['vkladki'] if t['id']==active['aktivnaya'])
    assert actual=='http://vpn-fixture.test/storage?case=normal-main&set=normal-marker',('Native history suggestion not selected',actual)
    results.append({'native_address_history_keyboard_selection':True})
    r=rect(container);gui.click(r.left+80,r.top+100);gui.hotkey('ctrl','f')
    ui('Найти на странице',['Edit']).click_input();gui.write('Findneedle');time.sleep(.8)
    deadline=time.monotonic()+6
    while time.monotonic()<deadline and not any(x.element_info.name=='1 из 2' for x in uia_descendants(win)):time.sleep(.1)
    find_ok=any(x.element_info.name=='1 из 2' for x in uia_descendants(win))
    gui.press('enter');time.sleep(.5)
    find_ok=find_ok and any(x.element_info.name=='2 из 2' for x in uia_descendants(win))
    results.append({'native_find_ordinals_correct':find_ok})
    gui.press('escape')
    # Настоящая русская раскладка в потоках WebView2 и CEF, не синтетический key='а'.
    u.LoadKeyboardLayoutW.argtypes=[wintypes.LPCWSTR,wintypes.UINT];u.LoadKeyboardLayoutW.restype=wintypes.HANDLE
    u.PostMessageW.argtypes=[wintypes.HWND,wintypes.UINT,wintypes.WPARAM,wintypes.LPARAM]
    def input_layout(name):
        layout=u.LoadKeyboardLayoutW(name,1);assert layout,'Keyboard layout unavailable: '+name
        targets=[hwnd]
        @ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
        def child(h,_):targets.append(h);return True
        u.EnumChildWindows(hwnd,child,0)
        for target in targets:u.PostMessageW(target,0x0050,0,layout)
        time.sleep(.3)
    input_layout('00000419');russian_shortcuts_ok=True;ru_stage='cef_find';ru_error='';before_count=len(session()['vkladki'])
    try:
        r=rect(container);gui.click(r.left+60,r.top+90);gui.hotkey('ctrl','f');ui('Найти на странице',['Edit']);gui.press('escape');time.sleep(.3)
        ru_stage='address_focus';r=rect(container);gui.click(r.left+60,r.top+90);gui.hotkey('ctrl','l');address=ui('Адрес страницы',['Edit','ComboBox']);deadline=time.monotonic()+3
        while time.monotonic()<deadline and not address.has_keyboard_focus():time.sleep(.05)
        assert address.has_keyboard_focus(),'Ctrl+L did not focus the address input in Russian layout'
        ru_stage='shell_new_tab';gui.hotkey('ctrl','t');wait_tabs(before_count+1);ru_stage='shell_close_tab';gui.hotkey('ctrl','w');wait_tabs(before_count)
    except AssertionError as error:russian_shortcuts_ok=False;ru_error=ru_stage+': '+str(error)[:200]
    finally:input_layout('00000409')
    # Продолжаем независимые сценарии, но общий итог остаётся красным при провале RU.
    while len(session()['vkladki'])>before_count:gui.hotkey('ctrl','w');time.sleep(.15)
    results.append({'native_russian_shortcuts_cef_and_shell':russian_shortcuts_ok,'error':ru_error})
    # Собственное адресное меню обращается к системному clipboard, не WebView permissions.
    address=ui('Адрес страницы',['Edit','ComboBox']);address.click_input();gui.hotkey('ctrl','a');expected_address=address.iface_value.CurrentValue;address.click_input(button='right')
    def address_action(prefix):
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            controls=list(uia_descendants(win))
            for popup in Desktop(backend='uia').windows(process=app.pid):
                if popup.handle!=hwnd:controls.extend(uia_descendants(popup))
            matches=[x for x in controls if x.element_info.control_type in ['MenuItem','Button'] and x.element_info.name.startswith(prefix) and x.is_visible() and x.is_enabled()]
            if matches:matches[0].click_input();return
            time.sleep(.1)
        raise AssertionError('Address menu item missing: '+prefix)
    address_action('Копировать')
    u.GetClipboardData.argtypes=[wintypes.UINT];u.GetClipboardData.restype=wintypes.HANDLE
    k32=ctypes.windll.kernel32;k32.GlobalLock.argtypes=[wintypes.HANDLE];k32.GlobalLock.restype=ctypes.c_void_p;k32.GlobalUnlock.argtypes=[wintypes.HANDLE]
    assert u.OpenClipboard(None)
    try:
        handle=u.GetClipboardData(13);ptr=k32.GlobalLock(handle);assert ptr;copied=ctypes.wstring_at(ptr);k32.GlobalUnlock(handle)
    finally:u.CloseClipboard()
    assert copied==expected_address,(copied,expected_address)
    address.click_input();gui.hotkey('ctrl','a');gui.press('delete');address.click_input(button='right');address_action('Вставить')
    deadline=time.monotonic()+5
    while time.monotonic()<deadline and address.iface_value.CurrentValue!=expected_address:time.sleep(.1)
    assert address.iface_value.CurrentValue==expected_address
    results.append({'native_address_menu_system_clipboard_copy_paste':True})

    gui.hotkey('ctrl','d');ui('Удалить закладку',['Button']).click_input();ui('Добавить в закладки',['Button']).click_input()
    # Живой импорт Chromium JSON и Firefox SQLite при открытом WAL writer.
    gui.hotkey('ctrl','b');ui('Перенести закладки из другого браузера…',['Button']).click_input()
    for source,url in [('Chrome Default','https://import-fixture.test/chrome'),('Firefox fixture.default','https://import-fixture.test/firefox')]:
        ui(source,['RadioButton','Button']).click_input();ui('Начать перенос закладок',['Button']).click_input()
        deadline=time.monotonic()+15
        while time.monotonic()<deadline:
            books=json.loads((data/'zakladki.json').read_text(encoding='utf-8'))
            if any(b['adres']==url for b in books):break
            time.sleep(.1)
        assert any(b['adres']==url for b in books),books
        time.sleep(.6)
    assert any(b['adres']=='https://import-fixture.test/nested' and b['papka']=='Nested' for b in books),books
    assert chrome_file.read_bytes()==chrome_original
    assert firefox_source.execute('SELECT count(*) FROM moz_bookmarks').fetchone()==(2,)
    results.append({'native_browser_import_chromium_and_live_firefox_wal':True,'native_bookmark_toggle':True})
    ui('Закрыть библиотеку',['Button']).click_input()
    gui.hotkey('ctrl','shift','b');time.sleep(.3);assert json.loads((data/'nastrojki.json').read_text(encoding='utf-8'))['panel_zakladok']
    gui.hotkey('ctrl','shift','b');time.sleep(.3);assert not json.loads((data/'nastrojki.json').read_text(encoding='utf-8'))['panel_zakladok']
    results.append({'native_bookmark_bar_toggle':True})
    gui.hotkey('ctrl','h')
    # Await the newly opened dialog's focused search, not a stale close button
    # from the preceding library. Exercise its actual Escape dismissal.
    search=focus_ui('Поиск в библиотеке',['Edit'])
    gui.press('escape')
    # UIA can retain the closing dialog. Wait for its disappearance before
    # targeting a shell control behind it; verify real keyboard focus and text.
    deadline=time.monotonic()+5
    while time.monotonic()<deadline:
        if not any(x.element_info.name=='Закрыть библиотеку' for x in uia_descendants(win)):break
        time.sleep(.1)
    assert not any(x.element_info.name=='Закрыть библиотеку' for x in uia_descendants(win)),'Library did not close before download navigation'
    address=focus_ui('Адрес страницы',['Edit','ComboBox'])
    download_url='http://vpn-fixture.test/download-fixture'
    for input_attempt in range(3):
        address=focus_ui('Адрес страницы',['Edit','ComboBox'])
        gui.hotkey('ctrl','a');gui.write(download_url,interval=.003)
        deadline=time.monotonic()+3
        while time.monotonic()<deadline and address.iface_value.CurrentValue!=download_url:time.sleep(.05)
        if address.iface_value.CurrentValue==download_url:break
    assert address.iface_value.CurrentValue==download_url,('Download URL was not entered',address.iface_value.CurrentValue)
    gui.press('enter')
    # The final path can exist while CEF is still writing/holding it on
    # Windows. Persisted completion AND a real readable payload are required.
    deadline=time.monotonic()+15
    downloads=[];download_bytes=None;download_read_error=None
    while time.monotonic()<deadline:
        try:
            downloads=json.loads((data/'zagruzki.json').read_text(encoding='utf-8'))
            if any(d['name']=='nexusbro-fixture.txt' and d['status']=='Готово' for d in downloads):
                download_bytes=(run/'downloads/nexusbro-fixture.txt').read_bytes()
                break
        except (FileNotFoundError, PermissionError) as error:
            # Only transient absence/sharing violations are retried; malformed
            # history and readable-but-wrong bytes remain immediate failures.
            download_read_error=type(error).__name__
        time.sleep(.1)
    assert download_bytes is not None,('Completed download did not become readable within 15 seconds',download_read_error)
    assert download_bytes==b'NexusBro download fixture\n'*256
    gui.hotkey('ctrl','j');ui('Закрыть библиотеку',['Button']).click_input()
    download_name_ok=any(d['name']=='nexusbro-fixture.txt' and d['status']=='Готово' for d in downloads)
    results.append({'native_download_name_correct':download_name_ok})
    assert any(d['status']=='Готово' for d in downloads),downloads
    after_download=session();assert '/storage?' in next(t['adres'] for t in after_download['vkladki'] if t['id']==after_download['aktivnaya']),after_download
    results.append({'native_download_file_and_history':True,'download_keeps_source_document_url':True})
    gui.hotkey('ctrl','t');storage('recent-normal');gui.hotkey('ctrl','w');time.sleep(.3);gui.hotkey('ctrl','shift','t')
    deadline=time.monotonic()+8
    while time.monotonic()<deadline:
        current=session();active=next(t for t in current['vkladki'] if t['id']==current['aktivnaya'])
        if 'case=recent-normal' in active['adres']:break
        time.sleep(.1)
    assert 'case=recent-normal' in active['adres'],current
    gui.hotkey('ctrl','w');time.sleep(.3)
    results.append({'native_restore_recently_closed':True})
    # Во втором окне должны быть все Super, а выбор — локальным для окна.
    # Ранее тест сворачивал панель; имя Super в UIA тогда скрыто за инициалом.
    ui('Развернуть левую панель',['Button']).click_input();time.sleep(.4)
    assert storage('normal-loopback','normal-loopback-marker',origin=base)['seen']==['']
    ui('Новая супер-вкладка',['Button']).click_input();ui('Название',['Edit']).click_input();gui.hotkey('ctrl','a');gui.write('OtherSuperFixture',interval=.01)
    fill_start_url(base+'/storage?case=other-super-start');ui('Создать',['Button']).click_input()
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and not any(r.get('case')==['other-super-start'] for r in storage_reports):time.sleep(.1)
    assert any(r.get('case')==['other-super-start'] for r in storage_reports)
    def choose_super(name):
        super_button(name).click_input();time.sleep(.4)
    choose_super('Проба');main_before_extra=session();main_hwnd=hwnd;main_win=win
    def browser_windows():
        found=[]
        @ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
        def enum(h,_):
            pid=wintypes.DWORD();u.GetWindowThreadProcessId(h,ctypes.byref(pid))
            text=ctypes.create_unicode_buffer(512);u.GetWindowTextW(h,text,512)
            if pid.value==app.pid and u.IsWindowVisible(h) and text.value.startswith('NexusBro'):found.append(int(h))
            return True
        u.EnumWindows(enum,0);return found
    second=subprocess.Popen([str(pathlib.Path(a.exe).resolve())],env={**env,'NEXUSBRO_PROBA':'0'});second.wait(timeout=15)
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and len(browser_windows())<2:time.sleep(.1)
    windows=browser_windows();assert len(windows)==2 and app.poll() is None,windows
    hwnd=next(h for h in windows if h!=main_hwnd);u.SetForegroundWindow(hwnd);win=Desktop(backend='uia').window(handle=hwnd)
    assert storage('second-normal')['seen']==['normal-marker']
    choose_super('OtherSuperFixture');other_report=storage('second-other-super',origin=base);assert other_report['seen']==[''] and not other_report['cookie'][0],other_report
    choose_super('Проба');assert storage('second-normal-after-switch')['seen']==['normal-marker']
    assert session()['aktivnaya']==main_before_extra['aktivnaya']
    assert next(t for t in session()['vkladki'] if t['id']==session()['aktivnaya'])['adres']==next(t for t in main_before_extra['vkladki'] if t['id']==main_before_extra['aktivnaya'])['adres']
    results.append({'native_secondary_switches_supers_without_changing_main':True,'super_profiles_isolated_between_windows':True})
    ui('Меню NexusBro',['Button']).click_input();ui('Настройки NexusBro',['MenuItem']).click_input();ui('Закрыть настройки',['Button']).click_input()
    assert any(x.element_info.control_type=='Button' and 'Проба' in x.element_info.name for x in uia_descendants(win)),'No Super sidebar in secondary window'
    results.append({'secondary_full_shell_and_local_menu':True})
    gui.hotkey('alt','f4');time.sleep(.7);assert app.poll() is None
    hwnd=main_hwnd;win=main_win;u.SetForegroundWindow(hwnd)
    main_tab_controls=[x for x in uia_descendants(win) if x.element_info.control_type=='TabItem' and x.is_visible() and x.rectangle().top<rect(container).top]
    window_events_scoped=len(main_tab_controls)==len(session()['vkladki'])
    results.append({'window_events_are_isolated':window_events_scoped})
    if not window_events_scoped:
        # Собираем остальные независимые результаты даже на старом бинарнике;
        # строгая проверка в конце всё равно запрещает зелёный итог.
        count=len(session()['vkladki']);r=rect(container);gui.click(r.left+80,r.top+100);gui.hotkey('ctrl','t');wait_tabs(count+1)
        ui('Адрес страницы',['Edit','ComboBox']).click_input();gui.hotkey('ctrl','w');wait_tabs(count)

    # A private ordinary tab shares the Super exit, but has neither normal cookies nor localStorage.
    r=rect(container);gui.click(r.left+70,r.top-70,button='right');ui('Новая приватная вкладка',['MenuItem','Button']).click_input()
    private=storage('private-canary','private-marker');assert private['seen']==[''] and not private['cookie'][0],private
    ui('Новая обычная вкладка',['Button']).click_input();assert storage('ordinary-plus-from-private')['seen']==['normal-marker']
    gui.hotkey('ctrl','w');time.sleep(.3);assert storage('private-still')['seen']==['private-marker']
    results.append({'ordinary_plus_from_private_uses_normal_context':True})
    gui.hotkey('ctrl','w');time.sleep(.5)
    r=rect(container);gui.click(r.left+70,r.top-70,button='right');ui('Новая приватная вкладка',['MenuItem','Button']).click_input()
    private2=storage('private-again');assert private2['seen']==[''] and not private2['cookie'][0],private2
    gui.hotkey('ctrl','w');time.sleep(.4)
    assert storage('normal-after-private')['seen']==['normal-marker']
    for path in data.rglob('*.json'):
        contents=path.read_text(encoding='utf-8',errors='ignore');assert 'private-canary' not in contents and 'private-marker' not in contents,path
    results.append({'native_find_next':True,'native_history_and_bookmarks_panels':True,'second_exe_opens_shared_profile_window':True,'closing_secondary_keeps_main':True,'private_tab_storage_isolated_and_reset':True,'private_urls_not_persisted':True})
    # Приватное окно остаётся приватным и после закрытия последней страницы.
    gui.hotkey('ctrl','shift','n');time.sleep(.8)
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and len(browser_windows())<2:time.sleep(.1)
    windows=browser_windows();assert len(windows)==2,windows
    hwnd=next(h for h in windows if h!=main_hwnd);u.SetForegroundWindow(hwnd);win=Desktop(backend='uia').window(handle=hwnd)
    assert storage('private-window','private-window-marker')['seen']==['']
    gui.hotkey('ctrl','w');time.sleep(.4)
    assert storage('private-window-still')['seen']==['private-window-marker']
    gui.hotkey('alt','f4');time.sleep(.5);hwnd=main_hwnd;win=main_win;u.SetForegroundWindow(hwnd)
    # Определение приватной Super сохраняется, данные просмотра — только во временном каталоге.
    ui('Новая супер-вкладка',['Button']).click_input()
    ui('Название',['Edit']).click_input();gui.hotkey('ctrl','a');gui.write('PrivateSuperFixture',interval=.02)
    fill_start_url(base+'/storage?case=private-super-start')
    # Фокус браузера прокручивает форму к чекбоксу; не кликаем по его
    # исходным координатам за пределами viewport (там может быть taskbar).
    gui.press('tab',presses=3,interval=.1);gui.press('space')
    boxes=[x for x in uia_descendants(win) if x.element_info.control_type=='CheckBox' and x.element_info.name.startswith('Приватная супер-вкладка')]
    assert boxes and boxes[0].iface_toggle.CurrentToggleState==1,'Private Super checkbox is not checked'
    ui('Создать',['Button']).click_input()
    deadline=time.monotonic()+25
    while time.monotonic()<deadline and not any(r.get('case')==['private-super-start'] for r in storage_reports):time.sleep(.1)
    private_start_url=any(r.get('case')==['private-super-start'] for r in storage_reports)
    assert private_start_url,storage_reports
    results.append({'new_private_super_uses_start_url':True})
    assert storage('private-super-canary','private-super-marker',origin=base)['seen']==['']
    import tempfile
    private_root=pathlib.Path(tempfile.gettempdir())/f'NexusBro-private-{app.pid}'
    assert private_root.exists()
    definitions=json.loads((data/'vkladki.json').read_text(encoding='utf-8'));private_definition=next(r for r in definitions if r['nazvanie']=='PrivateSuperFixture');assert private_definition['privatnaya']
    for path in data.rglob('*.json'):
        contents=path.read_text(encoding='utf-8',errors='ignore');assert 'private-super-canary' not in contents and 'private-super-marker' not in contents,path
    # Сочетание отправляем в браузер, а не в окно runner после запуска CEF.
    ui('Адрес страницы',['Edit','ComboBox']).click_input()
    # Enable the real local vault, then lock from a focused CEF document.
    ui('Меню NexusBro',['Button']).click_input()
    ui('Пароли, аккаунт и синхронизация',['MenuItem','Button']).click_input()
    browser_master='native-windows-master-2026'
    for label in ['Мастер-пароль','Повторите мастер-пароль']:
        ui(label,['Edit']).click_input();gui.write(browser_master,interval=.01)
    ui('Создать хранилище',['Button']).click_input()
    deadline=time.monotonic()+12
    while time.monotonic()<deadline and not (data/'security/browser-vault.v1').is_file():time.sleep(.1)
    assert (data/'security/browser-vault.v1').is_file(),'Vault was not created from the real settings form'
    ui('Закрыть пароли и синхронизацию',['Button']).click_input()
    box=rect(main_hwnd);gui.click(box.right-130,box.bottom-140)
    gui.hotkey('ctrl','shift','l')
    ui('Мастер-пароль для разблокировки браузера',['Edit'])
    ImageGrab.grab().save(run/'browser-locked.png')
    sample=ImageGrab.grab().getpixel((box.right-130,box.bottom-140))[:3]
    assert sample!=(24,85,65),'CEF remained visible above the lock screen'
    ui('Мастер-пароль для разблокировки браузера',['Edit']).click_input();gui.write(browser_master,interval=.01)
    ui('Разблокировать браузер',['Button']).click_input()
    ui('Адрес страницы',['Edit','ComboBox'])
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        restored_rgb=ImageGrab.grab().getpixel((box.right-130,box.bottom-140))[:3]
        if restored_rgb==(24,85,65):break
        time.sleep(.1)
    assert restored_rgb==(24,85,65),('Native CEF page did not repaint after master unlock',restored_rgb)
    ImageGrab.grab().save(run/'browser-unlocked.png')
    results.append({'real_vault_create_and_cef_ctrl_shift_l_locks_browser':True,'master_unlock_restores_browser':True})
    focus_ui('Адрес страницы',['Edit','ComboBox'])
    class GuiThreadInfo(ctypes.Structure):
        _fields_=[('cbSize',wintypes.DWORD),('flags',wintypes.DWORD),('hwndActive',wintypes.HWND),('hwndFocus',wintypes.HWND),('hwndCapture',wintypes.HWND),('hwndMenuOwner',wintypes.HWND),('hwndMoveSize',wintypes.HWND),('hwndCaret',wintypes.HWND),('rcCaret',wintypes.RECT)]
    keyboard_info=GuiThreadInfo();keyboard_info.cbSize=ctypes.sizeof(keyboard_info)
    assert u.GetGUIThreadInfo(0,ctypes.byref(keyboard_info)),'Native keyboard focus query failed'
    native_focus_class=ctypes.create_unicode_buffer(256)
    u.GetClassNameW.argtypes=[wintypes.HWND,wintypes.LPWSTR,ctypes.c_int]
    u.GetClassNameW(keyboard_info.hwndFocus,native_focus_class,len(native_focus_class))
    focus_pid=wintypes.DWORD()
    focus_thread=u.GetWindowThreadProcessId(keyboard_info.hwndFocus,ctypes.byref(focus_pid))
    try: focus_process=psutil.Process(focus_pid.value).name()
    except (psutil.NoSuchProcess,psutil.AccessDenied): focus_process='unavailable'
    # CEF and WebView2 both use Chrome_RenderWidgetHostHWND; class alone
    # cannot prove which process owns physical keyboard focus.
    results.append({'shell_keyboard_native_focus':{'active':int(keyboard_info.hwndActive or 0),'focus':int(keyboard_info.hwndFocus or 0),'class':native_focus_class.value,'main':int(main_hwnd),'focus_pid':focus_pid.value,'focus_thread':focus_thread,'focus_process':focus_process}})
    assert keyboard_info.hwndActive==main_hwnd,'Physical keyboard target is not the main browser window'
    gui.hotkey('ctrl','shift','l')
    ui('Мастер-пароль для разблокировки браузера',['Edit'])
    ImageGrab.grab().save(run/'browser-shell-locked.png')
    ui('Мастер-пароль для разблокировки браузера',['Edit']).click_input();gui.write(browser_master,interval=.01)
    ui('Разблокировать браузер',['Button']).click_input()
    ui('Адрес страницы',['Edit','ComboBox'])
    results.append({'native_shell_ctrl_shift_l_locks_browser':True})
    # Закрытие главного окна сохраняет дополнительное; последнее завершает приложение.
    gui.hotkey('ctrl','n');time.sleep(.8)
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and len(browser_windows())<2:time.sleep(.1)
    extra=next(h for h in browser_windows() if h!=main_hwnd)
    u.SetForegroundWindow(main_hwnd);gui.hotkey('alt','f4');time.sleep(.6);assert app.poll() is None and u.IsWindowVisible(extra)
    u.SetForegroundWindow(extra);gui.hotkey('alt','f4');app.wait(timeout=15)
    deadline=time.monotonic()+5
    while time.monotonic()<deadline and private_root.exists():time.sleep(.1)
    assert not private_root.exists(),private_root
    results.append({'private_window_stays_private':True,'private_super_definition_saved':True,'private_temp_directory_removed':True,'closing_main_keeps_secondary':True,'last_window_exits':True})
    reports_before_restart=len(storage_reports)
    app=subprocess.Popen([str(pathlib.Path(a.exe).resolve())],env={**env,'NEXUSBRO_PROBA':'0'})
    deadline=time.monotonic()+30
    while time.monotonic()<deadline:
        windows=browser_windows()
        if windows:break
        time.sleep(.1)
    hwnd=windows[0];main_hwnd=hwnd;win=Desktop(backend='uia').window(handle=hwnd);u.SetForegroundWindow(hwnd)
    ui('Мастер-пароль для разблокировки браузера',['Edit'])
    time.sleep(.6)
    assert len(storage_reports)==reports_before_restart,'Sites loaded before the startup master password'
    ui('Мастер-пароль для разблокировки браузера',['Edit']).click_input();gui.write(browser_master,interval=.01)
    ui('Разблокировать браузер',['Button']).click_input()
    results.append({'startup_master_required_before_site_requests':True})
    # Название UIA включает приватный значок; ищем точную запись по содержимому.
    deadline=time.monotonic()+20
    while time.monotonic()<deadline:
        candidates=[x for x in uia_descendants(win) if x.element_info.control_type=='Button' and 'PrivateSuperFixture' in x.element_info.name]
        if candidates:break
        time.sleep(.1)
    assert candidates,'Private Super definition disappeared after restart';candidates[0].click_input()
    deadline=time.monotonic()+20;check=None
    while time.monotonic()<deadline:
        check=next((r for r in storage_reports[reports_before_restart:] if r.get('case')==['private-super-start']),None)
        if check:break
        time.sleep(.1)
    assert check and check['seen']==[''] and not check['cookie'][0],check
    results.append({'private_super_survives_restart_without_browsing_data':True})
    assert russian_shortcuts_ok,'Russian keyboard shortcuts failed'
    assert splitter_drag_ok,'Native DevTools boundary did not move'
    assert focus_after_tools,'Keyboard focus was lost after closing DevTools'
    assert find_ok,'Find match ordinal did not update correctly'
    assert download_name_ok,'Completed download name was missing'
    assert window_events_scoped,'Another window overwrote the main tab strip'
    assert private_start_url,'New private Super ignored its start URL'
    # Recovery runs after every preceding browsing/privacy assertion. Build a
    # synthetic credential through the actual form so restoration proves data,
    # not merely successful opening of another empty vault.
    import secrets
    from pywinauto.keyboard import send_keys as native_keys
    recovery_master='native-reset-windows-'+secrets.token_hex(12)
    recovery_login='native-recovery-login-'+secrets.token_hex(6)
    recovery_password='native-recovery-password-'+secrets.token_hex(12)
    recovery_title='VaultRecoveryCredentialFixture'
    recovery_sensitive=[browser_master,recovery_master,recovery_login,recovery_password]
    recovery_stage='save-original-credential'
    try:
        ui('Меню NexusBro',['Button']).click_input()
        ui('Пароли, аккаунт и синхронизация',['MenuItem','Button']).click_input()
        ui('Добавить вручную',['Button']).click_input()
        for label,value in [('Адрес сайта для пароля','https://vault-recovery.fixture.invalid'),('Логин сайта',recovery_login),('Пароль сайта',recovery_password),('Название записи пароля',recovery_title)]:
            ui(label,['Edit']).click_input();gui.write(value,interval=.005)
        ui('Супер-вкладка пароля',['ComboBox','Button']).click_input()
        ui('Проба',['ListItem']).click_input()
        ui('Сохранить пароль',['Button']).click_input()
        ui('Показать пароль: '+recovery_title,['Button'])
        ui('Закрыть пароли и синхронизацию',['Button']).click_input()
        security_root=data/'security'
        active_vault=security_root/'browser-vault.v1'
        old_ciphertext=active_vault.read_bytes()
        ordinary_metadata={name:(data/name).read_bytes() for name in ['vkladki.json','sohranennye.json','zakladki.json','nastrojki.json'] if (data/name).is_file()}

        def recovery_files():
            return {str(path.relative_to(security_root)):path.read_bytes() for path in security_root.rglob('*') if path.is_file()}
        def recovery_control(name,kinds):
            # Recovery cards scroll inside the screen. Never click a clipped
            # UIA element whose center falls outside the visible native client.
            for attempt in range(7):
                item=ui(name,kinds,timeout=6);r=item.rectangle();window_box=rect(main_hwnd)
                try:item.iface_scroll_item.ScrollIntoView()
                except Exception:pass
                r=item.rectangle()
                if r.width()>3 and r.height()>3 and r.top>=window_box.top+45 and r.bottom<=window_box.bottom-30:return item
                gui.moveTo((window_box.left+window_box.right)//2,window_box.bottom-140)
                gui.scroll(4 if r.top<window_box.top+45 else -4);time.sleep(.15)
            raise AssertionError('Recovery control remained clipped: '+name)
        def recovery_fill(name,value,unicode=False):
            recovery_control(name,['Edit']).click_input();gui.hotkey('ctrl','a')
            if unicode:native_keys(value,pause=.01,vk_packet=True)
            else:gui.write(value,interval=.005)
        def recovery_focus_receipt():
            # Санитизированная квитанция фокуса вокруг ЕДИНСТВЕННОГО shortcut:
            # роли/булевы только; исходная ошибка никогда не подменяется.
            receipt={'owned_foreground':False,'focus_role':'none','ctrl_released':True,
                     'shift_released':True,'uia_gate':'missing'}
            try:
                app_pid=wintypes.DWORD()
                u.GetWindowThreadProcessId(main_hwnd,ctypes.byref(app_pid))
                family={app_pid.value}
                try:
                    parent=psutil.Process(app_pid.value)
                    family|={child.pid for child in parent.children(recursive=True)}
                except (psutil.NoSuchProcess,psutil.AccessDenied):
                    pass
                foreground_hwnd=u.GetForegroundWindow()
                fg_pid=wintypes.DWORD()
                if foreground_hwnd:
                    u.GetWindowThreadProcessId(foreground_hwnd,ctypes.byref(fg_pid))
                receipt['owned_foreground']=bool(foreground_hwnd) and fg_pid.value in family
                class Info(ctypes.Structure):
                    _fields_=[('cbSize',wintypes.DWORD),('flags',wintypes.DWORD),('hwndActive',wintypes.HWND),('hwndFocus',wintypes.HWND),('hwndCapture',wintypes.HWND),('hwndMenuOwner',wintypes.HWND),('hwndMoveSize',wintypes.HWND),('hwndCaret',wintypes.HWND),('rcCaret',wintypes.RECT)]
                info=Info();info.cbSize=ctypes.sizeof(info)
                if u.GetGUIThreadInfo(0,ctypes.byref(info)) and info.hwndFocus:
                    focus_pid=wintypes.DWORD()
                    u.GetWindowThreadProcessId(info.hwndFocus,ctypes.byref(focus_pid))
                    name='';cmd=[]
                    try:
                        proc=psutil.Process(focus_pid.value)
                        name=proc.name();cmd=proc.cmdline()
                    except (psutil.NoSuchProcess,psutil.AccessDenied):
                        pass
                    receipt['focus_role']=recovery_focus_role(focus_pid.value,app_pid.value,family,name,cmd)
                else:
                    receipt['focus_role']='none'
                receipt['ctrl_released']=(u.GetAsyncKeyState(0x11)&0x8000)==0
                receipt['shift_released']=(u.GetAsyncKeyState(0x10)&0x8000)==0
                receipt['uia_gate']='present' if any(
                    x.element_info.name=='Мастер-пароль для разблокировки браузера'
                    and x.element_info.control_type=='Edit' for x in uia_descendants(win)) else 'missing'
            except Exception:
                receipt['receipt']='unavailable'
            return receipt
        def recovery_lock():
            focus_ui('Адрес страницы',['Edit','ComboBox'])
            before=receipt=recovery_focus_receipt()
            gui.hotkey('ctrl','shift','l')
            try:
                recovery_control('Мастер-пароль для разблокировки браузера',['Edit'])
            except Exception:
                results.append({'recovery_lock_focus':{'stage':recovery_stage,
                    'before':before,'after':recovery_focus_receipt()}})
                raise  # первоначальная ошибка сохраняется без изменений
            results.append({'recovery_lock_focus':{'stage':recovery_stage,
                'before':before,'after':recovery_focus_receipt()}})
        def recovery_choose_original():
            # IDs encode creation nanos; the core returns this exact newest-first
            # order. Match ciphertext bytes, never guess from duplicate dates.
            archives=sorted([path for path in security_root.glob('browser-vault.v1.recovery-*') if path.is_file()],key=lambda p:p.name,reverse=True)
            indices=[i for i,path in enumerate(archives) if path.read_bytes()==old_ciphertext]
            assert indices,'Exact pre-reset encrypted vault archive is missing'
            recovery_control('Копия хранилища',['ComboBox','Button']).click_input()
            gui.press('home')
            if indices[0]:gui.press('down',presses=indices[0],interval=.1)
            gui.press('enter')
        def recovery_wait_error():
            deadline=time.monotonic()+40
            while time.monotonic()<deadline:
                errors=[x for x in uia_descendants(win) if x.element_info.control_type in ('Text','Edit') and
                        any(fragment in x.element_info.name.lower() for fragment in ['неверн','поврежд','не удалось'])]
                if errors:return
                time.sleep(.1)
            raise AssertionError('Invalid recovery password was not rejected on screen')

        recovery_stage='initial-lock-before-reset-cancel'
        recovery_lock();before_cancel=recovery_files()
        recovery_control('Забыли мастер-пароль?',['Button']).click_input()
        recovery_fill('Новый мастер-пароль',recovery_master)
        recovery_fill('Повторите новый мастер-пароль',recovery_master)
        recovery_fill('Подтверждение сброса','НЕ СБРАСЫВАТЬ',unicode=True)
        reset_buttons=[x for x in uia_descendants(win) if x.element_info.name=='Сбросить хранилище' and x.element_info.control_type=='Button']
        assert reset_buttons and not reset_buttons[0].is_enabled(),'Reset enabled without exact confirmation'
        recovery_stage='cancel-reset-back-to-lock'
        recovery_control('Назад',['Button']).click_input()
        recovery_control('Мастер-пароль для разблокировки браузера',['Edit'])
        assert recovery_files()==before_cancel,'Cancelled reset changed protected files'
        recovery_control('Забыли мастер-пароль?',['Button']).click_input()
        recovery_fill('Новый мастер-пароль',recovery_master)
        recovery_fill('Повторите новый мастер-пароль',recovery_master)
        recovery_fill('Подтверждение сброса','СБРОСИТЬ',unicode=True)
        assert recovery_control('Подтверждение сброса',['Edit']).iface_value.CurrentValue=='СБРОСИТЬ','Unicode reset confirmation was not typed correctly'
        recovery_control('Сбросить хранилище',['Button']).click_input()
        ui('Адрес страницы',['Edit','ComboBox'])
        assert active_vault.read_bytes()!=old_ciphertext,'Reset retained the original active ciphertext'
        assert any(path.read_bytes()==old_ciphertext for path in security_root.glob('browser-vault.v1.recovery-*')),'Reset did not archive original ciphertext exactly'
        assert all((data/name).read_bytes()==raw for name,raw in ordinary_metadata.items()),'Reset modified ordinary browser metadata'
        ui('Меню NexusBro',['Button']).click_input();ui('Пароли, аккаунт и синхронизация',['MenuItem','Button']).click_input()
        deadline=time.monotonic()+20
        while time.monotonic()<deadline and not any(x.element_info.name=='Сохранённых паролей пока нет.' for x in uia_descendants(win)):time.sleep(.1)
        assert any(x.element_info.name=='Сохранённых паролей пока нет.' for x in uia_descendants(win)),'Reset exposed the previous credential list'
        ui('Аккаунт',['Button']).click_input();ui('Имя аккаунта NexusBro',['Edit'])
        ui('Закрыть пароли и синхронизацию',['Button']).click_input()
        results.append({'native_vault_reset_confirmation_cancel_exact_cipher_archive_empty_vault_no_account':True})

        # A real new process must demand the new master; old master cannot open it.
        recovery_stage='close-reset-app'
        prior_children=psutil.Process(app.pid).children(recursive=True)
        deadline=time.monotonic()+25
        while app.poll() is None and time.monotonic()<deadline:
            owned_windows=browser_windows()
            if owned_windows:
                u.SetForegroundWindow(owned_windows[0]);gui.hotkey('alt','f4')
            time.sleep(.3)
        app.wait(timeout=10)
        assert app.returncode==0,'Reset restart did not close the old application normally'
        _,remaining=psutil.wait_procs(prior_children,timeout=10)
        writers=[]
        for process in remaining:
            try:
                args=process.cmdline()
                if '--nb-dvizhok' in args or any(arg.startswith('--type=') for arg in args):writers.append(process.pid)
            except psutil.NoSuchProcess:pass
        assert not writers,'Old native profile writer survived the normal reset restart'
        recovery_stage='restart-reset-app-locked'
        recovery_requests_before_restart=len(storage_reports)
        app=subprocess.Popen([str(pathlib.Path(a.exe).resolve())],env={**env,'NEXUSBRO_PROBA':'0'})
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            windows=browser_windows()
            if windows:break
            time.sleep(.1)
        assert windows,'Reset app did not restart'
        hwnd=windows[0];main_hwnd=hwnd;win=Desktop(backend='uia').window(handle=hwnd);u.SetForegroundWindow(hwnd)
        recovery_control('Мастер-пароль для разблокировки браузера',['Edit'])
        time.sleep(.6)
        assert len(storage_reports)==recovery_requests_before_restart,'Reset startup loaded sites before the new master password'
        before_wrong=recovery_files()
        recovery_stage='reject-old-master-after-reset'
        recovery_fill('Мастер-пароль для разблокировки браузера',browser_master)
        recovery_control('Разблокировать браузер',['Button']).click_input();recovery_wait_error()
        assert recovery_files()==before_wrong,'Wrong old master modified the new vault'
        recovery_stage='unlock-new-master-after-reset'
        recovery_fill('Мастер-пароль для разблокировки браузера',recovery_master)
        recovery_control('Разблокировать браузер',['Button']).click_input()
        ui('Адрес страницы',['Edit','ComboBox'])
        results.append({'native_vault_reset_restart_requires_new_master_rejects_old_master':True})

        recovery_stage='relock-after-reset-restart'
        recovery_lock();current_ciphertext=active_vault.read_bytes();before_restore=recovery_files()
        recovery_stage='open-backup-before-wrong-master'
        recovery_control('Восстановить из копии',['Button']).click_input();recovery_choose_original()
        recovery_stage='reject-wrong-copy-master'
        recovery_fill('Мастер-пароль выбранной копии',recovery_master)
        recovery_control('Восстановить хранилище',['Button']).click_input();recovery_wait_error()
        time.sleep(2.2)  # Require the authoritative lock-status poll to agree.
        recovery_control('Мастер-пароль выбранной копии',['Edit'])
        assert recovery_files()==before_restore,'Wrong restore password changed protected files'
        recovery_stage='cancel-copy-restore-back-to-lock'
        recovery_control('Назад',['Button']).click_input()
        recovery_control('Мастер-пароль для разблокировки браузера',['Edit'])
        assert recovery_files()==before_restore,'Cancelled restore changed protected files'
        results.append({'native_wrong_vault_restore_and_cancel_stay_locked_with_exact_files':True})

        recovery_stage='restore-original-copy'
        recovery_control('Восстановить из копии',['Button']).click_input();recovery_choose_original()
        recovery_fill('Мастер-пароль выбранной копии',browser_master)
        recovery_control('Восстановить хранилище',['Button']).click_input()
        ui('Адрес страницы',['Edit','ComboBox'])
        assert any(path.read_bytes()==current_ciphertext for path in security_root.glob('browser-vault.v1.recovery-*')),'Restoring old archive did not preserve the new vault ciphertext'
        ui('Меню NexusBro',['Button']).click_input();ui('Пароли, аккаунт и синхронизация',['MenuItem','Button']).click_input()
        recovery_stage='verify-restored-secret'
        ui('Показать пароль: '+recovery_title,['Button']).click_input()
        recovered_value=None
        try:
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                fields=[x for x in uia_descendants(win) if x.element_info.name=='Пароль: '+recovery_title and x.element_info.control_type=='Edit']
                if fields:recovered_value=fields[0].iface_value.CurrentValue;break
                time.sleep(.1)
        finally:
            # Close exactly once, clearing the revealed value before screenshots
            # or propagating a failed comparison; do not depend on Escape timing.
            ui('Закрыть пароли и синхронизацию',['Button']).click_input()
        assert recovered_value==recovery_password,'Restored saved credential does not match the original'
        import base64
        for vault_path in [active_vault,security_root/'browser-vault.v1.bak',*security_root.glob('browser-vault.v1.recovery-*')]:
            raw=vault_path.read_bytes();envelope=json.loads(raw);payload=base64.b64decode(envelope['payload'])
            assert envelope['header']['format']=='nexusbro-vault' and payload,'Recovery archive is not a vault ciphertext envelope'
            assert all(secret.encode() not in raw and secret.encode() not in payload for secret in recovery_sensitive),'Recovery files contain a plaintext fixture credential'
        results.append({'native_vault_restore_old_master_returns_saved_secret_and_archives_replaced_new_cipher':True})
    except Exception as recovery_error:
        import traceback
        # Preserve the originating helper and call site without persisting frame
        # locals, source expressions, input values or the unredacted UIA dump.
        frames=[{'file':pathlib.Path(frame.filename).name,'line':frame.lineno,'function':frame.name}
                for frame in traceback.extract_tb(recovery_error.__traceback__)
                if pathlib.Path(frame.filename).name=='windows_physical.py']
        results.append({'native_vault_recovery_failure':{'stage':recovery_stage,
                        'exception':type(recovery_error).__name__,'caller_frames':frames}})
        message=str(recovery_error)
        for secret in recovery_sensitive:message=message.replace(secret,'[synthetic credential redacted]')
        raise AssertionError('Vault recovery stage '+recovery_stage+': '+message).with_traceback(recovery_error.__traceback__) from None
    results.append({'success': True})
finally:
    if not a.baseline and (data/'zhurnal.log').is_file():
        with (data/'zhurnal.log').open('rb') as journal_file:
            journal_file.seek(max(0,(data/'zhurnal.log').stat().st_size-1024*1024))
            journal_tail=journal_file.read(1024*1024).decode('utf-8',errors='replace')
        flags=[int(v) for v in re.findall(r'NEXUSBRO_BROWSER_LOCK callback_diag=physical_l flags=(\d{1,4})(?!\d)',journal_tail) if int(v)<=1023]
        errors={code:len(re.findall('NEXUSBRO_BROWSER_LOCK callback_diag='+code+r'(?![a-z_])',journal_tail)) for code in ('kind_failed','virtual_key_failed','physical_status_failed','set_handled_failed')}
        # Distinguish "handler never fired" from "L arrived but was rejected":
        # webview2-installed counts handler registrations, webview2-key counts
        # accepted Ctrl+Shift+L key events (journal markers, counts only).
        handler_facts={'installed_count':len(re.findall(r'NEXUSBRO_BROWSER_LOCK shortcut=webview2-installed(?![a-z-])',journal_tail)),
                       'accepted_key_count':len(re.findall(r'NEXUSBRO_BROWSER_LOCK shortcut=webview2-key(?![a-z-])',journal_tail))}
        results.append({'native_lock_callback_diagnostics':{'physical_l_flags':flags[-256:],'api_errors':errors,'handler_facts':handler_facts}})
    (run/'ad-reports.json').write_text(json.dumps(ad_reports,indent=2),encoding='utf-8')
    (run/'fixture-hits.json').write_text(json.dumps(hits,indent=2),encoding='utf-8')
    server.shutdown()
    firefox_source.close()
    if vpn_server:
        vpn_server.terminate();vpn_server.wait(timeout=10)
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
