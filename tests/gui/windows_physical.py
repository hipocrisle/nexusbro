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
        path=urllib.parse.urlsplit(self.path).path
        hits[path] = hits.get(path,0)+1
        if path=='/download-fixture':
            body=b'NexusBro download fixture\n'*256
            self.send_response(200);self.send_header('Content-Type','application/octet-stream');self.send_header('Content-Disposition','attachment; filename="nexusbro-fixture.txt"');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body);return
        if path=='/storage-result':storage_reports.append(urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query,keep_blank_values=True))
        if path=='/translate':
            body=json.dumps([[["Переведённые новости","Regular news",None,None]]],ensure_ascii=False).encode()
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
new MutationObserver(()=>fetch('/translation-result?text='+encodeURIComponent(document.getElementById('news').textContent))).observe(document.getElementById('news'),{subtree:true,characterData:true,childList:true});
setTimeout(async()=>{let blocked=false;try{await fetch('/ads/fetch-promo')}catch(e){blocked=true}await fetch('/control?utm_source=news');const p=new URLSearchParams({path:location.pathname,sponsored:String(!!document.getElementById('sponsored')),browser:String(!!document.getElementById('browser-ad')),ad:String(!!document.getElementById('yandex-ad')),news:String(!!document.getElementById('news')),blocked:String(blocked)});await fetch('/ad-result?'+p);document.title='AdsFixtureVerified'},1500)
</script>'''
        else:
            body=b'<!doctype html><title>RenderFixture</title><style>html,body{margin:0;background:#185541;width:100%;height:100%}</style>'
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
    fixture = base+'/page'
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
    # UI Automation locates controls; actual input is mouse clicks in WebView2.
    from pywinauto import Desktop
    win=Desktop(backend="uia").window(handle=hwnd)
    def ui(name, kinds=None):
        # ⛔ Новые управления WebView2 регистрируются в UIA не мгновенно, а
        # после нашей же перерисовки дерево может ещё показывать прошлое
        # состояние. Поэтому ищем дольше и с паузой, а в ошибке печатаем, что
        # реально видно — по этому видно, на каком шаге остановились.
        deadline=time.monotonic()+30
        videno=set();scrolled=0
        while time.monotonic()<deadline:
            items=[x for x in win.descendants() if x.element_info.name==name and (not kinds or x.element_info.control_type in kinds)]
            for x in items:
                if x.is_visible() and x.is_enabled():return x
                try:x.iface_scroll_item.ScrollIntoView()
                except Exception:pass
            if name in ['Отмена','Создать','Сохранить'] and scrolled<2 and time.monotonic()>deadline-28:
                r=rect(hwnd);gui.moveTo((r.left+r.right)//2,r.bottom-150);gui.scroll(-4);scrolled+=1
            videno={x.element_info.name for x in win.descendants() if x.element_info.name}
            time.sleep(.25)
        raise AssertionError('UIA control missing: '+name+' | видно: '+', '.join(sorted(videno)[:40]))
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
    gui.moveTo(border,y);gui.mouseDown();gui.moveTo(border-110,y,duration=.6);gui.mouseUp();time.sleep(.4)
    after=rect(tools[0]);splitter_drag_ok=after.left<before.left-60
    results.append({'native_devtools_boundary_drag':splitter_drag_ok,'before':before.left,'after':after.left})
    ImageGrab.grab().save(run/'devtools.png')
    gui.click(rect(container).left+80,rect(container).top+100);gui.press('f12');time.sleep(.5);assert app.poll() is None;assert not devtools_windows();u.SetForegroundWindow(hwnd)
    results.append({'native_f12_docked_devtools':True,'devtools_close_keeps_browser':True})
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
    tab_controls=sorted([x for x in win.descendants() if x.element_info.control_type=='TabItem' and x.is_visible() and x.rectangle().top<rect(container).top],key=lambda x:x.rectangle().left)
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
    while time.monotonic()<deadline and any(x.element_info.name=='Настройки браузера' for x in win.descendants()):time.sleep(.1)
    ui('Настройки NexusBro',['Button']).click_input()
    ui('Шрифт интерфейса',['ComboBox','Button']).click_input()
    ui('Arial',['ListItem','Button']).click_input()
    ui('Сохранить',['Button']).click_input()
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        cfg=json.loads((data/'nastrojki.json').read_text(encoding='utf-8'))
        if cfg['shrift']=='arial':break
        time.sleep(.1)
    assert cfg['shrift']=='arial',cfg
    deadline=time.monotonic()+10
    while time.monotonic()<deadline and any(x.element_info.name=='Настройки браузера' for x in win.descendants()):time.sleep(.1)
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
            'routing':{'rules':[{'type':'field','domain':['full:vpn-fixture.test','full:yandex.ru'],'outboundTag':'fixture'}]}}
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
        (run/'vpn-diagnostic.json').write_text(json.dumps({'hits':hits,'server_exit':vpn_server.poll(),'uia':[x.element_info.name for x in win.descendants()]},ensure_ascii=False,indent=2),encoding='utf-8')
        raise AssertionError('CEF did not restart')
    container=new_container();hwnd=u.FindWindowW(None,'NexusBro');assert u.SetWindowPos(hwnd,None,20,20,width,height,0x0040);win=Desktop(backend='uia').window(handle=hwnd);u.SetForegroundWindow(hwnd);time.sleep(.5)
    def edit_super():
        pt=origin();gui.click(pt.x+26,pt.y+80,button='right');ui('Настройки').click_input()
    edit_super()
    ui('Сохранённое подключение',['ComboBox','Button']).click_input();ui('Local subscription',['ListItem','Button']).click_input()
    ui('Узел подписки',['ComboBox','Button']).click_input();ui('Local working',['ListItem','Button']).click_input()
    gui.press('tab',presses=5,interval=.1);time.sleep(.3)
    previous=container;ui('Сохранить',['Button']).click_input();container=new_container(previous)
    assert hits.get('/probe'),hits
    assert not consoles()-old_consoles,'Xray created a visible console'
    def vpn_page(path,host='vpn-fixture.test'):
        ui('Адрес страницы',['Edit']).click_input();gui.hotkey('ctrl','a');time.sleep(.2);gui.write('http://'+host+'/'+path,interval=.002);gui.press('enter')
        deadline=time.monotonic()+15
        while time.monotonic()<deadline and not hits.get('/'+path):time.sleep(.1)
        assert hits.get('/'+path),hits
        pixels(path)
    vpn_page('vpn-working')
    old_records=json.loads((data/'vkladki.json').read_text(encoding='utf-8'))
    assert old_records[0]['proxy_uzel']==local,old_records
    edit_super();ui('Узел подписки',['ComboBox','Button']).click_input();ui('Unavailable',['ListItem','Button']).click_input()
    gui.press('tab',presses=5,interval=.1);time.sleep(.3);ui('Сохранить',['Button']).click_input()
    deadline=time.monotonic()+20
    message='Выбранный узел не прошёл проверку HTTP(S). Прежние настройки и подключение сохранены. Выберите другой узел или повторите проверку.'
    while time.monotonic()<deadline:
        if any(x.element_info.name==message for x in win.descendants()):break
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
    ui('Показать оригинал',['Button']).click_input()
    deadline=time.monotonic()+10
    while time.monotonic()<deadline and not any(r.get('text')==['Regular news'] for r in translation_reports):time.sleep(.1)
    assert any(r.get('text')==['Regular news'] for r in translation_reports),translation_reports
    results.append({'native_translation_same_document_and_original':True,'translation_uses_super_proxy':hits.get('/translate',0)>0,'cef_ad_resources_cancelled':True,'cef_browser_and_yandex_banners_removed':True,'regular_news_preserved':True})
    # Выключатель защиты должен отключать и ResourceRequestHandler, и DOM-скрипт.
    app.terminate();app.wait(timeout=10);time.sleep(.5)
    records=json.loads((data/'vkladki.json').read_text(encoding='utf-8'))
    for record in records:record['rezat_reklamu']=False
    (data/'vkladki.json').write_text(json.dumps(records),encoding='utf-8');journal.unlink(missing_ok=True)
    app=subprocess.Popen([str(pathlib.Path(a.exe).resolve())],env={**env,'NEXUSBRO_PROBA':'0'})
    container=new_container();hwnd=u.FindWindowW(None,'NexusBro');assert u.SetWindowPos(hwnd,None,20,20,width,height,0x0040);win=Desktop(backend='uia').window(handle=hwnd);u.SetForegroundWindow(hwnd);time.sleep(.5)
    # При выключенной защите красная плашка ожидаема: проверяем DOM через ответ фикстуры.
    ui('Адрес страницы',['Edit']).click_input();gui.hotkey('ctrl','a');gui.write('http://yandex.ru/ad-fixture-off',interval=.002);gui.press('enter')
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
        ui('Адрес страницы',['Edit']).click_input();gui.hotkey('ctrl','a');gui.write(origin+'/storage?'+urllib.parse.urlencode(params),interval=.001);gui.press('enter')
        deadline=time.monotonic()+15
        while time.monotonic()<deadline:
            report=next((r for r in storage_reports if r.get('case')==[case]),None)
            if report:return report
            time.sleep(.1)
        raise AssertionError(('No storage report',case,storage_reports))
    storage('normal-main','normal-marker')
    r=rect(container);gui.click(r.left+80,r.top+100);gui.hotkey('ctrl','f')
    ui('Найти на странице',['Edit']).click_input();gui.write('Findneedle');time.sleep(.8)
    deadline=time.monotonic()+6
    while time.monotonic()<deadline and not any(x.element_info.name=='1 из 2' for x in win.descendants()):time.sleep(.1)
    find_ok=any(x.element_info.name=='1 из 2' for x in win.descendants())
    gui.press('enter');time.sleep(.5)
    find_ok=find_ok and any(x.element_info.name=='2 из 2' for x in win.descendants())
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
        ru_stage='address_focus';r=rect(container);gui.click(r.left+60,r.top+90);gui.hotkey('ctrl','l');address=ui('Адрес страницы',['Edit']);deadline=time.monotonic()+3
        while time.monotonic()<deadline and not address.has_keyboard_focus():time.sleep(.05)
        assert address.has_keyboard_focus(),'Ctrl+L did not focus the address input in Russian layout'
        ru_stage='shell_new_tab';gui.hotkey('ctrl','t');wait_tabs(before_count+1);ru_stage='shell_close_tab';gui.hotkey('ctrl','w');wait_tabs(before_count)
    except AssertionError as error:russian_shortcuts_ok=False;ru_error=ru_stage+': '+str(error)[:200]
    finally:input_layout('00000409')
    # Продолжаем независимые сценарии, но общий итог остаётся красным при провале RU.
    while len(session()['vkladki'])>before_count:gui.hotkey('ctrl','w');time.sleep(.15)
    results.append({'native_russian_shortcuts_cef_and_shell':russian_shortcuts_ok,'error':ru_error})
    # Собственное адресное меню обращается к системному clipboard, не WebView permissions.
    address=ui('Адрес страницы',['Edit']);address.click_input();gui.hotkey('ctrl','a');expected_address=address.iface_value.CurrentValue;address.click_input(button='right')
    def address_action(prefix):
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            controls=list(win.descendants())
            for popup in Desktop(backend='uia').windows(process=app.pid):
                if popup.handle!=hwnd:controls.extend(popup.descendants())
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
    gui.hotkey('ctrl','h');ui('Закрыть библиотеку',['Button']).click_input()
    ui('Адрес страницы',['Edit']).click_input();gui.hotkey('ctrl','a');gui.write('http://vpn-fixture.test/download-fixture',interval=.001);gui.press('enter')
    deadline=time.monotonic()+15
    while time.monotonic()<deadline and not (run/'downloads/nexusbro-fixture.txt').exists():time.sleep(.1)
    assert (run/'downloads/nexusbro-fixture.txt').read_bytes()==b'NexusBro download fixture\n'*256
    gui.hotkey('ctrl','j');ui('Закрыть библиотеку',['Button']).click_input()
    deadline=time.monotonic()+8
    while time.monotonic()<deadline:
        downloads=json.loads((data/'zagruzki.json').read_text(encoding='utf-8'))
        if any(d['name']=='nexusbro-fixture.txt' and d['status']=='Готово' for d in downloads):break
        time.sleep(.1)
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
    ui('Открыть сразу',['Edit']).click_input();gui.hotkey('ctrl','a');gui.write(base+'/storage?case=other-super-start',interval=.001);ui('Создать',['Button']).click_input()
    deadline=time.monotonic()+20
    while time.monotonic()<deadline and not any(r.get('case')==['other-super-start'] for r in storage_reports):time.sleep(.1)
    assert any(r.get('case')==['other-super-start'] for r in storage_reports)
    def choose_super(name):
        deadline=time.monotonic()+15
        while time.monotonic()<deadline:
            matches=[x for x in win.descendants() if x.element_info.control_type=='Button' and name in x.element_info.name and x.is_visible()]
            if matches:matches[0].click_input();time.sleep(.4);return
            time.sleep(.1)
        raise AssertionError('Super button missing: '+name)
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
    ui('Меню NexusBro',['Button']).click_input();ui('Настройки NexusBro',['MenuItem','Button']).click_input();ui('Закрыть настройки',['Button']).click_input()
    assert any(x.element_info.control_type=='Button' and 'Проба' in x.element_info.name for x in win.descendants()),'No Super sidebar in secondary window'
    results.append({'secondary_full_shell_and_local_menu':True})
    gui.hotkey('alt','f4');time.sleep(.7);assert app.poll() is None
    hwnd=main_hwnd;win=main_win;u.SetForegroundWindow(hwnd)
    main_tab_controls=[x for x in win.descendants() if x.element_info.control_type=='TabItem' and x.is_visible() and x.rectangle().top<rect(container).top]
    window_events_scoped=len(main_tab_controls)==len(session()['vkladki'])
    results.append({'window_events_are_isolated':window_events_scoped})
    if not window_events_scoped:
        # Собираем остальные независимые результаты даже на старом бинарнике;
        # строгая проверка в конце всё равно запрещает зелёный итог.
        count=len(session()['vkladki']);r=rect(container);gui.click(r.left+80,r.top+100);gui.hotkey('ctrl','t');wait_tabs(count+1)
        ui('Адрес страницы',['Edit']).click_input();gui.hotkey('ctrl','w');wait_tabs(count)

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
    ui('Открыть сразу',['Edit']).click_input();gui.hotkey('ctrl','a');gui.write(base+'/storage?case=private-super-start',interval=.001)
    # Фокус браузера прокручивает форму к чекбоксу; не кликаем по его
    # исходным координатам за пределами viewport (там может быть taskbar).
    gui.press('tab',presses=3,interval=.1);gui.press('space')
    boxes=[x for x in win.descendants() if x.element_info.control_type=='CheckBox' and x.element_info.name.startswith('Приватная супер-вкладка')]
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
    ui('Адрес страницы',['Edit']).click_input()
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
    app=subprocess.Popen([str(pathlib.Path(a.exe).resolve())],env={**env,'NEXUSBRO_PROBA':'0'})
    deadline=time.monotonic()+30
    while time.monotonic()<deadline:
        windows=browser_windows()
        if windows:break
        time.sleep(.1)
    hwnd=windows[0];main_hwnd=hwnd;win=Desktop(backend='uia').window(handle=hwnd);u.SetForegroundWindow(hwnd)
    # Название UIA включает приватный значок; ищем точную запись по содержимому.
    deadline=time.monotonic()+20
    while time.monotonic()<deadline:
        candidates=[x for x in win.descendants() if x.element_info.control_type=='Button' and 'PrivateSuperFixture' in x.element_info.name]
        if candidates:break
        time.sleep(.1)
    assert candidates,'Private Super definition disappeared after restart';candidates[0].click_input()
    check=storage('private-super-after-restart',origin=base);assert check['seen']==[''] and not check['cookie'][0],check
    results.append({'private_super_survives_restart_without_browsing_data':True})
    assert russian_shortcuts_ok,'Russian keyboard shortcuts failed'
    assert splitter_drag_ok,'Native DevTools boundary did not move'
    assert focus_after_tools,'Keyboard focus was lost after closing DevTools'
    assert find_ok,'Find match ordinal did not update correctly'
    assert download_name_ok,'Completed download name was missing'
    assert window_events_scoped,'Another window overwrote the main tab strip'
    assert private_start_url,'New private Super ignored its start URL'
    results.append({'success': True})
finally:
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
