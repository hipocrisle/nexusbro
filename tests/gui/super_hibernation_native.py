#!/usr/bin/env python3
"""Real product whole-engine sleep prerequisite on isolated GitHub desktops.

No lifecycle mutations, forged ACKs, fake IPC results, forced sleep or mocked
clock. Only normal native Super/tab/navigation actions and readonly snapshots.
"""
import argparse
import collections
import ctypes
import http.server
import io
import json
import os
import pathlib
import re
import secrets
import ssl
import struct
import subprocess
import sys
import threading
import time
import urllib.parse
import wave

import psutil
from resource_soak import foreground
from security_native import Device, call, js_wait, painted, require, super_record, wait_for
from tab_routes import WindowsDevice, click_control, switch_super, rows, painted_interaction_point
from route_fixture import Fixture as Routes


class Fixture:
    def __init__(self, cert, key):
        self.routes = Routes(cert, key)
        self.lock = threading.Lock(); self.receipts = []
        self.tokens = {lane: secrets.token_hex(20) for lane in ('A', 'B')}
        self.seeded = set()
        audio = io.BytesIO()
        with wave.open(audio, 'wb') as wav:
            wav.setnchannels(1); wav.setsampwidth(2); wav.setframerate(8000)
            wav.writeframes(b''.join(struct.pack('<h', int(3000 * __import__('math').sin(i / 30))) for i in range(16000)))
        sound = audio.getvalue(); owner = self

        class Origin(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self):
                path = urllib.parse.urlsplit(self.path); q = urllib.parse.parse_qs(path.query)
                case = q.get('case', [''])[0]
                with owner.routes.lock:
                    lane = owner.routes.peers.get(self.client_address[1], 'Direct')
                if path.path == '/audio.wav':
                    body, kind = sound, 'audio/wav'
                elif path.path == '/worker.js':
                    body = b"self.addEventListener('install',e=>self.skipWaiting());self.addEventListener('activate',e=>e.waitUntil(clients.claim()));"
                    kind = 'application/javascript'
                elif path.path == '/download':
                    self.send_response(200); self.send_header('Content-Disposition', 'attachment; filename=fixture.bin')
                    self.send_header('Content-Length', str(256 * 1024 * 1024)); self.end_headers()
                    with owner.lock: owner.receipts.append({'case':'download-started','lane':lane})
                    try:
                        for _ in range(1024): self.wfile.write(b'x' * 262144); self.wfile.flush(); time.sleep(.2)
                    except (BrokenPipeError, ConnectionResetError): pass
                    return
                elif path.path == '/proof':
                    cookie = self.headers.get('Cookie', '')
                    with owner.lock:
                        owner.receipts.append({'case':case,'lane':lane,'proof':True,
                            'auth':next((k for k,v in owner.tokens.items() if 'hb_session='+v in cookie and 'hb_persistent='+v in cookie),None),
                            'storage':q.get('storage')==['true'],'scroll':float(q.get('scroll',['0'])[0]),
                            'trusted':q.get('trusted')==['true'],'resource_ready':q.get('ready')==['true'],
                            'clock':float(q.get('clock',['0'])[0])})
                    body, kind = b'ok', 'text/plain'
                elif path.path == '/page' and re.fullmatch(r'[a-z0-9-]{1,40}',case):
                    token = owner.tokens.get(lane, '')
                    seed = q.get('seed') == ['1'] and (lane, case) not in owner.seeded
                    if seed: owner.seeded.add((lane,case))
                    with owner.lock: owner.receipts.append({'case':case,'lane':lane,'page':True})
                    storage = 'localStorage.setItem("hb",'+json.dumps(token)+');' if seed else ''
                    script = storage + '''const fixtureCase=CASE,expected=TOKEN;
function report(trusted=false,ready=false,clock=0){fetch('/proof?'+new URLSearchParams({case:fixtureCase,storage:localStorage.getItem('hb')===expected,scroll:scrollY,trusted,ready,clock}),{cache:'no-store'}).catch(()=>{});}
addEventListener('load',()=>report());addEventListener('scroll',e=>report(e.isTrusted));
'''.replace('CASE',json.dumps(case)).replace('TOKEN',json.dumps(token))
                    if case in ('audio','paused'):
                        script += "let resource=null;start.onclick=async e=>{if(!e.isTrusted)return;resource=new Audio('/audio.wav');resource.loop=true;resource.volume=.1;await resource.play();" + ("resource.pause();" if case=='paused' else '') + "report(true,true,resource.currentTime);setInterval(()=>report(true,true,resource.currentTime),1000);};"
                    elif case == 'draft':
                        script += "document.body.insertAdjacentHTML('beforeend','<input aria-label=\"Fixture draft\" style=\"position:fixed;top:90px;left:10px\">');addEventListener('beforeunload',e=>{e.preventDefault();e.returnValue='';});"
                    elif case == 'worker':
                        script += "navigator.serviceWorker.register('/worker.js').then(()=>navigator.serviceWorker.ready).then(()=>report(false,true));"
                    elif case == 'download':
                        script += "start.onclick=e=>{if(e.isTrusted)location.href='/download';};"
                    elif case == 'unspecified':
                        pass
                    body = ('<!doctype html><meta charset="utf-8"><style>body{margin:0;min-height:6000px;background:#185541}button{position:fixed;left:0;top:0;width:130px;height:70px;border:0;background:rgb(123,31,162);color:white}button span{position:absolute;left:5px;bottom:3px;font:12px sans-serif}</style>'+('<button id="start"><span>Start fixture</span></button>' if case in ('audio','paused','download') else '')+'<script>'+script+'</script>').encode()
                    kind = 'text/html'
                    self.send_response(200)
                    if seed:
                        self.send_header('Set-Cookie','hb_session='+token+'; Secure; HttpOnly; SameSite=Lax; Path=/')
                        self.send_header('Set-Cookie','hb_persistent='+token+'; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=86400')
                    if case == 'unspecified': self.send_header('Set-Cookie','hb_unsupported=fixture; Secure; HttpOnly; Path=/')
                    self.send_header('Content-Type',kind); self.send_header('Cache-Control','no-store'); self.send_header('Content-Length',str(len(body))); self.end_headers()
                    try:self.wfile.write(body)
                    except (BrokenPipeError,ConnectionResetError):pass
                    return
                else:
                    self.send_error(404);return
                self.send_response(200);self.send_header('Content-Type',kind);self.send_header('Content-Length',str(len(body)));self.send_header('Cache-Control','no-store');self.end_headers()
                try:self.wfile.write(body)
                except (BrokenPipeError,ConnectionResetError):pass
        self.server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Origin);self.server.daemon_threads=True
        tls=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);tls.load_cert_chain(cert,key);self.server.socket=tls.wrap_socket(self.server.socket,server_side=True)
        self.routes.ports.add(self.server.server_port)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.base='https://127.0.0.1:'+str(self.server.server_port)

    def records(self,case):
        with self.lock:return [dict(r) for r in self.receipts if r['case']==case]
    def url(self,case,seed=False):return self.base+'/page?'+urllib.parse.urlencode({'case':case,'seed':'1' if seed else '0'})
    def close(self):self.server.shutdown();self.server.server_close();self.thread.join(timeout=5);self.routes.close()


