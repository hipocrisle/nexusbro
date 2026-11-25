#!/usr/bin/env python3
"""GHA-only ordinary YouTube reference metadata, never comparison acceptance.

No light extraction, DOM playback mutation, account import or challenge handling.
Native reference admission remains the authority for video identity and ad state.
"""
import argparse
import json
import hashlib
import io
import subprocess
import math
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import time

from youtube_paired_native import (
    Device, WindowsDevice, switch_super, rows, probe, normal_key, route_file,
    OwnedWindowManager, close_for_privacy, super_record, call, click_control,
    Blocked, foreground,
)

from youtube_player_native import native_app_identity, native_app_identity_alive, native_app_window

BOOL_FIELDS=('playing','paused','ended','seeking','muted','default_sink')
NUMBER_FIELDS=('duration','video_width','video_height','position','decoded','dropped',
               'ready_state','volume','playback_rate','video_count')


def initial_report():
    return {'schema':1,'diagnostic_only':True,'completed':False,'metadata_observed':False,'snapshot_observed':False,
            'measurement_valid':False,'comparison_valid':False,
            'accepted_measurement':False,'acceptance_passed':False,
            'playback_observed':False,'clock_advanced':False,'reference_metadata':None,
            'budgets':{'startup_observation_seconds':45,'metadata_admission_window_seconds':30,
                       'metadata_window_is_soft':True,'shared_probe_reservation_seconds':60,
                       'work_admission_seconds':150,'cleanup_reservation_seconds':90,
                       'outer_workflow_timeout_seconds':300},
            'stage':'setup','snapshots':[],'play_key_sent':False,'ordinary_tab_focus_click_sent':False,'cleanup_completed':False}


def scalars(state):
    if not isinstance(state,dict):raise Blocked('reference_metadata_invalid')
    out={}
    for key in BOOL_FIELDS:
        if type(state.get(key))is not bool:raise Blocked('reference_metadata_invalid')
        out[key]=state[key]
    for key in NUMBER_FIELDS:
        value=state.get(key)
        if type(value)not in (int,float) or not math.isfinite(value) or not 0<=value<=10**12:
            raise Blocked('reference_metadata_invalid')
        out[key]=value
    if out['video_count']!=1 or not 0<=out['ready_state']<=4 or not 0<=out['volume']<=1:
        raise Blocked('reference_metadata_invalid')
    return out  # Explicit scalars only, never strings, URLs, titles or identifiers.


def metadata_ready(state):
    return (state['ready_state']>=2 and state['duration']>0
            and state['video_width']>0 and state['video_height']>0 and not state['ended'])


def advancement(first,last,wall):
    frames=last['decoded']-first['decoded'];clock=last['position']-first['position']
    if not math.isfinite(wall) or wall<=0 or frames<0 or clock<0:
        raise Blocked('reference_clock_changed')
    return {'wall_seconds':round(wall,3),'advanced_frames':frames,
            'advanced_media_seconds':clock,'clock_advanced':clock>0,'decoded_advanced':frames>0,'observed_frames_per_media_second':
            frames/clock if clock>0 else None,
            'observed_frames_per_wall_second':frames/wall}


class CropUnavailable(Exception):
    pass


def crop_box(rect,origin,scale,screen):
    values=[rect.get(k) for k in ('x','y','w','h','vw','vh')]+[origin.get('x'),origin.get('y'),scale]+list(screen)
    if any(type(v)not in(int,float) or not math.isfinite(v) for v in values):raise CropUnavailable('geometry_invalid')
    if not 0.5<=scale<=4 or min(rect['w'],rect['h'],rect['vw'],rect['vh'])<=0:raise CropUnavailable('geometry_invalid')
    left=math.ceil(origin['x']+max(0,rect['x'])*scale)
    top=math.ceil(origin['y']+max(0,rect['y'])*scale)
    right=min(math.floor(origin['x']+min(rect['vw'],rect['x']+rect['w'])*scale),left+640)
    bottom=min(math.floor(origin['y']+min(rect['vh'],rect['y']+rect['h'])*scale),top+360)
    if not (0<=left<right<=screen[0] and 0<=top<bottom<=screen[1]):raise CropUnavailable('geometry_invalid')
    return left,top,right,bottom


