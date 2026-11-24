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
    parser=Focus();parser.feed((Path(__file__).resolve().parents[2]/'desktop/src-tauri/src/youtube_player/player.html').read_text(encoding='utf-8'))
    require(parser.order==['download-video','download-audio','download-cancel','sponsor','sponsor-link','ratings-link','play'],'Shipped player focus order changed')
    active=[name for name in parser.order if name not in ({'download-video','download-audio'} if busy else {'download-cancel'})]
    target='download-cancel' if busy else 'download-audio' if audio else 'download-video'
    return active.index('play')-active.index(target)


def focus_download(client,device,tab,target,diagnostics):
    navigation=diagnostics.setdefault('control_navigation',[])
    require(len(navigation)<16,'Native download focus diagnostic bound exceeded')
    receipt={'target':target,'observed_controls':[],'reached':False};navigation.append(receipt)
    for step in range(17):
        state=observe(client,SUPER,tab);active=state.get('active_control')
        require(type(active)is int and 0<=active<=11,'Native download focus feedback unavailable')
        receipt['observed_controls'].append(active)
        if active==target:receipt['reached']=True;return
        require(step<16,'Native download control focus bound exceeded')
        keys(device,'shift+Tab')


def start_chooser(client,device,diagnostics,*,audio=False):
    before=dialogs(device)
    # Painted native Play establishes genuine CEF focus; derive the complete
    # shipped native tab order, including footer links, before physical input.
    physical(client,device,'play',diagnostics)
    backtabs(audio=audio)  # Keep the shipped control-order contract checked.
    focus_download(client,device,rows(client,SUPER)['aktivnaya'],8 if audio else 7,diagnostics)
    keys(device,'enter' if sys.platform=='win32' else 'Return')
    appeared=wait_for(lambda:dialogs(device)-before,'Native SaveAs did not appear',15)
    require(len(appeared)==1,'Native SaveAs ambiguous')
    return next(iter(appeared))


def linux_modal_observation(device,original,candidate):
    """Bounded read-only X11 metadata; never return titles, XIDs or paths."""
    env={**os.environ,'DISPLAY':device.display}
    def properties(window):
        result=subprocess.run(['xprop','-id',str(window),'_NET_WM_WINDOW_TYPE','_NET_WM_STATE','WM_TRANSIENT_FOR'],env=env,capture_output=True,text=True,timeout=3)
        text=result.stdout if len(result.stdout)<=8192 else ''
        transient=None
        import re
        match=re.search(r'WM_TRANSIENT_FOR[^\n]*?(0x[0-9a-fA-F]+)',text)
        if match:transient=int(match[1],16)
        geometry=subprocess.run(['xdotool','getwindowgeometry','--shell',str(window)],env=env,capture_output=True,text=True,timeout=3)
        numbers={}
        for line in geometry.stdout.splitlines()[:8]:
            key,_,value=line.partition('=')
            if key in {'WIDTH','HEIGHT'} and value.isdecimal():numbers[key.lower()]=int(value)
        return {'properties_read':result.returncode==0,'type_dialog':'_NET_WM_WINDOW_TYPE_DIALOG' in text,
                'modal':'_NET_WM_STATE_MODAL' in text,'hidden':'_NET_WM_STATE_HIDDEN' in text,
                'transient_matches_original':transient==original,**numbers}
    focused=subprocess.run(['xdotool','getwindowfocus'],env=env,capture_output=True,text=True,timeout=3)
    focus=focused.stdout.strip()
    visible=dialogs(device)
    return {'owned_visible_count':len(visible),'original_visible':original in visible,
            'candidate_visible':candidate in visible,'focus_read':focused.returncode==0 and focus.isdecimal(),
            'focus_is_original':focus==str(original),'focus_is_candidate':focus==str(candidate),
            'original':properties(original),'candidate':properties(candidate)}