def snapshot(client):
    value=call(client,'super_lifecycle_snapshot')
    require(isinstance(value,dict) and value.get('schema')==1 and isinstance(value.get('engines'),list),'Native lifecycle schema invalid')
    result={}
    for row in value['engines']:
        require(isinstance(row,dict) and row.get('id') not in result and row.get('phase') in ('Sleeping','Starting','Running','Preparing','Stopping'),'Native lifecycle row invalid')
        require(all(type(row.get(k)) is int and row[k]>=0 for k in ('generation','attempt','tab_count','live_count')),'Native lifecycle counter invalid')
        require(all(type(row.get(k)) is bool for k in ('profile_reserved','lease_confirmed')),'Native lease fields invalid')
        require(row.get('pid') is None or type(row['pid']) is int and row['pid']>0,'Native lifecycle PID invalid')
        result[row['id']]=row
    return result


def identities(pid):
    root=psutil.Process(pid)
    return {(p.pid,p.create_time()) for p in [root]+root.children(recursive=True)}


def gone(owners):
    for pid,created in owners:
        try:
            p=psutil.Process(pid)
            if p.create_time()==created and p.is_running():return False
        except psutil.NoSuchProcess:pass
    return True


class WriterWatch:
    """Bounded real OS sampling; paths/argv remain solely in local memory."""
    def __init__(self,device,writer_pid):
        arguments=psutil.Process(writer_pid).cmdline()
        profile=next((a for a in arguments if a.startswith('--user-data-dir=')),None)
        require(profile is not None,'Native writer profile argument unavailable')
        self.profile=profile;self.parent=psutil.Process(foreground(device))
        self.stop=threading.Event();self.peak=0;self.samples=0;self.error=None
        self.thread=threading.Thread(target=self.run,daemon=True)
    def run(self):
        try:
            while not self.stop.is_set():
                count=0
                for process in self.parent.children(recursive=True):
                    try:
                        arguments=process.cmdline()
                        if '--nb-dvizhok' in arguments and self.profile in arguments:count+=1
                    except psutil.NoSuchProcess:pass
                self.peak=max(self.peak,count);self.samples+=1
                self.stop.wait(.02)
        except Exception as error:self.error=type(error).__name__
    def finish(self):
        self.stop.set();self.thread.join(timeout=5)
        require(not self.thread.is_alive() and self.error is None and self.samples>0,'Native profile writer observation incomplete')
        require(self.peak<=1,'Two OS writers held the same native profile during rapid wake')


