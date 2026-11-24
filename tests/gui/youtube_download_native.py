#!/usr/bin/env python3
"""GHA-only REAL CEF button -> native SaveAs -> route-bound local WebM download.
No direct download IPC/path injection, JS click, renderer fetch or fallback.
Output contains only fixed stages/numeric receipts; disposable media stays local.
Native source preparation is not itself a passing native acceptance run.
"""
import argparse
import hashlib
from html.parser import HTMLParser
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.parse

import psutil
from security_native import Device, call, js_wait, require, super_record, wait_for
from tab_routes import WindowsDevice, switch_super, rows, click_control
from youtube_player_native import Fixture, Trust, certificates, observe, physical, foreground
from hibernation_cookie_probe import tls_proxy_preflight

SUPER='download-a'


def keys(device,*sequence):
    if sys.platform=='win32':
        import pyautogui as gui
        for key in sequence:
            if '+' in key:gui.hotkey(*key.lower().split('+'))
            else:gui.press(key.lower())
    else:
        for key in sequence:subprocess.run(['xdotool','key','--clearmodifiers',key],env={**os.environ,'DISPLAY':device.display},check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)


def owned_pids(device):
    require(device.app is not None and device.app.poll() is None,'Owned application not alive')
    root=psutil.Process(device.app.pid)
    return {root.pid,*[p.pid for p in root.children(recursive=True)]}


def dialogs(device):
    owned=owned_pids(device)
    if sys.platform=='win32':
        from pywinauto import Desktop
        return {w.handle for w in Desktop(backend='uia').windows() if w.process_id() in owned and w.class_name()=='#32770' and w.is_visible()}
    found=set()
    for pid in owned:
        result=subprocess.run(['xdotool','search','--onlyvisible','--pid',str(pid)],env={**os.environ,'DISPLAY':device.display},capture_output=True,text=True,timeout=4)
        if result.returncode==0:found.update(int(x) for x in result.stdout.splitlines() if x.isdecimal())
    return found


def backtabs(*,busy=False,audio=False):
    # Offline assertion against the SHIPPED HTML, including both footer links.
    # Actual key delivery remains physical; no CEF DOM actions are injected.
    class Focus(HTMLParser):
        def __init__(self):super().__init__();self.order=[]
        def handle_starttag(self,tag,attrs):
            attrs=dict(attrs);identifier=attrs.get('id')
            if identifier in {'download-video','download-audio','download-cancel','sponsor','play'}:self.order.append(identifier)
            elif tag=='a' and attrs.get('href') in ('https://sponsor.ajay.app/','https://returnyoutubedislike.com/'):
                self.order.append('sponsor-link' if 'sponsor.ajay' in attrs['href'] else 'ratings-link')
    parser=Focus();parser.feed((Path(__file__).resolve().parents[2]/'desktop/src-tauri/src/youtube_player/player.html').read_text())
    require(parser.order==['download-video','download-audio','download-cancel','sponsor','sponsor-link','ratings-link','play'],'Shipped player focus order changed')
    active=[name for name in parser.order if name not in ({'download-video','download-audio'} if busy else {'download-cancel'})]
    target='download-cancel' if busy else 'download-audio' if audio else 'download-video'
    return active.index('play')-active.index(target)


def start_chooser(client,device,diagnostics,*,audio=False):
    before=dialogs(device)
    # Painted native Play establishes genuine CEF focus; derive the complete
    # shipped native tab order, including footer links, before physical input.
    physical(client,device,'play',diagnostics)
    keys(device,*(['shift+Tab']*backtabs(audio=audio)), 'space')
    appeared=wait_for(lambda:dialogs(device)-before,'Native SaveAs did not appear',15)
    require(len(appeared)==1,'Native SaveAs ambiguous')
    return next(iter(appeared))


def choose(device,dialog,path=None,*,replace=False):
    if sys.platform=='win32':
        from pywinauto import Desktop
        window=Desktop(backend='uia').window(handle=dialog);window.set_focus()
        if path is None:keys(device,'escape')
        else:
            edits=window.descendants(control_type='Edit')
            target=next((e for e in edits if e.element_info.automation_id=='1001'),None)
            require(target is not None,'Native SaveAs filename edit missing')
            target.click_input();keys(device,'ctrl+a')
            import pyautogui as gui
            gui.write(str(path),interval=.002);keys(device,'enter')
    else:
        env={**os.environ,'DISPLAY':device.display}
        subprocess.run(['xdotool','windowfocus','--sync',str(dialog)],env=env,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)
        if path is None:keys(device,'Escape')
        else:
            keys(device,'ctrl+l','ctrl+a')
            subprocess.run(['xdotool','type','--clearmodifiers','--delay','2','--',str(path)],env=env,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10)
            keys(device,'Return')
            time.sleep(.4)
            if dialog in dialogs(device):keys(device,'alt+s')
    if replace:
        # Explicit physical confirmation is part of this synthetic user's choice;
        # backend must still refuse replacing any already-existing target.
        time.sleep(.5);keys(device,'enter' if sys.platform=='win32' else 'Return')
    wait_for(lambda:dialog not in dialogs(device),'Native SaveAs did not close',15)


