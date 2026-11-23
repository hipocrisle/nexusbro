#!/usr/bin/env python3
"""One-app native essentials acceptance on disposable Windows/Linux runners.

Uses real Tauri IPC/CEF, physical keys/clicks, real loopback HTTP and OS process
measurements. Inspector JS observes state; it never injects application events,
network failures, button clicks or replacement IPC results. Not run on DE-1.
"""
import argparse
import collections
import http.server
import json
import os
import pathlib
import socket
import shutil
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request

import psutil
import websocket
from client import Client
from security_native import Device, call, js_wait, painted, require, settings, super_record, wait_for
from tab_routes import WindowsDevice, WindowsClient, activate_saved, click_control, write_json


def keys(device, *names):
    if sys.platform == 'win32':
        import pyautogui
        pyautogui.hotkey(*names)
    else:
        linux = {'esc':'Escape','enter':'Return'}
        chord = '+'.join(linux.get(name,name) for name in names)
        subprocess.run(['xdotool','key','--clearmodifiers',chord],
            env={**os.environ,'DISPLAY':device.display},check=True,timeout=5)



def fill_search(client,text):
    selector='input[aria-label="Найти вкладку или супервкладку"]'
    if sys.platform!='win32':
        client.fill(selector,text)
        return
    # These fixture queries are deliberately ASCII; no clipboard or DOM input
    # injection is used. WindowsClient inherits a Linux-only fill() otherwise.
    require(text.isascii(),'Physical search fixture requires an ASCII query')
    import pyautogui
    client.click(selector)
    pyautogui.hotkey('ctrl','a')
    pyautogui.write(text,interval=.01)
    js_wait(client,'document.querySelector('+json.dumps(selector)+').value==='+json.dumps(text),
            'Physical Windows typing did not reach the actual search input',5)


def foreground(client):
    call(client,'fokus_obolochke')
    click_control(client,'input[aria-label="Адрес страницы"]',15)
    js_wait(client,'document.hasFocus()&&document.activeElement?.getAttribute("aria-label")==="Адрес страницы"',
            'Native shell address did not own physical keyboard focus')


def focus_initial_shell(client):
    """Bring the actual main WebView forward before the first physical tab click.

    Its address input can legitimately be disabled while restored rows are lazy.
    Do not use foreground(), whose editable-address assertion belongs after load.
    """
    require(client.js('window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')=='main',
            'Initial activation inspector does not belong to the main native shell')
    call(client,'fokus_obolochke')
    js_wait(client,'document.hasFocus()',
            'Initial main shell did not own desktop focus before physical activation',15)
    painted(client)


def copy_failure_logs(device,out,report):
    if not device:return
    sources=list(device.root.glob('*.log'))
    if device.data.exists():
        sources.extend(device.data.glob('zhurnal*.log'))
        for pattern in ('profili/*/engine.log','profili/*/engine.log.1','profili/*/cef*.log'):
            sources.extend(device.data.glob(pattern))
    copied=[];errors=[]
    for index,source in enumerate(sorted(set(sources))):
        if not source.is_file() or source.is_symlink():continue
        target=out/('failure-owned-'+str(index)+'-'+source.name+('' if source.name.endswith('.log') else '.log'))
        try:shutil.copyfile(source,target);copied.append(target.name)
        except OSError as error:errors.append(type(error).__name__)
    report['failure_logs']=copied
    if errors:report['failure_log_copy_errors']=errors