def navigate(device,url):
    foreground(device);client=device.client
    selector='[aria-label="Адрес страницы"]'
    if sys.platform=='win32':
        import pyautogui
        click_control(client,selector,20);pyautogui.hotkey('ctrl','a');pyautogui.write(url,interval=.001);pyautogui.press('enter')
    else:client.fill(selector,url,enter=True)


def wheel(device):
    client=device.client
    p=js_wait(client,'(()=>{const r=document.querySelector(".pole")?.getBoundingClientRect();return r?[r.x+300,r.y+250]:null})()','Real CEF wheel point absent')
    label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label');origin=client.command('plugin:window|inner_position',label=label);scale=client.js('devicePixelRatio||1')
    x,y=round(origin['x']+p[0]*scale),round(origin['y']+p[1]*scale)
    if sys.platform=='win32':
        import pyautogui
        from ctypes import wintypes
        pyautogui.moveTo(x,y)
        event=ctypes.WinDLL('user32',use_last_error=True).mouse_event
        event.argtypes=[wintypes.DWORD,wintypes.DWORD,wintypes.DWORD,wintypes.DWORD,ctypes.c_size_t];event.restype=None
        event(0x0800,0,0,ctypes.c_uint32(-6*120).value,0)
    else:subprocess.run(['xdotool','mousemove',str(x),str(y),'click','--repeat','6','--delay','30','5'],env={**os.environ,'DISPLAY':device.display},check=True,timeout=5)