def states(client,tab):
    values=client.js('window.__downloadObserved') or []
    return [v['data']['state'] for v in values if v.get('super_id')==SUPER and v.get('data',{}).get('id')==tab and isinstance(v.get('data',{}).get('state'),dict)]


def terminal(client,tab,kind,timeout=65):
    def result():
        values=states(client,tab)
        for state in reversed(values):
            if state.get('done') or state.get('cancelled') or 'error' in state:
                require(state.get(kind) is True if kind!='error' else isinstance(state.get('error'),str),'Wrong native download terminal state')
                return state
    return wait_for(result,'Native download terminal missing',timeout)


def clear_states(client):client.js('window.__downloadObserved=[];true')


def ffprobe(args,output,*,audio_only=False):
    directory=Path(args.components).resolve();manifest=json.loads((directory/'components.json').read_text());record=manifest['components']['ffmpeg']
    for item in record['files']:
        relative=Path(item['filename']);require(not relative.is_absolute() and '..' not in relative.parts,'Component path invalid')
        path=directory/relative
        require(all(not ancestor.is_symlink() for ancestor in (path,*path.parents) if ancestor!=directory.parent),'Component symlink')
        require(type(item['bytes']) is int and 0<item['bytes']<=256*1024*1024,'Component byte bound')
        require(not path.is_symlink() and path.is_file() and path.stat().st_size==item['bytes'] and hashlib.sha256(path.read_bytes()).hexdigest()==item['sha256'],'Component hash changed')
    binary=directory/record['ffprobe_filename']
    env={k:os.environ[k] for k in ('SystemRoot','WINDIR','TEMP','TMP') if k in os.environ}
    env['PATH' if sys.platform=='win32' else 'LD_LIBRARY_PATH']=str(binary.parent if sys.platform=='win32' else directory/'ffmpeg'/'lib')
    result=subprocess.run([str(binary),'-v','error','-protocol_whitelist','file,pipe','-f','matroska','-show_entries','stream=codec_name,codec_type:format=duration','-of','json',str(output)],env=env,stdin=subprocess.DEVNULL,capture_output=True,timeout=15)
    require(result.returncode==0 and len(result.stdout)+len(result.stderr)<=65536,'Native ffprobe failed')
    data=json.loads(result.stdout);require(sorted((s['codec_type'],s['codec_name']) for s in data['streams'])==([('audio','opus')] if audio_only else [('audio','opus'),('video','vp9')]),'Output stream mismatch')
    duration=float(data['format']['duration']);require(math.isfinite(duration) and 118<=duration<=122,'Output duration mismatch')
    return {'streams':1 if audio_only else 2,'vp9':not audio_only,'opus':True,'duration':round(duration,3),'actual_ffprobe':True}


def clean_output(directory):
    return not any(p.name.startswith('.nexusbro-download-') or p.name.endswith('.part') for p in directory.iterdir())


