#!/usr/bin/env python3
"""GHA real-input quality/captions/fullscreen acceptance; no page JS actions.
Requires feature-mode native fixture/snapshot and backward-compatible owned HTTP
fixture extensions. Emits only numeric/boolean receipts, no captions or URLs.
Prepared source is not a passing native acceptance result.
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

from security_native import Device, call, js_wait, require, super_record, wait_for
from tab_routes import WindowsDevice, switch_super, rows, click_control
from youtube_player_native import (HOST, Fixture, Trust, certificates, observe, physical,
                                  foreground, OwnedWindowManager, native_owner)
from hibernation_cookie_probe import tls_proxy_preflight

SUPER='features-a'
CAPTION=b'WEBVTT\n\n00:00.000 --> 02:00.000\n[NEXUSBRO CAPTION FIXTURE]\n'


SAFE_FIELDS={'configured','disposed','playing','separate','hidden','video_paused','audio_paused','position','audio_position','duration','skew','decoded','dropped','video_width','video_height','caption_selected','caption_cues','caption_active','caption_showing','fullscreen','active_control','quality_index','quality_count'}

def safe_state(value):
    return {k:v for k,v in value.items() if k in SAFE_FIELDS and (type(v)is bool or type(v)in(int,float) and math.isfinite(v))}


def input_observation(device):
    receipt={}
    try:receipt['shell_has_focus']=device.client.js('document.hasFocus()') is True
    except Exception:receipt['shell_focus_unavailable']=True
    if sys.platform.startswith('linux'):
        try:
            env={**os.environ,'DISPLAY':device.display}
            xid=subprocess.check_output(['xdotool','getwindowfocus'],env=env,text=True,stderr=subprocess.DEVNULL,timeout=2).strip()
            pid=int(subprocess.check_output(['xdotool','getwindowpid',xid],env=env,text=True,stderr=subprocess.DEVNULL,timeout=2).strip())
            import psutil
            owner=psutil.Process(device.app.pid)
            receipt['native_focus_role']='shell' if pid==owner.pid else 'owned_child' if pid in {p.pid for p in owner.children(recursive=True)} else 'other'
        except Exception:receipt['native_focus_unavailable']=True
    return receipt


def keys(device,*sequence):
    trace=getattr(device,'feature_keys',None)
    if trace is None:trace=[];device.feature_keys=trace
    if len(trace)<32:trace.append({'keys':[key if key in {'Tab','shift+Tab','space','Home','End','Return','enter','Escape','esc','Up','Down'} else 'other' for key in sequence[:12]],'before':input_observation(device)})
    if sys.platform=='win32':
        import pyautogui as gui
        for key in sequence:
            if '+' in key:gui.hotkey(*key.lower().split('+'))
            else:gui.press(key.lower())
    else:
        for key in sequence:subprocess.run(['xdotool','key','--clearmodifiers',key],env={**os.environ,'DISPLAY':device.display},check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)


def focus_steps(target):
    # Read actual shipped HTML. Runtime state has enabled two download buttons,
    # visible quality/caption selectors; Cancel/ordinary source link are not on
    # the path between these target controls and the native Play focus anchor.
    class Focus(HTMLParser):
        def __init__(self):super().__init__();self.order=[]
        def handle_starttag(self,tag,attrs):
            a=dict(attrs)
            if a.get('id') in {'captions','download-video','download-audio','sponsor','play','seek','quality','volume','fullscreen'}:self.order.append(a['id'])
            elif tag=='a' and a.get('href') in ('https://sponsor.ajay.app/','https://returnyoutubedislike.com/'):
                self.order.append('sponsor-link' if 'sponsor.ajay' in a['href'] else 'ratings-link')
    parser=Focus();parser.feed((Path(__file__).resolve().parents[2]/'desktop/src-tauri/src/youtube_player/player.html').read_text(encoding='utf-8'))
    expected=['captions','download-video','download-audio','sponsor','sponsor-link','ratings-link','play','seek','quality','volume','fullscreen']
    require(parser.order==expected,'Shipped feature focus order changed')
    delta=parser.order.index(target)-parser.order.index('play')
    return ('Tab' if delta>0 else 'shift+Tab',abs(delta))


CONTROL_IDS={'play':1,'seek':2,'quality':3,'volume':4,'fullscreen':5,'captions':6,
             'download-video':7,'download-audio':8,'cancel':9,'sponsor':10,'ordinary':11}


def focus_control(client,device,tab,target,diagnostics):
    require(target in CONTROL_IDS,'Unknown native focus target')
    direction=focus_steps(target)[0]
    target_id=CONTROL_IDS[target]
    navigation=diagnostics.setdefault('control_navigation',[])
    require(len(navigation)<8,'Native focus diagnostic bound exceeded')
    receipt={'target':target_id,'backwards':direction=='shift+Tab','tabs':0,'observed_controls':[],'reached':False}
    navigation.append(receipt)
    for attempt in range(17):
        state=snapshot(client,tab);active=state.get('active_control')
        require(type(active)is int and 0<=active<=11,'Native focus feedback missing or invalid')
        receipt['observed_controls'].append(active)
        if active==target_id:
            receipt['reached']=True
            return state
        require(attempt<16,'Native physical focus target not reached within bound')
        keys(device,direction);receipt['tabs']+=1
    raise AssertionError('Native focus bound unreachable')


def quality_control(client,device,tab,index,diagnostics):
    state=focus_control(client,device,tab,'quality',diagnostics)
    require(type(state.get('quality_count'))is int and state['quality_count']==2 and type(state.get('quality_index'))is int
            and 0<=state['quality_index']<2 and index in (0,1),'Native two-option quality feedback invalid')
    changed_at=time.monotonic()
    if state['quality_index']!=index:
        # A closed focused native select changes exactly one option with arrows.
        # No ambiguous popup Space/Home/End/Return sequence.
        keys(device,'Up' if index<state['quality_index'] else 'Down')
    return changed_at


def snapshot(client,tab):
    value=observe(client,SUPER,tab)
    diagnostics=getattr(client,'feature_diagnostics',None)
    if diagnostics is not None:
        clean=safe_state(value);diagnostics['observations']=diagnostics.get('observations',0)+1;diagnostics['last_snapshot']=clean
        transitions=diagnostics.setdefault('state_transitions',[])
        signature={key:clean.get(key) for key in ('video_width','video_height','playing','video_paused','audio_paused','caption_selected','fullscreen')}
        if len(transitions)<8 and (not transitions or transitions[-1]['signature']!=signature):transitions.append({'signature':signature,'state':clean})
    return value


def caption_pixels(client,device):
    from PIL import ImageGrab
    box=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height}})()')
    require(box['w']>=700 and box['h']>=400,'Caption viewport too small')
    origin=call(client,'plugin:window|inner_position',label='main');scale=client.js('devicePixelRatio')
    # Shipped flex layout anchors media immediately above controls/status; this
    # lower media band excludes controls, headers and mouse focus indicators.
    coordinates=(round(origin['x']+(box['x']+box['w']*.1)*scale),round(origin['y']+(box['y']+box['h']-185)*scale),
                 round(origin['x']+(box['x']+box['w']*.9)*scale),round(origin['y']+(box['y']+box['h']-102)*scale))
    with ImageGrab.grab(**({'xdisplay':device.display} if sys.platform!='win32' else {})) as image:
        require(0<=coordinates[0]<coordinates[2]<=image.width and 0<=coordinates[1]<coordinates[3]<=image.height,'Caption paint outside screen')
        return image.crop(coordinates).convert('RGB')


def changed_fraction(before,after):
    from PIL import ImageChops
    if before.size!=after.size:return None
    difference=ImageChops.difference(before,after)
    pixels=difference.get_flattened_data() if hasattr(difference,'get_flattened_data') else difference.getdata()
    changed=sum(max(pixel)>20 for pixel in pixels)
    return changed/(before.width*before.height)


def native_geometry(device):
    owner=native_owner(device)
    if sys.platform=='win32':
        rect=owner.rectangle();return (rect.left,rect.top,rect.right-rect.left,rect.bottom-rect.top)
    result=subprocess.run(['xdotool','getwindowgeometry','--shell',str(owner)],env=device.player_wm.env,capture_output=True,text=True,timeout=3)
    require(result.returncode==0,'Native geometry query failed')
    values=dict(line.split('=',1) for line in result.stdout.splitlines() if '=' in line)
    return tuple(int(values[key]) for key in ('X','Y','WIDTH','HEIGHT'))


def native_fullscreen(client,device):
    tauri=call(client,'plugin:window|is_fullscreen',label='main')
    if sys.platform=='win32':
        import pyautogui
        rect=native_owner(device).rectangle();width,height=pyautogui.size()
        actual=abs(rect.left)<=3 and abs(rect.top)<=3 and abs(rect.right-width)<=3 and abs(rect.bottom-height)<=3
    else:
        owner=native_owner(device)
        result=subprocess.run(['xprop','-id',str(owner),'_NET_WM_STATE'],env=device.player_wm.env,capture_output=True,text=True,timeout=3)
        require(result.returncode==0,'Native fullscreen state query failed')
        actual='_NET_WM_STATE_FULLSCREEN' in result.stdout
    return bool(tauri),actual


def paused(client,device,tab,diagnostics):
    state=snapshot(client,tab)
    if state.get('playing'):physical(client,device,'play',diagnostics)
    value=wait_for(lambda:(lambda s:s if not s['playing'] and s['video_paused'] and s['audio_paused'] else None)(snapshot(client,tab)),'Native feature pause missing',15)
    time.sleep(.5);again=snapshot(client,tab)
    require(not again['playing'] and abs(again['audio_position']-value['audio_position'])<.15,'Paused feature clocks changed')
    return again


def run(args):
    require(os.environ.get('GITHUB_ACTIONS')=='true','Disposable GHA runner required')
    out=Path(args.out).resolve();out.mkdir(parents=True,exist_ok=False)
    root=Path(tempfile.mkdtemp(prefix='nb-player-'));device=fixture=trust=wm=None
    report={'schema':1,'completed':False,'stage':'setup','checks':[],'extractor_acceptance':False};code=1
    names=('NEXUSBRO_TEST_YOUTUBE_FEATURES','NEXUSBRO_TEST_YOUTUBE_DNS_GUARD','NEXUSBRO_TEST_YOUTUBE_CAPTIONS_CA','NEXUSBRO_TEST_YOUTUBE_CAPTIONS_CA_SHA256');old={name:os.environ.get(name) for name in names}
    try:
        thumb=certificates(root/'tls',hosts=[HOST,'www.youtube.com']);trust=Trust(root,root/'tls'/'ca.crt',thumb);trust.install()
        fixture=Fixture(root/'tls'/'server.crt',root/'tls'/'server.key',Path(args.video_webm),Path(args.audio_webm),video_high=Path(args.video_high_webm),caption_body=CAPTION)
        report['tls_preflight']=tls_proxy_preflight(fixture.proxies['A'],HOST,443,root/'tls'/'ca.crt',root/'tls'/'server.crt')
        os.environ[names[0]]='1';os.environ[names[1]]='1';os.environ[names[2]]=str(root/'tls'/'ca.crt');os.environ[names[3]]=hashlib.sha256((root/'tls'/'ca.crt').read_bytes()).hexdigest()
        exe,cef=Path(args.exe).resolve(),Path(args.cef).resolve();device=WindowsDevice(root/'device',exe,cef) if sys.platform=='win32' else Device('features',root/'device',exe,cef)
        item=super_record(SUPER,'Feature fixture','about:blank',proxy='http://127.0.0.1:'+str(fixture.proxies['A']))
        folder=device.data/'profili'/SUPER;folder.mkdir(parents=True)
        (folder/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':'about:blank','zagolovok':'Fixture','otlozhena':True}],'aktivnaya':1,'sleduyushchaya':2}))
        for filename in ('vkladki.json','sohranennye.json'):(device.data/filename).write_text(json.dumps([item]))
        (device.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False}))
        client=device.start()
        if sys.platform!='win32':wm=OwnedWindowManager(device,root/'wm');device.player_wm=wm
        foreground(device)
        client.js('window.__playerObserved=[];window.__decodeObserved=[];window.__featureReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-state")window.__playerObserved.push(e.payload);if(e.payload?.data?.tip==="youtube-decode-state")window.__decodeObserved.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__featureReady=true);true')
        js_wait(client,'window.__featureReady','Native feature listener missing');switch_super(client,SUPER)
        # Restored lazy about:blank needs the shipped physical Open action.
        if client.js('!!document.querySelector(".pusto button")'):click_control(client,'.pusto button',20)
        session=wait_for(lambda:rows(client,SUPER) if any(t.get('browser_id',0)>0 and not t.get('zagruzka') for t in rows(client,SUPER)['vkladki']) else None,'Native feature browser missing',40);tab=session['aktivnaya']
        call(client,'obychnaya_komanda',id=SUPER,komanda={'deystvie':'youtube_fixture','id':tab})
        initial=wait_for(lambda:(lambda s:s if s.get('configured') and s.get('video_width')==640 and s.get('video_height')==360 else None)(observe(client,SUPER,tab,admitting=True)),'Initial high geometry missing',35)
        require(not initial['playing'],'Feature fixture autoplayed');diagnostics={}
        report['feature_diagnostics']=diagnostics;diagnostics['initial_snapshot']=safe_state(initial);client.feature_diagnostics=diagnostics
        report['stage']='quality-paused'
        # Two explicit native Play clicks establish real focus and restore pause
        # intent before changing quality; no playback starts due to the change.
        physical(client,device,'play',diagnostics)
        wait_for(lambda:(lambda s:s['playing'] and s['decoded']>=3)(snapshot(client,tab)),'Initial native decode missing',20)
        physical(client,device,'play',diagnostics)
        before_quality=paused(client,device,tab,diagnostics)
        report['stage']='native-decode-observation'
        call(client,'obychnaya_komanda',id=SUPER,komanda={'deystvie':'youtube_decode_snapshot','id':tab,'video_id':'BaW_jenozKc'})
        decode_ok=False
        try:
            event=wait_for(lambda:client.js('window.__decodeObserved.find(e=>e.super_id==='+json.dumps(SUPER)+'&&e.data?.id==='+str(tab)+')'),'Native decode observation receipt missing',10)
            data=event.get('data',{});state=data.get('state')
            if isinstance(state,dict):
                decode_ok=(state.get('codec')=='vp9' and state.get('video_pipelines')==1
                           and type(state.get('platform_decoder'))is bool and state.get('hardware_activity_proven') is False)
                report['native_decoder']={key:state[key] for key in ('codec','decoder','platform_decoder','decode_mode','hardware_activity_proven','video_pipelines') if key in state}
            else:
                code=data.get('code')
                report['native_decoder']={'observed':False,'code':code if isinstance(code,str) and 0<len(code)<=96 and code.isascii() and all(c.isalnum() or c=='_' for c in code) else 'unavailable'}
        except Exception:
            report['native_decoder']={'observed':False,'code':'native_receipt_unavailable'}
        report['stage']='quality-paused'
        quality_control(client,device,tab,0,diagnostics)
        low=wait_for(lambda:(lambda s:s if s.get('quality_count')==2 and s.get('quality_index')==0 and s['video_width']==320 and s['video_height']==180 and not s['playing'] else None)(snapshot(client,tab)),'Paused low quality geometry missing',25)
        time.sleep(.8);still=snapshot(client,tab)
        require(not still['playing'] and abs(still['audio_position']-before_quality['audio_position'])<.2,'Paused quality resumed without Play')
        report['checks'].append({'name':'quality-no-autoplay','width':320,'height':180})
        report['stage']='quality-playing';physical(client,device,'play',diagnostics)
        playing=wait_for(lambda:(lambda s:s if s['playing'] and s['audio_position']>1 and s['decoded']>=3 else None)(snapshot(client,tab)),'Feature playback missing',25)
        started=quality_control(client,device,tab,1,diagnostics)
        changed=wait_for(lambda:(lambda s:s if s.get('quality_count')==2 and s.get('quality_index')==1 and s['playing'] and s['video_width']==640 and s['video_height']==360 and s['decoded']>=3 else None)(snapshot(client,tab)),'Playing quality change failed',25)
        require(playing['audio_position']-.3<=changed['audio_position']<=playing['audio_position']+(time.monotonic()-started)+4 and changed['skew']<.6,'Quality audio continuity failed')
        report['checks'].append({'name':'quality-playing-continuity','width':640,'height':360,'skew':round(changed['skew'],3)})
        report['stage']='captions';stable=paused(client,device,tab,diagnostics);before=caption_pixels(client,device)
        # Native feedback establishes actual focus without playing a frame.
        caption_focus=focus_control(client,device,tab,'captions',diagnostics)
        require(caption_focus.get('caption_selected')==-1,'Initial caption select was not off')
        keys(device,'Down')
        caption=wait_for(lambda:(lambda s:s if s['caption_selected']==0 and s['caption_showing'] and s['caption_cues']>0 and s['caption_active']>0 and not s['playing'] else None)(snapshot(client,tab)),'Actual native caption track missing',25)
        require(abs(caption['audio_position']-stable['audio_position'])<.2,'Caption paint media frame changed')
        fraction=wait_for(lambda:(lambda value:value if value is not None and .0001<value<.3 else None)(changed_fraction(before,caption_pixels(client,device))),'Caption pixels not physically painted',10)
        # Disable using the still-focused actual native select; no CEF JS action.
        focus_control(client,device,tab,'captions',diagnostics);keys(device,'Up')
        wait_for(lambda:(lambda s:s if s['caption_selected']==-1 and not s['caption_showing'] and s['caption_active']==0 else None)(snapshot(client,tab)),'Caption disable failed',10)
        wait_for(lambda:(lambda value:value is not None and value<=.0001)(changed_fraction(before,caption_pixels(client,device))),'Caption pixels did not clear',10)
        with fixture.lock:
            captions=[r for r in fixture.receipts if r.get('kind')=='caption']
            require(captions and all(r['lane']=='A' and r.get('host')=='youtube' for r in captions),'Caption actual route witness missing')
        report['checks'].append({'name':'captions-native-fetch-and-paint','active_cues':caption['caption_active'],'changed_fraction':round(fraction,6),'route_requests':len(captions)})
        report['stage']='fullscreen';require(native_fullscreen(client,device)==(False,False),'Owner initially fullscreen');original_geometry=native_geometry(device)
        # Feedback identifies the actual button; Space is the intended click.
        focus_control(client,device,tab,'fullscreen',diagnostics);keys(device,'space')
        wait_for(lambda:native_fullscreen(client,device)==(True,True) and snapshot(client,tab)['fullscreen'] and client.js('document.querySelector(".okno")?.classList.contains("youtube-fullscreen")'),'Actual OS fullscreen missing',15)
        keys(device,'Escape' if sys.platform!='win32' else 'esc')
        wait_for(lambda:native_fullscreen(client,device)==(False,False) and not snapshot(client,tab)['fullscreen'],'Fullscreen Escape restore failed',15)
        require(not client.js('document.querySelector(".okno")?.classList.contains("youtube-fullscreen")'),'Fullscreen browser chrome not restored')
        restored_geometry=native_geometry(device)
        require(all(abs(a-b)<=8 for a,b in zip(original_geometry,restored_geometry)),'Fullscreen original OS geometry not restored')
        report['checks'].append({'name':'actual-native-fullscreen-and-escape','os_fullscreen':True,'cef_fullscreen':True,'restored':True})
        with fixture.lock:
            media=[r for r in fixture.receipts if r.get('kind')=='media' and r.get('track')=='video']
            require({r.get('quality') for r in media}>={'low','high'} and all(r['lane']=='A' for r in media) and fixture.direct==0,'Feature native route bypass')
        report['stage']='native-decoder-final-check'
        require(decode_ok,'Native bounded decoder observation unavailable')
        report['completed']=True;report['stage']='complete';code=0
    except Exception as error:
        report['failure_class']=type(error).__name__;trace=error.__traceback__
        while trace:
            if Path(trace.tb_frame.f_code.co_filename).name==Path(__file__).name:report['failure_location']={'function':trace.tb_frame.f_code.co_name,'line':trace.tb_lineno}
            trace=trace.tb_next
    finally:
        cleanup=True
        for obj in (device,fixture,trust,wm):
            if obj:
                try:obj.close()
                except Exception:cleanup=False
        for name,value in old.items():
            if value is None:os.environ.pop(name,None)
            else:os.environ[name]=value
        try:shutil.rmtree(root)
        except OSError:cleanup=False
        if device and getattr(device,'feature_keys',None):report['physical_keys']=device.feature_keys
        report['cleanup_completed']=cleanup
        if not cleanup:report['completed']=False;code=1
        (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    return code


def self_test():
    original_snapshot,original_keys=globals()['snapshot'],globals()['keys']
    pressed=[]
    try:
        feedback=iter([{'active_control':1},{'active_control':2},{'active_control':3}])
        globals()['snapshot']=lambda *_:next(feedback)
        globals()['keys']=lambda _device,*sequence:pressed.extend(sequence)
        diagnostics={};focus_control(None,None,1,'quality',diagnostics)
        require(pressed==['Tab','Tab'] and diagnostics['control_navigation'][0]['reached'],'Native focus feedback navigation failed')
        pressed.clear();globals()['snapshot']=lambda *_:{'active_control':0}
        bounded={}
        try:focus_control(None,None,1,'quality',bounded)
        except Exception:pass
        else:raise AssertionError('Missing physical focus was accepted')
        require(len(pressed)==16 and bounded['control_navigation'][0]['tabs']==16,'Physical focus iteration bound failed')
        pressed.clear();globals()['snapshot']=lambda *_:{'active_control':3,'quality_count':2,'quality_index':1}
        quality_control(None,None,1,0,{})
        require(pressed==['Up'],'Closed low quality selection did not use exact arrow')
        pressed.clear();globals()['snapshot']=lambda *_:{'active_control':3,'quality_count':2,'quality_index':0}
        quality_control(None,None,1,1,{})
        require(pressed==['Down'],'Closed high quality selection did not use exact arrow')
    finally:globals()['snapshot'],globals()['keys']=original_snapshot,original_keys
    require(safe_state({'url':'secret','duration':float('nan'),'playing':True,'video_width':640})=={'playing':True,'video_width':640},'Diagnostic field filtering failed')
    require(focus_steps('quality')==('Tab',2) and focus_steps('fullscreen')==('Tab',4) and focus_steps('captions')==('shift+Tab',6),'Feature focus sequence mismatch')
    from PIL import Image
    black=Image.new('RGB',(100,100),'black');white=black.copy();white.putpixel((50,50),(255,255,255))
    require(changed_fraction(black,black)==0 and 0<changed_fraction(black,white)<.01,'Caption difference classifier mismatch')
    return {'self_test':True,'source_only':True,'native_execution':False}


if __name__=='__main__':
    if sys.argv[1:]==['--self-test']:print(json.dumps(self_test()));raise SystemExit(0)
    parser=argparse.ArgumentParser()
    for name in ('exe','cef','video-webm','audio-webm','video-high-webm','out'):parser.add_argument('--'+name,required=True)
    raise SystemExit(run(parser.parse_args()))
