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


def keys(device,*sequence):
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
    parser=Focus();parser.feed((Path(__file__).resolve().parents[2]/'desktop/src-tauri/src/youtube_player/player.html').read_text())
    expected=['captions','download-video','download-audio','sponsor','sponsor-link','ratings-link','play','seek','quality','volume','fullscreen']
    require(parser.order==expected,'Shipped feature focus order changed')
    delta=parser.order.index(target)-parser.order.index('play')
    return ('Tab' if delta>0 else 'shift+Tab',abs(delta))


def snapshot(client,tab):return observe(client,SUPER,tab)


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
        client.js('window.__playerObserved=[];window.__featureReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-state")window.__playerObserved.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__featureReady=true);true')
        js_wait(client,'window.__featureReady','Native feature listener missing');switch_super(client,SUPER)
        # Restored lazy about:blank needs the shipped physical Open action.
        if client.js('!!document.querySelector(".pusto button")'):click_control(client,'.pusto button',20)
        session=wait_for(lambda:rows(client,SUPER) if any(t.get('browser_id',0)>0 and not t.get('zagruzka') for t in rows(client,SUPER)['vkladki']) else None,'Native feature browser missing',40);tab=session['aktivnaya']
        call(client,'obychnaya_komanda',id=SUPER,komanda={'deystvie':'youtube_fixture','id':tab})
        initial=wait_for(lambda:(lambda s:s if s.get('configured') and s.get('video_width')==640 and s.get('video_height')==360 else None)(observe(client,SUPER,tab,admitting=True)),'Initial high geometry missing',35)
        require(not initial['playing'],'Feature fixture autoplayed');diagnostics={}
        report['stage']='quality-paused'
        # Two explicit native Play clicks establish real focus and restore pause
        # intent before changing quality; no playback starts due to the change.
        physical(client,device,'play',diagnostics);physical(client,device,'play',diagnostics)
        before_quality=paused(client,device,tab,diagnostics)
        key,count=focus_steps('quality');keys(device,*([key]*count));keys(device,'space','Home','Return' if sys.platform!='win32' else 'enter')
        low=wait_for(lambda:(lambda s:s if s['video_width']==320 and s['video_height']==180 and not s['playing'] else None)(snapshot(client,tab)),'Paused low quality geometry missing',25)
        time.sleep(.8);still=snapshot(client,tab)
        require(not still['playing'] and abs(still['audio_position']-before_quality['audio_position'])<.2,'Paused quality resumed without Play')
        report['checks'].append({'name':'quality-no-autoplay','width':320,'height':180})
        report['stage']='quality-playing';physical(client,device,'play',diagnostics)
        playing=wait_for(lambda:(lambda s:s if s['playing'] and s['audio_position']>1 and s['decoded']>=3 else None)(snapshot(client,tab)),'Feature playback missing',25)
        key,count=focus_steps('quality');keys(device,*([key]*count));started=time.monotonic();keys(device,'space','End','Return' if sys.platform!='win32' else 'enter')
        changed=wait_for(lambda:(lambda s:s if s['playing'] and s['video_width']==640 and s['video_height']==360 and s['decoded']>=3 else None)(snapshot(client,tab)),'Playing quality change failed',25)
        require(playing['audio_position']-.3<=changed['audio_position']<=playing['audio_position']+(time.monotonic()-started)+4 and changed['skew']<.6,'Quality audio continuity failed')
        report['checks'].append({'name':'quality-playing-continuity','width':640,'height':360,'skew':round(changed['skew'],3)})
        report['stage']='captions';stable=paused(client,device,tab,diagnostics);before=caption_pixels(client,device)
        # paused() left actual Play focused. Move focus without playing even a
        # single frame, so caption differences cannot be animated-video pixels.
        key,count=focus_steps('captions');keys(device,*([key]*count));keys(device,'space','End','Return' if sys.platform!='win32' else 'enter')
        caption=wait_for(lambda:(lambda s:s if s['caption_selected']==0 and s['caption_showing'] and s['caption_cues']>0 and s['caption_active']>0 and not s['playing'] else None)(snapshot(client,tab)),'Actual native caption track missing',25)
        require(abs(caption['audio_position']-stable['audio_position'])<.2,'Caption paint media frame changed')
        fraction=wait_for(lambda:(lambda value:value if value is not None and .0001<value<.3 else None)(changed_fraction(before,caption_pixels(client,device))),'Caption pixels not physically painted',10)
        # Disable using the still-focused actual native select; no CEF JS action.
        keys(device,'space','Home','Return' if sys.platform!='win32' else 'enter')
        wait_for(lambda:(lambda s:s if s['caption_selected']==-1 and not s['caption_showing'] and s['caption_active']==0 else None)(snapshot(client,tab)),'Caption disable failed',10)
        wait_for(lambda:(lambda value:value is not None and value<=.0001)(changed_fraction(before,caption_pixels(client,device))),'Caption pixels did not clear',10)
        with fixture.lock:
            captions=[r for r in fixture.receipts if r.get('kind')=='caption']
            require(captions and all(r['lane']=='A' and r.get('host')=='youtube' for r in captions),'Caption actual route witness missing')
        report['checks'].append({'name':'captions-native-fetch-and-paint','active_cues':caption['caption_active'],'changed_fraction':round(fraction,6),'route_requests':len(captions)})
        report['stage']='fullscreen';require(native_fullscreen(client,device)==(False,False),'Owner initially fullscreen');original_geometry=native_geometry(device)
        # Caption selector remains focused after disabling. Move six real Tab
        # stops to Play, then four to Fullscreen, keeping the same paused frame.
        key,count=focus_steps('captions');keys(device,*(['Tab']*count))
        key,count=focus_steps('fullscreen');keys(device,*([key]*count),'space')
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
        report['cleanup_completed']=cleanup
        if not cleanup:report['completed']=False;code=1
        (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    return code


def self_test():
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