def failure_diagnostics(device,out,report):
    """Owned-window IDs, rectangles and focus only; never read form values/text."""
    if not device:return
    try:
        from PIL import ImageGrab
        ImageGrab.grab(**({'xdisplay':device.display} if sys.platform!='win32' else {})).save(out/'native-failure-full-screen.png')
        report['failure_full_screen_saved']=True
    except Exception as error:report['failure_screenshot_error']=type(error).__name__
    observers=[]
    for label,observer in getattr(device,'essentials_clients',{}).items():
        try:
            state=observer.js('(()=>{const e=document.activeElement,r=e?.getBoundingClientRect();return {document_has_focus:document.hasFocus(),window_width:innerWidth,window_height:innerHeight,active_tag:e?.tagName??null,active_disabled:!!e?.disabled,active_rect:r?[r.x,r.y,r.width,r.height]:null}})()')
            observers.append({'window':label,'state':state})
        except Exception as error:observers.append({'window':label,'error_type':type(error).__name__})
    report['failure_shell_focus']=observers
    try:
        process=psutil.Process(device.app.pid)
        pids={process.pid}|{p.pid for p in process.children(recursive=True)}
        if sys.platform=='win32':
            import ctypes
            from ctypes import wintypes
            user=ctypes.WinDLL('user32',use_last_error=True)
            user.GetForegroundWindow.restype=wintypes.HWND
            user.GetWindowThreadProcessId.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.DWORD)]
            user.GetWindowThreadProcessId.restype=wintypes.DWORD
            user.GetWindowRect.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.RECT)]
            user.IsWindowVisible.argtypes=[wintypes.HWND]
            callback_type=ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
            user.EnumWindows.argtypes=[callback_type,wintypes.LPARAM]
            windows=[]
            def collect(hwnd,_parameter):
                pid=wintypes.DWORD();thread=user.GetWindowThreadProcessId(hwnd,ctypes.byref(pid))
                if pid.value in pids:
                    rect=wintypes.RECT();have_rect=user.GetWindowRect(hwnd,ctypes.byref(rect))
                    windows.append({'handle':int(hwnd),'pid':pid.value,'thread':thread,'visible':bool(user.IsWindowVisible(hwnd)),
                        'rect':[rect.left,rect.top,rect.right,rect.bottom] if have_rect else None})
                return True
            user.EnumWindows(callback_type(collect),0)
            hwnd=user.GetForegroundWindow();pid=wintypes.DWORD();thread=user.GetWindowThreadProcessId(hwnd,ctypes.byref(pid))
            class GuiThreadInfo(ctypes.Structure):
                _fields_=[('size',wintypes.DWORD),('flags',wintypes.DWORD),('active',wintypes.HWND),('focus',wintypes.HWND),('capture',wintypes.HWND),('menu',wintypes.HWND),('move_size',wintypes.HWND),('caret',wintypes.HWND),('caret_rect',wintypes.RECT)]
            user.GetGUIThreadInfo.argtypes=[wintypes.DWORD,ctypes.POINTER(GuiThreadInfo)]
            gui=GuiThreadInfo();gui.size=ctypes.sizeof(gui)
            info={'foreground_handle':int(hwnd or 0),'foreground_pid':pid.value,'foreground_thread':thread,'owned_windows':windows}
            if user.GetGUIThreadInfo(thread,ctypes.byref(gui)):
                info.update({'active_handle':int(gui.active or 0),'focus_handle':int(gui.focus or 0),'capture_handle':int(gui.capture or 0)})
            report['failure_os_focus']=info
        else:
            env={**os.environ,'DISPLAY':device.display}
            focus=subprocess.run(['xdotool','getwindowfocus'],env=env,capture_output=True,text=True,timeout=3)
            windows=[]
            for pid in pids:
                result=subprocess.run(['xdotool','search','--onlyvisible','--pid',str(pid)],env=env,capture_output=True,text=True,timeout=3)
                if result.returncode==0:windows.extend({'handle':int(xid),'pid':pid} for xid in result.stdout.splitlines() if xid.isdigit())
            report['failure_os_focus']={'focus_handle':int(focus.stdout.strip()) if focus.returncode==0 and focus.stdout.strip().isdigit() else None,'owned_visible_windows':windows}
    except Exception as error:report['failure_os_focus_error']=type(error).__name__
    copy_failure_logs(device,out,report)


def main_rows(client):
    return call(client,'obychnye_spisok',id='essentials-main')


def loaded(client, address, timeout=35):
    def ready():
        current=main_rows(client)
        row=next((row for row in current['vkladki'] if row['id']==current['aktivnaya']),None)
        return row if row and row.get('adres')==address and row.get('zagolovok')=='EssentialsNativeFixture' and not row.get('zagruzka') else None
    return wait_for(ready,'Actual CEF fixture did not load at the selected ordinary row',timeout)