def run(args):
    require(os.environ.get('GITHUB_ACTIONS')=='true','Disposable GHA runner required')
    out=Path(args.out).resolve();out.mkdir(parents=True,exist_ok=False)
    root=Path(tempfile.mkdtemp(prefix='nb-player-'));device=fixture=trust=None
    report={'schema':1,'completed':False,'stage':'setup','checks':[],'extractor_acceptance':False}
    names=('NEXUSBRO_TEST_YOUTUBE_DNS_GUARD','NEXUSBRO_TEST_YOUTUBE_DOWNLOAD_CA','NEXUSBRO_TEST_YOUTUBE_DOWNLOAD_CA_SHA256');old={k:os.environ.get(k) for k in names}
    code=1
    try:
        thumb=certificates(root/'tls');trust=Trust(root,root/'tls'/'ca.crt',thumb);trust.install()
        fixture=Fixture(root/'tls'/'server.crt',root/'tls'/'server.key',Path(args.video_webm),Path(args.audio_webm))
        # Instrument this owned HTTP fixture, not production code: distinguish
        # the fixture's anonymous native downloader from Chromium's UA requests.
        native=[];origin=fixture.servers[0].RequestHandlerClass;original=origin.do_GET
        def instrument(handler):
            is_native=not handler.headers.get('User-Agent')
            if is_native:
                track=urllib.parse.parse_qs(urllib.parse.urlsplit(handler.path).query).get('track',[''])[0]
                with fixture.lock:native.append({'lane':fixture.peers.get(handler.client_address[1],'Direct'),'ranged':bool(handler.headers.get('Range')),'track':track if track in ('video','audio') else 'unknown'})
            writer=handler.wfile
            class PacedWriter:
                # This owned HTTP origin paces genuine response bytes so at
                # least one nonzero native worker progress sample is observable.
                def write(self,data):
                    if is_native and len(data)>65536:
                        writer.write(data[:65536]);writer.flush()
                        fixture.closing.wait(.6)
                        writer.write(data[65536:]);return len(data)
                    return writer.write(data)
                def __getattr__(self,name):return getattr(writer,name)
            handler.wfile=PacedWriter()
            try:return original(handler)
            finally:handler.wfile=writer
        origin.do_GET=instrument
        report['tls_preflight']=tls_proxy_preflight(fixture.proxies['A'],'fixture.googlevideo.com',443,root/'tls'/'ca.crt',root/'tls'/'server.crt')
        os.environ[names[0]]='1';os.environ[names[1]]=str(root/'tls'/'ca.crt');os.environ[names[2]]=hashlib.sha256((root/'tls'/'ca.crt').read_bytes()).hexdigest()
        exe,cef=Path(args.exe).resolve(),Path(args.cef).resolve();device=WindowsDevice(root/'device',exe,cef) if sys.platform=='win32' else Device('download',root/'device',exe,cef)
        item=super_record(SUPER,'Download fixture','about:blank',proxy='http://127.0.0.1:'+str(fixture.proxies['A']))
        folder=device.data/'profili'/SUPER;folder.mkdir(parents=True)
        (folder/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':'about:blank','zagolovok':'Fixture','otlozhena':True}],'aktivnaya':1,'sleduyushchaya':2}))
        for filename in ('vkladki.json','sohranennye.json'):(device.data/filename).write_text(json.dumps([item]))
        (device.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False}))
        client=device.start();foreground(device)
        client.js('window.__playerObserved=[];window.__downloadObserved=[];window.__downloadReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{const t=e.payload?.data?.tip;if(t==="youtube-state")window.__playerObserved.push(e.payload);if(t==="youtube-download-state")window.__downloadObserved.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__downloadReady=true);true')
        js_wait(client,'window.__downloadReady','Native event listener missing')
        switch_super(client,SUPER)
        # A saved about:blank tab is deferred until the visible Open action.
        # Exercise that native UI action before waiting for a real CEF browser.
        if client.js('!!document.querySelector(".pusto button")'):
            click_control(client,'.pusto button',20)
        session=wait_for(lambda:rows(client,SUPER) if any(t.get('browser_id',0)>0 and not t.get('zagruzka') for t in rows(client,SUPER)['vkladki']) else None,'Native browser missing',40);tab=session['aktivnaya']
        call(client,'obychnaya_komanda',id=SUPER,komanda={'deystvie':'youtube_fixture','id':tab})
        wait_for(lambda:(lambda s:s if s.get('configured') and not s.get('disposed') else None)(observe(client,SUPER,tab,admitting=True)),'Native player not configured',30)
        output=root/'downloads';output.mkdir();diagnostics={}
        report['stage']='save-dialog-cancel';clear_states(client);dialog=start_chooser(client,device,diagnostics);choose(device,dialog)
        terminal(client,tab,'cancelled',20)
        require(not list(output.iterdir()),'Save cancel created output')
        report['checks'].append({'name':'physical-save-cancel','no_output':True})
        report['stage']='download-success';clear_states(client);dialog=start_chooser(client,device,diagnostics);target=output/'complete.webm';choose(device,dialog,target)
        done=terminal(client,tab,'done');require(target.is_file() and target.stat().st_size==done.get('bytes'),'Native completed file missing')
        require(any(isinstance(s.get('progress'),dict) and s['progress'].get('phase') in ('downloading','muxing','verifying','publishing') and type(s['progress'].get('bytes')) is int and s['progress']['bytes']>0 for s in states(client,tab)),'Actual nonzero worker progress missing')
        require(native and all(r['lane']=='A' and r['ranged'] for r in native),'Native download route witness missing')
        report['checks'].append(dict(name='actual-dual-stream-download',**ffprobe(args,target),native_route_requests=len(native)))
        report['stage']='audio-only-download';clear_states(client)
        with fixture.lock:audio_start=len(native)
        audio_target=output/'audio-only.webm';dialog=start_chooser(client,device,diagnostics,audio=True);choose(device,dialog,audio_target)
        audio_done=terminal(client,tab,'done')
        require(audio_target.is_file() and audio_target.stat().st_size==audio_done.get('bytes'),'Native audio file missing')
        with fixture.lock:audio_requests=list(native[audio_start:])
        require(audio_requests and all(r['lane']=='A' and r['ranged'] and r['track']=='audio' for r in audio_requests),'Audio-only native route witness missing')
        report['checks'].append(dict(name='actual-audio-only-download',**ffprobe(args,audio_target,audio_only=True),native_route_requests=len(audio_requests)))
        for attempt in range(2):
            report['stage']='cancel-'+str(attempt);clear_states(client)
            policy=fixture.stall_policy('A','video');policy['gate'].clear()
            dialog=start_chooser(client,device,diagnostics);cancelled=output/('cancel-'+str(attempt)+'.webm');choose(device,dialog,cancelled)
            wait_for(lambda:any(isinstance(s.get('progress'),dict) for s in states(client,tab)),'Native cancellation progress missing',15)
            physical(client,device,'play',diagnostics);keys(device,*(['shift+Tab']*backtabs(busy=True)),'space')
            terminal(client,tab,'cancelled',20);fixture.retire_stall('A','video',policy)
            require(not cancelled.exists() and clean_output(output),'Cancellation left files')
            report['checks'].append({'name':'physical-cancel-'+str(attempt),'bounded_terminal':True,'no_output':True})
        report['stage']='no-overwrite';immutable=output/'existing.webm';immutable.write_bytes(b'owned immutable fixture');before=immutable.read_bytes();clear_states(client)
        dialog=start_chooser(client,device,diagnostics);choose(device,dialog,immutable,replace=True);refused=terminal(client,tab,'error',20)
        require(refused.get('error')=='Файл назначения уже существует','Overwrite refusal not specific')
        require(immutable.read_bytes()==before and clean_output(output),'Existing target changed')
        report['checks'].append({'name':'no-overwrite','unchanged':True})
        report['stage']='route-error';fixture.close();clear_states(client)
        failed=output/'failed.webm';dialog=start_chooser(client,device,diagnostics);choose(device,dialog,failed);terminal(client,tab,'error',60)
        require(not failed.exists() and clean_output(output),'Failed download left files')
        report['checks'].append({'name':'bounded-route-error','no_output':True})
        owned=[psutil.Process(pid) for pid in owned_pids(device)]
        device.stop_app()
        wait_for(lambda:all(not process.is_running() for process in owned),'Owned processes remain',15)
        require(clean_output(output),'Temporary leftovers after owned exit')
        report['completed']=True;report['stage']='complete';code=0
    except Exception as error:
        report['failure_class']=type(error).__name__
        trace=error.__traceback__
        while trace:
            if Path(trace.tb_frame.f_code.co_filename).name==Path(__file__).name:
                report['failure_location']={'function':trace.tb_frame.f_code.co_name,'line':trace.tb_lineno}
            trace=trace.tb_next
    finally:
        cleanup=True
        for obj in (device,fixture,trust):
            if obj:
                try:obj.close()
                except Exception:cleanup=False
        for name,value in old.items():
            if value is None:os.environ.pop(name,None)
            else:os.environ[name]=value
        try:shutil.rmtree(root)
        except OSError:cleanup=False
        report['cleanup_completed']=cleanup
        if not cleanup:code=1;report['completed']=False
        (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    return code


def self_test():
    import inspect
    inspect.signature(Fixture).bind(Path("cert"),Path("key"),Path("video"),Path("audio"))
    require(backtabs()==5 and backtabs(audio=True)==4 and backtabs(busy=True)==4,'Shipped physical focus sequence mismatch')
    class Client:
        def js(self,_):return [{'super_id':SUPER,'data':{'id':1,'state':{'done':True,'bytes':123}}}]
    require(terminal(Client(),1,'done')['bytes']==123,'Native receipt parsing mismatch')
    try:terminal(Client(),1,'cancelled')
    except Exception:pass
    else:raise AssertionError('wrong terminal accepted')
    with tempfile.TemporaryDirectory() as directory:
        path=Path(directory);require(clean_output(path),'Empty output not clean')
        (path/'.nexusbro-download-owned').mkdir();require(not clean_output(path),'Stage leftovers ignored')
    return {'self_test':True,'source_only':True,'native_execution':False}


if __name__=='__main__':
    if sys.argv[1:]==['--self-test']:
        print(json.dumps(self_test()));raise SystemExit(0)
    parser=argparse.ArgumentParser();parser.add_argument('--exe',required=True);parser.add_argument('--cef',required=True);parser.add_argument('--components',required=True);parser.add_argument('--video-webm',required=True);parser.add_argument('--audio-webm',required=True);parser.add_argument('--out',required=True)
    raise SystemExit(run(parser.parse_args()))