def failure_status_crop(client,device,wm,out,identifier,tab,video):
    """One failure-only owned public content crop; no UI content interaction."""
    receipt={'saved':False,'width':0,'height':0,'bytes':0,'sha256':None,'failure':'ownership_unproven'}
    def ensure(condition,reason):
        if not condition:raise CropUnavailable(reason)
    try:
        ensure(os.environ.get('GITHUB_ACTIONS')=='true' and sys.platform=='linux'
               and video=='aqz-KE-bpKQ','scope_denied')
        ensure(device is not None and client is not None and isinstance(wm,OwnedWindowManager)
               and device.player_wm is wm and wm.device is device and wm.env.get('DISPLAY')==device.display and wm.process.poll()is None
               and device.x is not None and device.x.poll()is None
               and re.fullmatch(r':\d+',device.display or '') is not None,'ownership_unproven')
        identity=native_app_identity(device);owner=native_app_window(device,identity)
        expected=int(str(owner),0)
        def active():
            text=subprocess.check_output(['xdotool','getactivewindow'],env=wm.env,stderr=subprocess.DEVNULL,timeout=3,text=True).strip()
            ensure(text.isdecimal() and len(text)<=20,'ownership_unproven')
            return int(text)
        def bound():
            ensure(device.x.poll()is None and wm.process.poll()is None,'ownership_unproven')
            ensure(native_app_identity_alive(device,identity) and int(str(native_app_window(device,identity)),0)==expected
                   and active()==expected,'ownership_unproven')
            ensure(client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')=='main','binding_unproven')
            ensure(type(tab)is int and tab>0 and identifier=='reference-metadata','binding_unproven')
            matched=client.js('(()=>{const s=document.querySelector(".vkladka.aktivna[data-super-id]"),t=document.querySelector(".obychnaya[aria-selected=true]");return !!s&&s.dataset.superId==="reference-metadata"&&!!t&&t.dataset.id==='+json.dumps(str(tab))+'})()')
            ensure(matched is True,'binding_unproven')
            value=rows(client,identifier,timeout=3)
            ensure(value.get('aktivnaya')==tab and any(v.get('id')==tab and v.get('browser_id',0)>0 and not v.get('route_pending') for v in value.get('vkladki',[])),'binding_unproven')
        foreground(device);bound()
        rect=client.js('(()=>{const r=document.querySelector(".pole")?.getBoundingClientRect();return r?{x:r.x,y:r.y,w:r.width,h:r.height,vw:innerWidth,vh:innerHeight}:null})()')
        ensure(isinstance(rect,dict),'geometry_invalid')
        origin=call(client,'plugin:window|inner_position',label='main',timeout=3)
        scale=client.js('devicePixelRatio')
        text=subprocess.check_output(['xdotool','getdisplaygeometry'],env=wm.env,stderr=subprocess.DEVNULL,timeout=3,text=True).split()
        ensure(len(text)==2 and all(v.isdecimal() for v in text),'geometry_invalid')
        box=crop_box(rect,origin,scale,tuple(map(int,text)))
        from PIL import ImageGrab
        bound()
        with ImageGrab.grab(bbox=box,xdisplay=device.display) as image:
            ensure(image.size==(box[2]-box[0],box[3]-box[1]),'capture_invalid')
            stream=io.BytesIO();image.save(stream,format='PNG');payload=stream.getvalue()
        bound()
        ensure(len(payload)<=100*1024,'encoded_size_limit')
        target=out/'reference-status.png'
        with target.open('xb') as stream:stream.write(payload)
        receipt.update(saved=True,width=box[2]-box[0],height=box[3]-box[1],bytes=len(payload),sha256=hashlib.sha256(payload).hexdigest(),failure=None)
    except CropUnavailable as error:receipt['failure']=str(error)
    except FileExistsError:receipt['failure']='output_exists'
    except Exception:receipt['failure']='capture_unavailable'
    return receipt


def run(args):
    report=initial_report();device=wm=None;root=None;client=None;closed=False;close_attempted=False
    out=Path(args.out).resolve();out.mkdir(parents=True,exist_ok=False)
    identifier='reference-metadata';tab=None
    started=time.monotonic();observation_deadline=started+150
    def remaining(cap):
        left=observation_deadline-time.monotonic()
        if left<=0:raise Blocked('reference_observation_deadline')
        return min(cap,left)
    def reference_probe(identifier,tab):
        # Same shared native authority and parser; no local bypass API. The
        # admission window is soft: an admitted probe can take shared 40+8s.
        if observation_deadline-time.monotonic()<60:raise Blocked('reference_observation_deadline')
        return scalars(probe(client,identifier,tab,'normal',args.video_id))
    def selected(identifier,tab):
        value=rows(client,identifier,timeout=remaining(3))
        item=next((v for v in value.get('vkladki',[]) if v.get('id')==tab),None)
        if not item or value.get('aktivnaya')!=tab or item.get('route_pending') or item.get('browser_id',0)<=0:
            raise Blocked('reference_selected_binding_changed')
    try:
        if os.environ.get('GITHUB_ACTIONS')!='true' or sys.platform not in ('win32','linux'):
            raise Blocked('gha_only')
        if args.video_id!='aqz-KE-bpKQ':raise Blocked('reference_video_invalid')
        if route_file(args.route_config)!='':raise Blocked('reference_direct_route_required')
        root=Path(tempfile.mkdtemp(prefix='nb-player-')).resolve(strict=True)
        exe=Path(args.exe).resolve(strict=True);cef=Path(args.cef).resolve(strict=True)
        device=WindowsDevice(root/'device',exe,cef) if sys.platform=='win32' else Device('reference-metadata',root/'device',exe,cef)
        identifier='reference-metadata';url='https://www.youtube.com/watch?v='+args.video_id
        record=super_record(identifier,'Public reference metadata',url,proxy='')
        folder=device.data/'profili'/identifier;folder.mkdir(parents=True)
        (folder/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':url,'zagolovok':'Public reference','otlozhena':True}],'aktivnaya':1,'sleduyushchaya':2}))
        for name in ('vkladki.json','sohranennye.json'):(device.data/name).write_text(json.dumps([record]))
        (device.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False,'memory_saver':False}))
        report['stage']='startup';client=device.start()
        if time.monotonic()-started>45:raise Blocked('reference_startup_deadline')
        if sys.platform!='win32':wm=OwnedWindowManager(device,root/'wm');device.player_wm=wm
        client.js('window.__paired=[];window.__pairedReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-reference-state")window.__paired.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__pairedReady=true);true')
        deadline=time.monotonic()+remaining(5)
        while not client.js('window.__pairedReady'):
            if time.monotonic()>=deadline:raise Blocked('reference_listener_unavailable')
            time.sleep(.1)
        switch_super(client,identifier,timeout=remaining(10))
        if client.js('!!document.querySelector(".pusto button")'):click_control(client,'.pusto button',remaining(10))
        report['stage']='metadata';deadline=time.monotonic()+remaining(30);state=None;tab=None
        while time.monotonic()<deadline:
            session=rows(client,identifier,timeout=remaining(3));tab=session.get('aktivnaya')
            if any(v.get('id')==tab and v.get('browser_id',0)>0 for v in session.get('vkladki',[])):
                try:
                    selected(identifier,tab)
                    state=reference_probe(identifier,tab)
                    report['snapshot_observed']=True;report['reference_metadata']=state
                    if metadata_ready(state):break
                except Blocked as error:
                    if str(error)!='native_normal_unavailable':raise
            time.sleep(.5)
        if state is None:raise Blocked('reference_guard_unavailable_ad_or_wall_or_not_ready')
        if not metadata_ready(state):raise Blocked('reference_metadata_unready')
        report['metadata_observed']=True;report['reference_metadata']=state;report['snapshots'].append(state)
        if state['paused'] and not state['ended'] and state['ready_state']>=2:
            report['stage']='optional-native-key';selected(identifier,tab)
            # Exactly one ordinary chrome-tab click, never a website/ad/consent
            # click, transfers focus through the product's normal selection UI.
            foreground(device)
            click_control(client,'.obychnaya[data-id="'+str(tab)+'"][aria-selected=true]',remaining(5))
            report['ordinary_tab_focus_click_sent']=True
            selected(identifier,tab);state=reference_probe(identifier,tab)
            report['reference_metadata']=state
            if state['paused'] and metadata_ready(state):
                normal_key(device,'k');report['play_key_sent']=True
                state=reference_probe(identifier,tab)
                report['reference_metadata']=state;report['snapshots'].append(state)
        if state['playing'] or report['play_key_sent']:
            report['stage']='clock-observation';first=state;began=time.monotonic()
            for offset in (2,5,10):
                delay=began+offset-time.monotonic()
                if delay>0:time.sleep(min(delay,remaining(delay)))
                selected(identifier,tab);state=reference_probe(identifier,tab)
                report['snapshots'].append(state);report['reference_metadata']=state
            report['observed_playback']=advancement(first,state,time.monotonic()-began)
            report['clock_advanced']=report['observed_playback']['clock_advanced']
            report['playback_observed']=report['clock_advanced'] and report['observed_playback']['decoded_advanced'] and state['playing']
        metadata=report['reference_metadata'];observation=report.get('observed_playback',{})
        fps=observation.get('observed_frames_per_media_second')
        if not (metadata['duration']>0 and metadata['video_width']>0 and metadata['video_height']>0):
            raise Blocked('reference_metadata_not_usable')
        if not (report['clock_advanced'] and report['playback_observed']
                and observation.get('advanced_frames',0)>0 and type(fps)in(int,float)
                and math.isfinite(fps) and 1<=fps<=120):
            raise Blocked('reference_playback_fps_not_observed')
        report['stage']='normal-owned-exit';close_attempted=True;close_for_privacy(client,device,report);closed=True
        report['completed']=True;report['stage']='complete'
    except Exception as error:
        report['status_crop']=failure_status_crop(client,device,wm,out,identifier,tab,args.video_id)
        report['blocked_reason']=str(error) if isinstance(error,Blocked) and str(error) in SAFE_CODES else 'native_reference_unavailable'
        report['failure_class']=type(error).__name__ if type(error).__name__ in ('Blocked','AssertionError','TimeoutExpired','OSError','RuntimeError') else 'NativeError'
    finally:
        # A failed observation still attempts ordinary owned shutdown. Emergency
        # Device cleanup can never turn a failed run into completed/accepted.
        if client is not None and not close_attempted and device is not None and device.app is not None:
            prior=report['stage']
            try:
                close_attempted=True;close_for_privacy(client,device,report)
            except Exception:report['normal_exit_incomplete']=True
            report['stage']=prior
        cleanup=True
        for obj in (device,wm):
            if obj:
                try:obj.close()
                except Exception:cleanup=False
        if root:
            try:shutil.rmtree(root)
            except OSError:cleanup=False
        receipt=report.get('app_shutdown_receipt')
        if isinstance(receipt,dict):
            report['app_shutdown_receipt']={key:receipt[key] for key in ('schema','complete','owners','clean','forced','unacknowledged','unclean','pending_starts') if key in receipt}
        report['cleanup_completed']=cleanup
        if not cleanup:report['completed']=False;report['blocked_reason']='cleanup_failed'
        if not report.get('owned_exit',{}).get('clean_shutdown_receipt_verified'):report['completed']=False
        report['wall_seconds']=round(time.monotonic()-started,3)
        (out/'result.json').write_text(json.dumps(report,indent=2,allow_nan=False)+'\n')
    return 0 if report['completed'] else 1


