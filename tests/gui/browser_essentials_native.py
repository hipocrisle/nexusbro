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


def foreground(client):
    call(client,'fokus_obolochke')
    click_control(client,'input[aria-label="Адрес страницы"]',15)
    js_wait(client,'document.hasFocus()&&document.activeElement?.getAttribute("aria-label")==="Адрес страницы"',
            'Native shell address did not own physical keyboard focus')


def main_rows(client):
    return call(client,'obychnye_spisok',id='essentials-main')


def loaded(client, address, timeout=35):
    def ready():
        current=main_rows(client)
        row=next((row for row in current['vkladki'] if row['id']==current['aktivnaya']),None)
        return row if row and row.get('adres')==address and row.get('zagolovok')=='EssentialsNativeFixture' and not row.get('zagruzka') else None
    return wait_for(ready,'Actual CEF fixture did not load at the selected ordinary row',timeout)


def connect_window(device,label):
    if sys.platform!='win32':
        for index in range(8):
            candidate=None
            try:
                candidate=Client(display=device.display,port=device.inspector,page_index=index)
                if candidate.js('window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')==label:
                    return candidate
            except (OSError,ConnectionError,RuntimeError,websocket.WebSocketException):
                pass
            if candidate:
                candidate.close()
        return None
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(f'http://127.0.0.1:{device.inspector}/json/list',timeout=3) as response:
        pages=json.load(response)
    for page in pages:
        endpoint=urllib.parse.urlsplit(page.get('webSocketDebuggerUrl',''))
        if page.get('type')!='page' or not endpoint.scheme:
            continue
        require(endpoint.scheme=='ws' and endpoint.hostname in ('127.0.0.1','localhost')
                and endpoint.port==device.inspector and not endpoint.username and not endpoint.password,
                'Inspector target escaped the owned loopback endpoint')
        transport=socket.create_connection(('127.0.0.1',device.inspector),timeout=3)
        candidate=WindowsClient.__new__(WindowsClient)
        candidate.seq=0
        try:
            candidate.ws=websocket.create_connection(page['webSocketDebuggerUrl'],socket=transport,
                timeout=5,suppress_origin=True,http_proxy_host=None,http_no_proxy=['127.0.0.1','localhost'])
            if candidate.js('window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')==label:
                return candidate
            candidate.close()
        except (OSError,RuntimeError,websocket.WebSocketException):
            transport.close()
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
                body=b'<!doctype html><meta charset="utf-8"><title>EssentialsNativeFixture</title><style>html,body{margin:0;background:#185541;color:white;min-height:100vh}</style><main>Real native essentials fixture</main>'
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
        unrelated=subprocess.Popen([sys.executable,'-c','import time; time.sleep(180)'],
                                   stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        client=device.start()
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
        client.fill('input[aria-label="Найти вкладку или супервкладку"]','lazy-target')
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
        # One extra window reuses the same Super engine, not another app instance.
        before=main_rows(client)['aktivnaya']
        call(client,'novoe_okno',id='essentials-main',privatnaya=False)
        def extra_entry():
            return next((r for r in call(client,'tab_search_snapshot') if r['window']!='main' and r['super_id']=='essentials-main'),None)
        entry=wait_for(extra_entry,'New native window absent from search snapshot',30)
        extra_client=wait_for(lambda:connect_window(device,entry['window']),'Additional native shell inspector unavailable',30)
        extra=call(extra_client,'obychnye_spisok',id='essentials-main')
        call(extra_client,'obychnaya_komanda',id='essentials-main',komanda={'deystvie':'pereyti','id':extra['aktivnaya'],'adres':base+'/window-target'})
        wait_for(lambda:any(r['window']==entry['window'] and r['address']==base+'/window-target' for r in call(client,'tab_search_snapshot')),
                 'Actual extra-window navigation absent from global search')
        foreground(client);keys(device,'ctrl','shift','a')
        js_wait(client,'!!document.querySelector('+json.dumps(search)+')','Main search absent for cross-window target')
        client.fill('input[aria-label="Найти вкладку или супервкладку"]','window-target')
        js_wait(client,'document.querySelectorAll("#tab-search-results [role=option]").length===1','Cross-window result was not unique')
        keys(device,'enter')
        js_wait(client,'!document.querySelector('+json.dumps(search)+')','Cross-window activation did not finish')
        require(call(extra_client,'plugin:window|is_focused',label=entry['window']),'Search did not focus the actual target window')
        require(main_rows(client)['aktivnaya']==before,'Cross-window search selected a different main-window tab')
        extra=call(extra_client,'obychnye_spisok',id='essentials-main')
        require(next(t for t in extra['vkladki'] if t['id']==extra['aktivnaya'])['adres']==base+'/window-target','Wrong extra-window tab activated')
        report['checks'].append('search_targets_actual_other_window_without_changing_main_selection')
        report['coverage']['search']='passed'
        foreground(client);keys(device,'shift','esc')
        task='[role="dialog"][aria-label="Диспетчер задач NexusBro"]'
        js_wait(client,'!!document.querySelector('+json.dumps(task)+')&&!!document.querySelector('+json.dumps(task+' tbody tr')+')',
                'Physical Shift+Escape did not open measured task manager')
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
        require('Путь: Прямое соединение' in banner and 'Автоматической смены пути нет' in banner,
                'Network failure lost the effective direct route or promised automatic fallback')
        current=next(t for t in main_rows(client)['vkladki'] if t['id']==active)
        require(current['route']=={'kind':'direct'} and current['adres']==failed_url,'Failed navigation changed route or logical target')
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
        raise
    finally:
        write_json(out/'results.json',report)
        if extra_client:
            try: extra_client.close()
            except Exception: pass
        if device: device.close()
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