def run(args):
    require(os.environ.get('GITHUB_ACTIONS')=='true','Native fixture only runs on isolated GitHub runners')
    out=pathlib.Path(args.out).resolve();out.mkdir(parents=True,exist_ok=False)
    report={'schema':1,'completed':False,'product_hibernation_accepted':False,'platform':sys.platform,'test_idle_seconds':5,'normal_policy_minutes':30,'checks':[],'coverage':{}}
    fixture=device=None;stage='setup';code=1
    previous_idle=os.environ.get('NEXUSBRO_TEST_SUPER_IDLE_SECONDS');previous_download=os.environ.get('NEXUSBRO_TEST_DOWNLOAD_DIR')
    try:
        fixture=Fixture(args.tls_cert,args.tls_key)
        exe,cef=pathlib.Path(args.exe).resolve(),pathlib.Path(args.cef).resolve()
        root=out/'private-device';device=WindowsDevice(root,exe,cef) if sys.platform=='win32' else Device('hibernate',root,exe,cef)
        specs=[('hb-a','roundtrip','A',False),('hb-b','keeper','B',False),
               ('hb-private','private','A',True),('hb-multi','multi-a','A',False),('hb-pinned','pinned','A',False)]
        specs += [('hb-'+case,case,'A',False) for case in ('audio','paused','draft','worker','download','unspecified')]
        definitions=[]
        for index,(identifier,case,lane,private) in enumerate(specs):
            item=super_record(identifier,'Hibernate '+identifier,fixture.url(case,True),proxy='http://127.0.0.1:'+str(fixture.routes.proxies[lane]),private=private);item['poryadok']=index;definitions.append(item)
            folder=device.data/'profili'/identifier;folder.mkdir(parents=True)
            tabs=[{'id':1,'adres':fixture.url(case,True),'zagolovok':'Fixture','otlozhena':True}]
            if identifier=='hb-pinned':tabs[0]['zakreplena']=True
            if identifier=='hb-multi':tabs.append({'id':2,'adres':fixture.url('multi-b',True),'zagolovok':'Second context','otlozhena':True,'route':{'kind':'connection','id':'lane-b'}})
            (folder/'obychnye.json').write_text(json.dumps({'vkladki':tabs,'aktivnaya':1,'sleduyushchaya':len(tabs)+1}))
        (device.data/'vkladki.json').write_text(json.dumps(definitions))
        (device.data/'sohranennye.json').write_text(json.dumps([r for r in definitions if not r['privatnaya']]))
        (device.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False,'vyhody':[{'id':'lane-b','nazvanie':'Fixture B','adres':'http://127.0.0.1:'+str(fixture.routes.proxies['B']),'proxy_uzel':''}]}))
        download=root/'downloads';download.mkdir(parents=True)
        os.environ['NEXUSBRO_TEST_SUPER_IDLE_SECONDS']='5';os.environ['NEXUSBRO_TEST_DOWNLOAD_DIR']=str(download)
        client=device.start()
        policy=call(client,'nastrojki_poluchit');require(policy.get('memory_saver',True) is True and policy.get('memory_saver_minutes',30)==30,'Normal30min policy changed')
        def activate(identifier):
            foreground(device);switch_super(client,identifier)
            foreground(device)
            # Standard visible lazy-page button avoids a micro-width tab target.
            selector='.pusto button'
            if client.js('!![...document.querySelectorAll(".pusto button")].find(e=>e.textContent.trim()==="Загрузить вкладку")'):
                click_control(client,selector,20)
            wait_for(lambda:snapshot(client).get(identifier,{}).get('phase')=='Running','Native engine did not reach Running',40)
        def latest_proof(case):return [r for r in fixture.records(case) if r.get('proof')]
        def running(identifier):
            r=snapshot(client)[identifier]
            require(r['phase']=='Running' and r['pid'] is not None and r['profile_reserved'] and r['lease_confirmed'],'Native Running lease missing')
            return r
        def refuse(identifier,seconds=9):
            before=running(identifier);owners=identities(before['pid']);activate('hb-b')
            until=time.monotonic()+seconds
            while time.monotonic()<until:
                row=snapshot(client)[identifier]
                require(row['phase']!='Sleeping' and row['generation']==before['generation'] and row['pid']==before['pid'] and row['profile_reserved'] and row['lease_confirmed'],'Protected Super lost native lease')
                time.sleep(.1)
            require(not gone(owners),'Protected engine exited')
        stage='selected-native-super-protection'
        activate('hb-a');navigate(device,fixture.url('roundtrip'))
        wait_for(lambda:any(r.get('auth')=='A' and r.get('storage') for r in latest_proof('roundtrip')),'Initial server cookie/storage proof absent',30)
        before=running('hb-a');time.sleep(7);require(running('hb-a')['pid']==before['pid'],'Selected main Super slept')
        report['checks'].append('selected_main_native_super_remains_running_beyond_test_idle')
        stage='physical-wheel-before-whole-engine-sleep'
        wheel(device)
        scroll=wait_for(lambda:next((r for r in reversed(latest_proof('roundtrip')) if r['trusted'] and r['scroll']>=200),None),'Native trusted wheel receipt absent',20)['scroll']
        report['checks'].append('physical_native_wheel_without_page_click_sets_real_scroll')
        stage='whole-engine-exit-cookie-storage-scroll-roundtrip'
        for cycle in range(3):
            before=running('hb-a');owners=identities(before['pid'])
            activate('hb-b')
            asleep=wait_for(lambda:(lambda r:r if r['phase']=='Sleeping' else None)(snapshot(client)['hb-a']),'Ordinary hidden Super never slept',50)
            require(asleep['pid'] is None and not asleep['profile_reserved'] and not asleep['lease_confirmed'],'Sleeping retains native lease')
            wait_for(lambda:gone(owners),'Old native engine tree survived Sleeping',8)
            receipts_before=len(latest_proof('roundtrip'))
            activate('hb-a');after=running('hb-a')
            require(after['generation']>before['generation'] and (after['pid'],psutil.Process(after['pid']).create_time()) not in owners,'Wake did not create exactly new native generation')
            proofs=wait_for(lambda:latest_proof('roundtrip')[receipts_before:] or None,'Restored page never reached real origin',30)
            require(all(p['lane']=='A' and p['auth']=='A' and p['storage'] for p in proofs),'Whole-engine wake lost session HttpOnly auth/storage or route')
            wait_for(lambda:any(abs(p['scroll']-scroll)<=64 for p in latest_proof('roundtrip')[receipts_before:]),'Whole-engine wake lost physical scroll',20)
        report['checks'].append('three_actual_whole_engine_exit_new_generation_session_httponly_secure_persistent_headers_storage_scroll_roundtrips')
        stage='rapid-wake-before-shutdown-exit'
        before=running('hb-a');owners=identities(before['pid'])
        observer=WriterWatch(device,before['pid'])
        activate('hb-b')
        transition=None;until=time.monotonic()+20
        while time.monotonic()<until:
            state=snapshot(client)['hb-a']
            if state['phase'] in ('Preparing','Stopping'):transition=state;break
            require(state['phase']!='Sleeping','Preparing/stopping interval finished before observation')
            time.sleep(.02)
        require(transition is not None,'No actual preparing/stopping interval observed')
        observer.thread.start()
        try:
            activate('hb-a');after=running('hb-a')
            if after['generation']>before['generation']:require(gone(owners),'Rapid wake started replacement before old tree exit')
            else:require(after['pid']==before['pid'],'Cancelled prepare changed writer without generation')
        finally:observer.finish()
        report['rapid_wake_writer_observation']={'samples':observer.samples,'peak_same_profile_writers':observer.peak,'period_seconds':.02}
        report['checks'].append('physical_wake_during_observed_prepare_or_stopping_keeps_single_native_writer')
        stage='private-native-refusal'
        activate('hb-private');refuse('hb-private');report['checks'].append('private_super_native_profile_never_hibernates')
        stage='guarded-native-pages'
        for case in ('audio','paused','draft','worker','download','unspecified'):
            identifier='hb-'+case
            stage='native-refusal-'+case
            activate(identifier);navigate(device,fixture.url(case))
            wait_for(lambda:latest_proof(case),'Protected native page not loaded',30)
            if case in ('audio','paused','download'):
                foreground(device);client.mouse(*painted_interaction_point(client))
                if case=='download':wait_for(lambda:fixture.records('download-started'),'Actual download never started',20)
                else:wait_for(lambda:any(r['trusted'] and r['resource_ready'] for r in latest_proof(case)),'Actual native media resource not started',20)
            if case=='draft':
                point=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return [r.x+100,r.y+100]})()');client.mouse(*point)
                if sys.platform=='win32':
                    import pyautogui;pyautogui.write('synthetic-unsaved-draft')
                else:subprocess.run(['xdotool','type','--clearmodifiers','--','synthetic-unsaved-draft'],env={**os.environ,'DISPLAY':device.display},check=True)
            if case=='worker':wait_for(lambda:any(r['resource_ready'] for r in latest_proof(case)),'Actual service worker never became ready',25)
            refuse(identifier)
            report['checks'].append(case+'_actual_native_super_retains_profile_and_process_past_idle')
            # Close this saved fixture normally through its native UI after the
            # refusal assertion, so one case cannot protect a later case via a
            # surviving worker, download or beforeunload registration.
            old_owners=identities(running(identifier)['pid'])
            foreground(device);click_control(client,'.vkladka[data-super-id="'+identifier+'"] .krest',20)
            wait_for(lambda:identifier not in snapshot(client),'Normal fixture close did not remove profile owner',20)
            wait_for(lambda:gone(old_owners),'Normally closed fixture engine did not exit',15)
        stage='pinned-native-refusal'
        activate('hb-pinned');refuse('hb-pinned');report['checks'].append('pinned_ordinary_page_protects_whole_super')
        stage='multiple-effective-context-native-refusal'
        activate('hb-multi');foreground(device);click_control(client,'.obychnaya[data-id="2"]',20)
        wait_for(lambda:any(r.get('lane')=='B' for r in fixture.records('multi-b')),'Second effective route never reached B proxy',30)
        refuse('hb-multi');report['checks'].append('multiple_effective_route_contexts_refuse_whole_engine_sleep')
        stage='secondary-native-window-reference-refusal'
        activate('hb-a');call(client,'novoe_okno',id='hb-a',privatnaya=False)
        wait_for(lambda:any(r.get('window')!='main' for r in call(client,'tab_search_snapshot')),'Extra window did not register ordinary views',20)
        refuse('hb-a');report['checks'].append('real_extra_window_reference_protects_super_even_when_main_switches')
        stage='unavailable-configured-route-no-direct-fallback'
        activate('hb-b');navigate(device,fixture.url('keeper'))
        wait_for(lambda:any(r.get('auth')=='B' and r.get('storage') for r in latest_proof('keeper')),'B profile auth/storage proof absent',30)
        before_hits=len(fixture.records('keeper'));fixture.routes.stop_proxy('B')
        navigate(device,fixture.url('keeper')+'&network=unavailable')
        time.sleep(4)
        require(len(fixture.records('keeper'))==before_hits,'Dead configured proxy used Direct or other origin fallback')
        require(all(r.get('lane')!='Direct' for r in fixture.receipts),'A native origin request bypassed configured route')
        report['checks'].append('real_unavailable_configured_B_proxy_never_falls_back_to_Direct_or_other_profile')
        report['coverage']={'cookie_storage_scroll':'three_whole_native_process_replacements','rapid_wake_phase':transition['phase'],'private_media_draft_worker_download_unsupported_multicontext_extra_window':'native_refusal','atomic_overlap':'parent_lease_and_OS_exit_identity_checks_not_exhaustive_scheduler_trace'}
        report['completed']=True;report['product_hibernation_accepted']=True;code=0
    except Exception as error:
        report.update(failure_stage=stage,failure_class=type(error).__name__)
        if device and device.client:
            try:
                state=snapshot(device.client)
                report['failure_lifecycle_counts']=dict(collections.Counter(r['phase'] for r in state.values()))
                report['failure_live_profile_count']=sum(r['pid'] is not None for r in state.values())
            except Exception as diagnostic_error:report['diagnostic_failure_class']=type(diagnostic_error).__name__
    finally:
        for name,value in [('NEXUSBRO_TEST_SUPER_IDLE_SECONDS',previous_idle),('NEXUSBRO_TEST_DOWNLOAD_DIR',previous_download)]:
            if value is None:os.environ.pop(name,None)
            else:os.environ[name]=value
        if device:
            try:device.close()
            except Exception as error:report['cleanup_failure_class']=type(error).__name__;report['completed']=False;report['product_hibernation_accepted']=False;code=1
        if fixture:
            try:fixture.close()
            except Exception as error:report['fixture_cleanup_failure_class']=type(error).__name__;report['completed']=False;report['product_hibernation_accepted']=False;code=1
        (out/'results.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'completed':report['completed'],'product_hibernation_accepted':report['product_hibernation_accepted']}))
    return code


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('exe','cef','out','tls-cert','tls-key'):parser.add_argument('--'+name,required=True)
    sys.exit(run(parser.parse_args()))