SAFE_CODES={'gha_only','reference_video_invalid','reference_direct_route_required','route_config_invalid',
 'reference_metadata_invalid','reference_clock_changed','reference_observation_deadline','reference_startup_deadline',
 'reference_selected_binding_changed','reference_listener_unavailable','reference_guard_unavailable_ad_or_wall_or_not_ready',
 'normal_cef_focus_unproven','normal_key_invalid','reference_metadata_not_usable','reference_playback_fps_not_observed','reference_metadata_unready'}


def self_test():
    state={k:False for k in BOOL_FIELDS};state.update({k:0 for k in NUMBER_FIELDS});state.update(video_count=1,ready_state=2,volume=1)
    assert scalars(dict(state,url='secret',video_id='secret'))==state
    for changed in ({'duration':float('nan')},{'decoded':True},{'video_count':2},{'playing':1},{'volume':2}):
        try:scalars(dict(state,**changed))
        except Blocked:pass
        else:raise AssertionError('invalid_native_metadata_accepted')
    assert advancement(dict(state,position=1,decoded=4),dict(state,position=3,decoded=28),2)['observed_frames_per_media_second']==12
    ready=dict(state,ready_state=2,duration=600,video_width=1920,video_height=1080)
    sequence=[state,state,ready]
    assert next((index for index,value in enumerate(sequence) if metadata_ready(value)),None)==2
    assert next((value for value in [state,state,state] if metadata_ready(value)),None)is None
    assert not metadata_ready(dict(ready,ended=True))
    assert crop_box({'x':20,'y':100,'w':900,'h':600,'vw':1000,'vh':800},{'x':0,'y':0},1,(1280,900))==(20,100,660,460)
    for bad in (float('nan'),0,5):
        try:crop_box({'x':0,'y':0,'w':10,'h':10,'vw':100,'vh':100},{'x':0,'y':0},bad,(1280,900))
        except CropUnavailable:pass
        else:raise AssertionError('invalid_crop_geometry_accepted')
    no_clock=advancement(state,state,10)
    assert no_clock['clock_advanced']is False and no_clock['decoded_advanced']is False
    assert no_clock['observed_frames_per_media_second']is None
    for key in ('measurement_valid','comparison_valid','accepted_measurement','acceptance_passed'):assert initial_report()[key]is False
    return {'self_test':True,'source_only':True,'native_execution':False}


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for key in ('exe','cef','route-config','video-id','out'):parser.add_argument('--'+key)
    parser.add_argument('--self-test',action='store_true');args=parser.parse_args()
    if args.self_test:print(json.dumps(self_test()));raise SystemExit(0)
    if not all(getattr(args,key) for key in ('exe','cef','route_config','video_id','out')):parser.error('Required metadata arguments missing')
    raise SystemExit(run(args))