def choose(device,dialog,path=None,*,replace=False,diagnostics=None):
    # A replacement confirmation is a distinct native modal. Do not send its
    # acceptance key before it exists or to the underlying file chooser.
    before_confirmation=dialogs(device) if replace else set()
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
        confirmations=wait_for(lambda:dialogs(device)-before_confirmation,
                               'Native replacement confirmation did not appear',15)
        require(len(confirmations)==1,'Native replacement confirmation ambiguous')
        confirmation=next(iter(confirmations))
        if sys.platform=='win32':
            Desktop(backend='uia').window(handle=confirmation).set_focus()
        else:
            subprocess.run(['xdotool','windowfocus','--sync',str(confirmation)],
                           env={**os.environ,'DISPLAY':device.display},check=True,
                           stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)
        if sys.platform=='win32':
            buttons=[b for b in Desktop(backend='uia').window(handle=confirmation).descendants(control_type='Button') if b.window_text().replace('&','') in ('Yes','Replace')]
            require(len(buttons)==1,'Native replacement accept button ambiguous')
            buttons[0].click_input()
        else:
            # GTK's actual overwrite dialog exposes the _Replace mnemonic.
            if diagnostics is not None:
                try:diagnostics['replacement_before']=linux_modal_observation(device,dialog,confirmation)
                except Exception:diagnostics['replacement_before']={'observation_incomplete':True}
            keys(device,'alt+r')
            if diagnostics is not None:
                try:diagnostics['replacement_after']=linux_modal_observation(device,dialog,confirmation)
                except Exception:diagnostics['replacement_after']={'observation_incomplete':True}
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
    directory=Path(args.components).resolve();manifest=json.loads((directory/'components.json').read_text(encoding='utf-8'));record=manifest['components']['ffmpeg']
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
                with fixture.lock:native.append({'lane':getattr(handler,'connection_lane','Direct'),'ranged':bool(handler.headers.get('Range')),'track':track if track in ('video','audio') else 'unknown'})
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
        client.js('window.__playerObserved=[];window.__downloadObserved=[];window.__downloadTrace=[];window.__downloadReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{const t=e.payload?.data?.tip;if(t==="youtube-state")window.__playerObserved.push(e.payload);if(t==="youtube-download-state")window.__downloadObserved.push(e.payload);if(t==="youtube-download-trace"&&window.__downloadTrace.length<64)window.__downloadTrace.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__downloadReady=true);true')
        js_wait(client,'window.__downloadReady','Native event listener missing')
        switch_super(client,SUPER)
        # A saved about:blank tab is deferred until the visible Open action.
        # Exercise that native UI action before waiting for a real CEF browser.
        if client.js('!!document.querySelector(".pusto button")'):
            click_control(client,'.pusto button',20)
        session=wait_for(lambda:rows(client,SUPER) if any(t.get('browser_id',0)>0 and not t.get('zagruzka') for t in rows(client,SUPER)['vkladki']) else None,'Native browser missing',40);tab=session['aktivnaya']
        call(client,'obychnaya_komanda',id=SUPER,komanda={'deystvie':'youtube_fixture','id':tab})
        wait_for(lambda:(lambda s:s if s.get('configured') and not s.get('disposed') else None)(observe(client,SUPER,tab,admitting=True)),'Native player not configured',30)
        output=root/'downloads';output.mkdir();diagnostics={};report['player_diagnostics']=diagnostics
        report['stage']='save-dialog-cancel';clear_states(client);dialog=start_chooser(client,device,diagnostics);choose(device,dialog,diagnostics=diagnostics)
        terminal(client,tab,'cancelled',20)
        require(not list(output.iterdir()),'Save cancel created output')
        report['checks'].append({'name':'physical-save-cancel','no_output':True})
        report['stage']='download-success';clear_states(client);dialog=start_chooser(client,device,diagnostics);target=output/'complete.webm';choose(device,dialog,target,diagnostics=diagnostics)
        done=terminal(client,tab,'done');require(target.is_file() and target.stat().st_size==done.get('bytes'),'Native completed file missing')
        require(any(isinstance(s.get('progress'),dict) and s['progress'].get('phase') in ('downloading','muxing','verifying','publishing') and type(s['progress'].get('bytes')) is int and s['progress']['bytes']>0 for s in states(client,tab)),'Actual nonzero worker progress missing')
        require(native and all(r['lane']=='A' and r['ranged'] for r in native),'Native download route witness missing')
        report['checks'].append(dict(name='actual-dual-stream-download',**ffprobe(args,target),native_route_requests=len(native)))
        report['stage']='audio-only-download';clear_states(client)
        with fixture.lock:audio_start=len(native)
        audio_target=output/'audio-only.webm';dialog=start_chooser(client,device,diagnostics,audio=True);choose(device,dialog,audio_target,diagnostics=diagnostics)
        audio_done=terminal(client,tab,'done')
        require(audio_target.is_file() and audio_target.stat().st_size==audio_done.get('bytes'),'Native audio file missing')
        with fixture.lock:audio_requests=list(native[audio_start:])
        require(audio_requests and all(r['lane']=='A' and r['ranged'] and r['track']=='audio' for r in audio_requests),'Audio-only native route witness missing')
        report['checks'].append(dict(name='actual-audio-only-download',**ffprobe(args,audio_target,audio_only=True),native_route_requests=len(audio_requests)))
        for attempt in range(2):
            report['stage']='cancel-'+str(attempt);clear_states(client)
            policy=fixture.stall_policy('A','video');policy['gate'].clear()
            dialog=start_chooser(client,device,diagnostics);cancelled=output/('cancel-'+str(attempt)+'.webm');choose(device,dialog,cancelled,diagnostics=diagnostics)
            wait_for(lambda:any(isinstance(s.get('progress'),dict) for s in states(client,tab)),'Native cancellation progress missing',15)
            physical(client,device,'play',diagnostics);backtabs(busy=True)
            focus_download(client,device,tab,9,diagnostics);keys(device,'space')
            terminal(client,tab,'cancelled',20);fixture.retire_stall('A','video',policy)
            require(not cancelled.exists() and clean_output(output),'Cancellation left files')
            report['checks'].append({'name':'physical-cancel-'+str(attempt),'bounded_terminal':True,'no_output':True})
        report['stage']='no-overwrite';immutable=output/'existing.webm';immutable.write_bytes(b'owned immutable fixture');before=immutable.read_bytes();clear_states(client)
        dialog=start_chooser(client,device,diagnostics);choose(device,dialog,immutable,replace=True,diagnostics=diagnostics);refused=terminal(client,tab,'error',20)
        require(refused.get('error')=='Файл назначения уже существует','Overwrite refusal not specific')
        require(immutable.read_bytes()==before and clean_output(output),'Existing target changed')
        report['checks'].append({'name':'no-overwrite','unchanged':True})
        report['stage']='route-error';direct_before=fixture.direct;fixture.fail_proxies();clear_states(client)
        failed=output/'failed.webm';dialog=start_chooser(client,device,diagnostics);choose(device,dialog,failed,diagnostics=diagnostics);terminal(client,tab,'error',60)
        require(not failed.exists() and clean_output(output),'Failed download left files')
        require(fixture.direct==direct_before==0,'Native download attempted a direct route fallback')
        report['checks'].append({'name':'bounded-route-error','no_output':True,'direct_connections':fixture.direct})
        owned=[psutil.Process(pid) for pid in owned_pids(device)]
        device.stop_app()
        wait_for(lambda:all(not process.is_running() for process in owned),'Owned processes remain',15)
        require(clean_output(output),'Temporary leftovers after owned exit')
        report['completed']=True;report['stage']='complete';code=0
    except Exception as error:
        report['failure_class']=type(error).__name__
        report['failure_code']={
            'Native replacement confirmation did not appear':'replacement_confirmation_missing',
            'Native replacement confirmation ambiguous':'replacement_confirmation_ambiguous',
            'Native replacement accept button ambiguous':'replacement_accept_ambiguous',
            'Native SaveAs did not close':'save_dialog_did_not_close',
            'Native SaveAs did not appear':'save_dialog_did_not_appear',
            'Native SaveAs ambiguous':'save_dialog_ambiguous',
        }.get(str(error),'unclassified')
        if device is not None and 'client' in locals() and 'tab' in locals():
            try:
                trace=client.js('window.__downloadTrace??[]') or []
                stages={'marker_observed','marker_admitted','request_received','request_admitted','parent_received','parent_admitted','dialog_dispatch','dialog_returned','target_received','worker_started'}
                reasons={'none','media_not_admitted','not_selected','locked_or_closing','route_missing','format_unsupported','job_limit','invalid_request','current_engine_mismatch','selected_session_mismatch','owner_missing','cancelled','picked','channel_missing','channel_sent','random_unavailable','state_unavailable','worker_spawn_failed'}
                report['download_pipeline']=[{'stage':event['data']['stage'],'reason':event['data']['reason']} for event in trace[-32:] if isinstance(event,dict) and event.get('super_id')==SUPER and isinstance(event.get('data'),dict) and event['data'].get('id')==tab and event['data'].get('stage') in stages and event['data'].get('reason') in reasons]
                recent=states(client,tab)[-8:]
                report['native_download_receipts']=[{name:state.get(name) is True for name in ('cancelled','done','choosing')}|{'has_error':bool(state.get('error')),'has_progress':isinstance(state.get('progress'),dict)} for state in recent]
                if sys.platform=='win32':
                    import ctypes
                    from ctypes import wintypes
                    from pywinauto import Desktop
                    from youtube_player_native import native_owner
                    root_handle=native_owner(device).handle;owned=owned_pids(device)
                    user=ctypes.WinDLL('user32',use_last_error=True)
                    user.GetWindow.argtypes=[wintypes.HWND,wintypes.UINT];user.GetWindow.restype=wintypes.HWND
                    counts={'owned_process':0,'owned_window_chain':0,'other':0}
                    for window in Desktop(backend='uia').windows():
                        if window.class_name()!='#32770' or not window.is_visible():continue
                        cursor=window.handle;seen=set();owner_match=False
                        for depth in range(8):
                            if not cursor or cursor in seen:break
                            if cursor==root_handle:owner_match=True;break
                            seen.add(cursor);cursor=user.GetWindow(cursor,4)
                        counts['owned_process' if window.process_id() in owned else 'owned_window_chain' if owner_match else 'other']+=1
                    report['native_dialog_counts']=counts
            except Exception:
                report['native_download_diagnostic_unavailable']=True
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