def connect_window(device,label):
    """One retained inspector connection per native shell route.

    WebKit disconnects an existing inspector when another attaches to the same
    route. Never probe a route already owned by this fixture's main/extra client.
    """
    clients=getattr(device,'essentials_clients',None)
    if clients is None:
        clients={};device.essentials_clients=clients;device.essentials_routes=set()
    if label in clients:
        require(device.app.poll() is None,'Native application exited before inspector lookup')
        require(clients[label].js('window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')==label,
                'Retained inspector no longer belongs to its recorded native window')
        return clients[label]
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    if sys.platform!='win32':
        with opener.open(f'http://127.0.0.1:{device.inspector}',timeout=3) as response:
            import re
            routes=re.findall(r'/socket/\d+/\d+/WebPage',response.read().decode())
        endpoints=[f'ws://127.0.0.1:{device.inspector}'+route for route in routes]
    else:
        with opener.open(f'http://127.0.0.1:{device.inspector}/json/list',timeout=3) as response:
            pages=json.load(response)
        endpoints=[page['webSocketDebuggerUrl'] for page in pages
                   if page.get('type')=='page' and page.get('webSocketDebuggerUrl')]
    for endpoint_url in endpoints:
        if endpoint_url in device.essentials_routes:
            continue
        endpoint=urllib.parse.urlsplit(endpoint_url)
        require(endpoint.scheme=='ws' and endpoint.hostname in ('127.0.0.1','localhost')
                and endpoint.port==device.inspector and not endpoint.username and not endpoint.password,
                'Inspector target escaped the owned loopback endpoint')
        transport=socket.create_connection(('127.0.0.1',device.inspector),timeout=3)
        candidate=WindowsClient.__new__(WindowsClient) if sys.platform=='win32' else Client.__new__(Client)
        candidate.seq=0;candidate.display=getattr(device,'display',None)
        try:
            candidate.ws=websocket.create_connection(endpoint_url,socket=transport,
                timeout=5,suppress_origin=True,http_proxy_host=None,http_no_proxy=['127.0.0.1','localhost'])
            if sys.platform!='win32':
                candidate.target='page-7';candidate.ws.settimeout(.3)
                try:
                    event=json.loads(candidate.ws.recv())
                    if event.get('method')=='Target.targetCreated':
                        candidate.target=event['params']['targetInfo']['targetId']
                except websocket.WebSocketTimeoutException:
                    pass
                candidate.ws.settimeout(5)
            actual=candidate.js('window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')
            if not isinstance(actual,str) or not actual:
                candidate.close();continue
            require(actual not in clients,'One shell label advertised multiple inspector routes')
            candidate.essentials_endpoint=endpoint_url
            clients[actual]=candidate;device.essentials_routes.add(endpoint_url)
            if actual==label:
                return candidate
        except (OSError,RuntimeError,websocket.WebSocketException):
            if getattr(candidate,'ws',None): candidate.close()
            else: transport.close()
    return None


def image(device,path):
    from PIL import ImageGrab
    painted(device.client)
    ImageGrab.grab(**({'xdisplay':device.display} if sys.platform!='win32' else {})).save(path)


def run(args):
    out=pathlib.Path(args.out).resolve()
    out.mkdir(parents=True,exist_ok=False)
    report={'schema':1,'completed':False,'platform':sys.platform,'checks':[],
            'scope':'one native app; real CEF/IPC, OS metrics and physical input',
            'coverage':{'search':'not_executed','tasks':'not_executed','network_error':'not_executed'}}
    device=server=recovered_server=unrelated=extra_client=blocked=None
    counts=collections.Counter()
    count_lock=threading.Lock()
    try:
        require(os.environ.get('GITHUB_ACTIONS')=='true','Only disposable GitHub desktop runners may run native acceptance')
        exe,cef=pathlib.Path(args.exe).resolve(),pathlib.Path(args.cef).resolve()
        require(exe.is_file() and cef.is_dir(),'Built app and CEF are required')

        class Page(http.server.BaseHTTPRequestHandler):
            def log_message(self,*_): pass
            def do_GET(self):
                path=urllib.parse.urlsplit(self.path).path
                with count_lock: counts[path]+=1
                body=b'<!doctype html><meta charset="utf-8"><title>EssentialsNativeFixture</title><style>html,body{margin:0;background:#185541;color:white;min-height:100vh}</style><main>Real native essentials fixture</main><script>addEventListener("pointerdown",e=>{if(e.isTrusted)fetch("/physical-pointer"+location.pathname)})</script>'
                self.send_response(200);self.send_header('Content-Type','text/html; charset=utf-8')
                self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)

        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Page)
        threading.Thread(target=server.serve_forever,daemon=True).start()
        base=f'http://127.0.0.1:{server.server_port}'
        device=WindowsDevice(out/'device',exe,cef) if sys.platform=='win32' else Device('essentials',out/'device',exe,cef)
        config=settings('grafit','unused');config['vyhody']=[]
        write_json(device.data/'nastrojki.json',config)
        definitions=[super_record('essentials-main','Основная проверка',base+'/loaded'),
                     super_record('essentials-sleeping','Спящая проверка',base+'/sleeping-super'),
                     super_record('pvt-essentials','Приватная проверка',base+'/private-super',private=True)]
        for index,definition in enumerate(definitions): definition['poryadok']=index
        write_json(device.data/'vkladki.json',definitions)
        write_json(device.data/'sohranennye.json',[super_record('essentials-saved','Сохранённая проверка','about:blank')])
        def tab(identifier,path,title,route,private=False):
            return {'id':identifier,'adres':base+path,'zagolovok':title,'route':route,'privatnaya':private,'otlozhena':True}
        write_json(device.data/'profili/essentials-main/obychnye.json',{
            'vkladki':[tab(1,'/loaded','Главная проверка',{'kind':'inherit'}),
                       tab(2,'/lazy-target','Ленивая цель',{'kind':'direct'})],
            'aktivnaya':1,'sleduyushchaya':3,'zakrytye':[]})
        write_json(device.data/'profili/essentials-sleeping/obychnye.json',{
            'vkladki':[tab(1,'/sleeping-super','Спящий движок',{'kind':'inherit'})],
            'aktivnaya':1,'sleduyushchaya':2,'zakrytye':[]})
        write_json(device.data/'windows-recovery.json',{'schema':1,'windows':[{
            'key':'essentials-cold-window','active_super':'essentials-main','views':[{
                'super_id':'essentials-main','session':{
                    'vkladki':[tab(901,'/cold-secondary-old','Старую активную не загружать',{'kind':'inherit'}),
                               tab(902,'/cold-secondary-target','Холодная цель окна',{'kind':'direct'})],
                    'aktivnaya':901,'sleduyushchaya':903,'zakrytye':[]}}]}]})
        unrelated=subprocess.Popen([sys.executable,'-c','import time; time.sleep(180)'],
                                   stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        initial_client=device.start()
        # Device's bootstrap connection has no recorded endpoint. Close it
        # explicitly before creating the fixture-owned, route-cached observers.
        initial_client.close();device.client=None
        client=wait_for(lambda:connect_window(device,'main'),'Main native inspector unavailable',30)
        device.client=client
        focus_initial_shell(client)
        activate_saved(client,'essentials-main')
        loaded(client,base+'/loaded')
        with count_lock:
            require(counts['/lazy-target']==0 and counts['/sleeping-super']==0,'Cold startup eagerly visited inactive tabs')
        foreground(client);keys(device,'ctrl','shift','a')
        search='[role="dialog"][aria-labelledby="tab-search-title"]'
        js_wait(client,'!!document.querySelector('+json.dumps(search)+')','Physical shell Ctrl+Shift+A did not open search')
        snapshot=call(client,'tab_search_snapshot')
        require(any(r['address']==base+'/lazy-target' and r['sleeping'] for r in snapshot),'Lazy tab absent from search')
        require(any(r['super_id']=='essentials-saved' and r['tab_id'] is None for r in snapshot),'Saved Super absent from search')
        require(all(not r['super_id'].startswith('pvt-') and '/private' not in r['address'] for r in snapshot),'Private Super leaked into global search')
        time.sleep(.3)
        with count_lock:
            require(counts['/lazy-target']==0 and counts['/sleeping-super']==0,'Search snapshot launched a lazy tab or engine')
        states=call(client,'spisok')
        require(not next(r for r in states if r['id']=='essentials-sleeping')['zapushchena'],'Search launched a sleeping Super engine')
        fill_search(client,'lazy-target')
        js_wait(client,'document.querySelectorAll("#tab-search-results [role=option]").length===1','Search did not filter exact lazy target')
        keys(device,'enter');lazy=loaded(client,base+'/lazy-target')
        require(lazy['route']=={'kind':'direct'},'Search changed the lazy target route')
        js_wait(client,'!document.querySelector('+json.dumps(search)+')','Search did not close after activation')
        report['checks'].append('physical_shell_search_lazy_exact_row_route_no_background_engine_start')
        # A private ordinary tab is visible in its native view, never in global search.
        call(client,'obychnaya_komanda',id='essentials-main',komanda={'deystvie':'novaya_privatnaya','adres':base+'/private-ordinary'})
        wait_for(lambda:any(t.get('privatnaya') and t['adres']==base+'/private-ordinary' and not t['zagruzka'] for t in main_rows(client)['vkladki']),
                 'Actual private ordinary fixture did not load')
        require(all('/private' not in r['address'] for r in call(client,'tab_search_snapshot')),'Private ordinary page leaked into search')
        call(client,'obychnaya_komanda',id='essentials-main',komanda={'deystvie':'vybrat','id':lazy['id']})
        loaded(client,base+'/lazy-target')
        # Keyboard event starts in CEF, not a shell input.
        call(client,'pokazat_dvizhki',vidno=True)
        point=js_wait(client,'(()=>{const e=document.querySelector(".pole");if(!e)return null;const r=e.getBoundingClientRect();return r.width>300&&r.height>200?[r.x+200,r.y+150]:null})()',
                      'Native CEF area unavailable')
        client.mouse(*point);keys(device,'ctrl','shift','a')
        js_wait(client,'!!document.querySelector('+json.dumps(search)+')','Physical CEF Ctrl+Shift+A did not open search')
        keys(device,'esc')
        js_wait(client,'!document.querySelector('+json.dumps(search)+')','Escape did not close CEF-opened search')
        report['checks'].append('physical_cef_search_shortcut_and_private_ordinary_exclusion')
        # Select the SECOND row of a cold recovered window via real search.
        # The first row was active on disk and must never make a request.
        def cold_entry():
            return next((r for r in call(client,'tab_search_snapshot')
                         if r['window']!='main' and r['address']==base+'/cold-secondary-target'),None)
        entry=wait_for(cold_entry,'Cold recovered window absent from search snapshot',30)
        require(entry['cold'] and entry['tab_id']==902,'Recovered window unexpectedly attached before user selection')
        with count_lock:
            require(counts['/cold-secondary-old']==0 and counts['/cold-secondary-target']==0,
                    'Cold recovered window eagerly navigated before search')
        before=main_rows(client)['aktivnaya']
        foreground(client);keys(device,'ctrl','shift','a')
        js_wait(client,'!!document.querySelector('+json.dumps(search)+')','Search absent for cold secondary target')
        fill_search(client,'cold-secondary-target')
        js_wait(client,'document.querySelectorAll("#tab-search-results [role=option]").length===1','Cold-window result was not unique')
        keys(device,'enter')
        js_wait(client,'!document.querySelector('+json.dumps(search)+')','Cold-window native selection did not finish',40)
        extra_client=wait_for(lambda:connect_window(device,entry['window']),'Recovered secondary inspector unavailable',30)
        def cold_selected():
            session=call(extra_client,'obychnye_spisok',id='essentials-main')
            active=next((t for t in session['vkladki'] if t['id']==session['aktivnaya']),None)
            return active if active and active['adres']==base+'/cold-secondary-target' and active.get('browser_id',0)>0 and not active['zagruzka'] else None
        restored=wait_for(cold_selected,'Actual cold secondary target did not become selected/loaded',35)
        require(restored['id']!=902 and restored['route']=={'kind':'direct'},'Search did not remap the old ID with its exact route')
        time.sleep(.3)
        with count_lock:
            require(counts['/cold-secondary-target']>0 and counts['/cold-secondary-old']==0,
                    'Cold search loaded the previous active page before its intended target')
        require(main_rows(client)['aktivnaya']==before,'Cold secondary search changed main-window selection')
        report['checks'].append('cold_secondary_search_second_row_native_ID_remap_no_previous_active_HTTP')
        # The same extra window is now warm and reuses this one Super engine.
        extra=call(extra_client,'obychnye_spisok',id='essentials-main')
        call(extra_client,'obychnaya_komanda',id='essentials-main',komanda={'deystvie':'pereyti','id':extra['aktivnaya'],'adres':base+'/window-target'})
        wait_for(lambda:any(r['window']==entry['window'] and r['address']==base+'/window-target' for r in call(client,'tab_search_snapshot')),
                 'Actual extra-window navigation absent from global search')
        foreground(client);keys(device,'ctrl','shift','a')
        js_wait(client,'!!document.querySelector('+json.dumps(search)+')','Main search absent for cross-window target')
        fill_search(client,'window-target')
        js_wait(client,'document.querySelectorAll("#tab-search-results [role=option]").length===1','Cross-window result was not unique')
        keys(device,'enter')
        js_wait(client,'!document.querySelector('+json.dumps(search)+')','Cross-window activation did not finish')
        require(call(extra_client,'plugin:window|is_focused',label=entry['window']),'Search did not focus the actual target window')
        require(main_rows(client)['aktivnaya']==before,'Cross-window search selected a different main-window tab')
        extra=call(extra_client,'obychnye_spisok',id='essentials-main')
        require(next(t for t in extra['vkladki'] if t['id']==extra['aktivnaya'])['adres']==base+'/window-target','Wrong extra-window tab activated')
        report['checks'].append('search_targets_actual_other_window_without_changing_main_selection')
        report['coverage']['search']='passed'
        # This scenario requires CEF keyboard focus, not an address-input focus.
        # Raise the owning shell and prove a physical click reached its real page
        # by a trusted-pointer HTTP witness before sending the CEF shortcut.
        call(client,'fokus_obolochke')
        call(client,'pokazat_dvizhki',vidno=True)
        point=js_wait(client,'(()=>{const r=document.querySelector(".pole")?.getBoundingClientRect();return r&&r.width>300&&r.height>200?[r.x+200,r.y+150]:null})()',
                      'Native CEF area unavailable for task-manager shortcut')
        pointer_before=counts['/physical-pointer/lazy-target']
        client.mouse(*point)
        wait_for(lambda:counts['/physical-pointer/lazy-target']>pointer_before,
                 'Physical task-manager click did not reach the owning main CEF page',15)
        report['task_cef_trusted_pointer_http_witness']=True
        keys(device,'shift','esc')
        task='[role="dialog"][aria-label="Диспетчер задач NexusBro"]'
        js_wait(client,'!!document.querySelector('+json.dumps(task)+')&&!!document.querySelector('+json.dumps(task+' tbody tr')+')',
                'Physical CEF Shift+Escape did not open measured task manager')
        js_wait(client,'document.hasFocus()&&document.activeElement?.getAttribute("aria-label")==="Закрыть диспетчер задач"','CEF-opened task manager did not own keyboard focus')
        task_layout=client.js('(()=>{const t=document.querySelector(".browser-task-table"),p=t?.parentElement;if(!t||!p)return null;const r=p.getBoundingClientRect();return {width:document.querySelector(".browser-tasks").getBoundingClientRect().width,overflow:p.scrollWidth>p.clientWidth+1,cells:[...t.querySelectorAll("th,td")].every(c=>{const q=c.getBoundingClientRect();return q.left>=r.left-1&&q.right<=r.right+1;})};})()')
        require(task_layout and not task_layout['overflow'] and task_layout['cells'],'Native task table clipped a column or required horizontal scrolling')
        report['task_layout']=task_layout
        image(device,out/'native-task-manager.png')
        measured=call(client,'browser_tasks_snapshot')
        samples=measured['totals']['cpu_samples'];pids={s['pid'] for s in samples}
        require(len(pids)==len(samples) and measured['totals']['processes']==len(samples),'Process snapshot duplicates ownership')
        require(unrelated.pid not in pids,'Unrelated sibling process entered browser task metrics')
        wrapper=psutil.Process(device.app.pid)
        permitted={wrapper.pid}|{p.pid for p in wrapper.children(recursive=True)}
        require(pids and pids<=permitted,'Task metrics include a process outside the launched app tree')
        roots=[]
        for pid in pids:
            try:
                process=psutil.Process(pid)
                if process.ppid() not in pids: roots.append(process)
            except psutil.NoSuchProcess:
                pass
        require(len(roots)==1 and pathlib.Path(roots[0].exe()).resolve()==exe,'Measured process root is not the actual app binary')
        shell=roots[0];before_os=shell.memory_info().rss
        measured=call(client,'browser_tasks_snapshot')
        after_os=shell.memory_info().rss
        own=next(p for p in measured['totals']['cpu_samples'] if p['pid']==shell.pid)
        require(isinstance(own['working_set_bytes'],int) and own['working_set_bytes']>0,'Native own-process working set unavailable')
        require(min(before_os,after_os)-8*1048576<=own['working_set_bytes']<=max(before_os,after_os)+8*1048576,
                'Actual OS RSS differs materially from the backend working-set measurement')
        require(all(s['identity'].startswith(str(s['pid'])+':') and s['cpu_time_ms']>=0 for s in measured['totals']['cpu_samples']),
                'Native CPU counters lack process creation identities')
        groups=measured['groups']
        require(any(g['super_id']=='essentials-main' and g['processes']>0 for g in groups),'Live Super process group absent')
        require(sum(g['processes'] for g in groups)==measured['totals']['processes'],'Group process counts double count shared processes')
        require(measured['scope']=='verified_descendants_of_this_instance','Process snapshot did not enforce instance ancestry')
        time.sleep(2.2)
        js_wait(client,'Array.from(document.querySelectorAll('+json.dumps(task+' tbody td')+')).some(e=>e.textContent.includes("%"))',
                'Second real task-manager sample did not display CPU deltas')
        report['native_metrics']={'owned_processes':measured['totals']['processes'],
            'working_set_bytes':measured['totals']['working_set_bytes'],'private_bytes':measured['totals']['private_bytes'],
            'complete':measured['complete'],'unrelated_sibling_excluded':True,'root_rss_bracket_matches':True}
        keys(device,'esc');js_wait(client,'!document.querySelector('+json.dumps(task)+')','Escape did not close task manager')
        report['checks'].append('physical_task_shortcut_real_owned_OS_metrics_other_process_excluded')
        report['coverage']['tasks']='passed'
        # Observe real backend events; no application event or IPC response is replaced.
        foreground(client)
        client.js('window.__essentialsErrors=[];window.__essentialsListenReady=false;'
            'window.__essentialsHandler=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="navigation-error")window.__essentialsErrors.push(e.payload);});'
            'window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:window.__essentialsHandler}).then(id=>{window.__essentialsListener=id;window.__essentialsListenReady=true;});true')
        js_wait(client,'window.__essentialsListenReady','Real network event observer did not register')
        blocked=socket.socket();blocked.bind(('127.0.0.1',0))
        failed_url=f'http://127.0.0.1:{blocked.getsockname()[1]}/real-refused'
        active=main_rows(client)['aktivnaya']
        call(client,'obychnaya_komanda',id='essentials-main',komanda={'deystvie':'pereyti','id':active,'adres':failed_url})
        error=js_wait(client,'window.__essentialsErrors.find(e=>e.super_id==="essentials-main"&&e.data.id==='+str(active)+')',
                      'Real CEF failed connection did not emit a native network error',35)
        require(isinstance(error['data']['code'],int) and error['data']['code']<0,'Network witness was not a Chromium failure')
        banner=js_wait(client,'(()=>{const e=document.querySelector(".oshibka-navigacii[role=alert]");return e&&e.getBoundingClientRect().height>0?e.textContent:null})()',
                       'Real failed navigation did not display the network error banner')
        require('Путь: Прямое соединение' in banner and 'Путь не менялся' in banner,
                'Network failure lost the effective direct route or promised automatic fallback')
        current=next(t for t in main_rows(client)['vkladki'] if t['id']==active)
        require(current['route']=={'kind':'direct'} and current['adres']==failed_url,'Failed navigation changed route or logical target')
        network_layout=client.js('(()=>{const e=document.querySelector(".network-error"),b=e?.querySelector(".network-error-retry");if(!e||!b)return null;const r=e.getBoundingClientRect(),n=document.querySelector(".navigaciya").getBoundingClientRect(),p=document.querySelector(".pole").getBoundingClientRect(),q=b.getBoundingClientRect();return {outsideForm:!e.closest("form"),top:r.top,bottom:r.bottom,navBottom:n.bottom,pageTop:p.top,noHorizontalClip:e.scrollWidth<=e.clientWidth+1,retryType:b.type,retryLeft:q.left,retryRight:q.right,width:innerWidth,x:q.x+12,y:q.y+5,k:devicePixelRatio||1,rgb:getComputedStyle(b).backgroundColor.match(/\\d+/g).slice(0,3).map(Number)};})()')
        require(network_layout and network_layout['outsideForm'] and network_layout['top']>=network_layout['navBottom']-1 and network_layout['bottom']<=network_layout['pageTop']+1 and network_layout['noHorizontalClip'] and network_layout['retryType']=='button' and 0<=network_layout['retryLeft']<network_layout['retryRight']<=network_layout['width'],'Native error banner or Retry was clipped/inside the address form')
        # Actual screen pixels must show the accent Retry above native CEF, not merely DOM visibility.
        from PIL import ImageGrab
        origin=client.command('plugin:window|inner_position',label='main')
        point=(round(origin['x']+network_layout['x']*network_layout['k']),round(origin['y']+network_layout['y']*network_layout['k']))
        def retry_painted():
            screen=ImageGrab.grab() if sys.platform=='win32' else ImageGrab.grab(xdisplay=device.display)
            rgb=screen.convert('RGB').getpixel(point)
            return list(rgb) if max(abs(rgb[i]-network_layout['rgb'][i]) for i in range(3))<24 else None
        report['network_retry_painted_rgb']=wait_for(retry_painted,'Native CEF covered the visible Retry control',8)
        report['network_layout']=network_layout
        image(device,out/'native-real-network-error.png')
        retry_port=blocked.getsockname()[1];blocked.close();blocked=None
        recovered_server=http.server.ThreadingHTTPServer(('127.0.0.1',retry_port),Page)
        threading.Thread(target=recovered_server.serve_forever,daemon=True).start()
        # Physical Retry activates the same failed URL, now served by a real HTTP listener.
        client.js('(()=>{const e=Array.from(document.querySelectorAll(".oshibka-navigacii[role=alert] button")).find(e=>e.textContent==="Повторить");if(!e)return false;e.setAttribute("data-native-retry","true");return true})()')
        click_control(client,'[data-native-retry="true"]',15)
        loaded(client,failed_url)
        js_wait(client,'!document.querySelector(".oshibka-navigacii[role=alert]")','Recovered navigation retained stale network warning')
        with count_lock: require(counts['/real-refused']>0,'Retry did not reach the recovered real HTTP server')
        report['network_witness']={'native_code':error['data']['code'],'route_preserved':True,'same_url_retry_reached_real_server':True}
        report['checks'].append('real_connection_refusal_native_event_visible_route_and_physical_retry_recovery')
        report['coverage']['network_error']='passed'
        report['completed']=True
    except Exception as error:
        report['failure_type']=type(error).__name__
        report['failure']=str(error)[:1200]
        if device and device.app:
            report['app_exit_code_at_failure']=device.app.poll()
            try:
                process=psutil.Process(device.app.pid)
                report['owned_tree_alive_at_failure']=len(process.children(recursive=True))
            except psutil.NoSuchProcess:
                report['owned_tree_alive_at_failure']=0
        with count_lock:
            expected_paths=('/loaded','/lazy-target','/sleeping-super','/private-super','/private-ordinary','/cold-secondary-old','/cold-secondary-target','/window-target','/real-refused')
            report['failure_http_counts']={path:counts[path] for path in expected_paths}
            report['failure_other_http_requests']=sum(count for path,count in counts.items() if path not in expected_paths)
        failure_diagnostics(device,out,report)
        raise
    finally:
        write_json(out/'results.json',report)
        if extra_client:
            try: extra_client.close()
            except Exception: pass
        if device:
            for observed in getattr(device,'essentials_clients',{}).values():
                try: observed.close()
                except Exception: pass
            device.close()
            if report.get('failure_type'):
                copy_failure_logs(device,out,report)
                write_json(out/'results.json',report)
        if blocked: blocked.close()
        for fixture in (server,recovered_server):
            if fixture: fixture.shutdown();fixture.server_close()
        if unrelated:
            if unrelated.poll() is None: unrelated.terminate()
            unrelated.wait(timeout=5)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe',required=True)
    parser.add_argument('--cef',required=True)
    parser.add_argument('--out',required=True)
    run(parser.parse_args())


if __name__=='__main__':
    main()
