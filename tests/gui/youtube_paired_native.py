#!/usr/bin/env python3
"""Prepared GHA-only SAME-VM real public YouTube ABBA software experiment.
No fixture URLs, account/cookie imports, consent bypass, JS play/seek/quality or
normal-quality fallback. Future reference hook MUST verify movie_player exists,
no observed ad markers and getVideoData().video_id matches native expected ID.
That guard is not universal ad detection. Unknown evidence blocks acceptance.
Startup cost is WALL time; 5s sampling cannot count all short-lived startup CPU.
Exit0 means a valid policy-matched measurement, regardless of a resource win.
Native snapshots prove output POLICY (volume=1/unmuted/default sink), not
physical speaker output or PCM capture. There is no volume-setting JS injection.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.parse

import psutil
from security_native import Device, NativeFailure, call, js_wait, require, super_record, wait_for
from tab_routes import WindowsDevice, switch_super, rows, click_control
from youtube_player_native import physical, foreground, OwnedWindowManager, close_for_privacy
from resource_soak import Sampler, kernel_birth

ORDER=('light','normal','normal','light')
class Blocked(Exception):pass

def digest(path):
    value=hashlib.sha256()
    with Path(path).open('rb') as stream:
        for chunk in iter(lambda:stream.read(1024*1024),b''):value.update(chunk)
    return value.hexdigest()


def route_file(path):
    path=Path(path)
    if path.is_symlink() or not path.is_file() or not 0<path.stat().st_size<=16384:raise Blocked('route_config_invalid')
    def unique(pairs):
        result={}
        for key,value in pairs:
            if key in result:raise Blocked('route_config_invalid')
            result[key]=value
        return result
    try:value=json.loads(path.read_text(encoding='utf-8'),object_pairs_hook=unique)
    except (ValueError,UnicodeError):raise Blocked('route_config_invalid') from None
    if value=={'mode':'direct'}:return ''
    if not isinstance(value,dict) or set(value)!={'mode','url'} or value['mode']!='proxy' or not isinstance(value['url'],str):raise Blocked('route_config_invalid')
    try:
        url=urllib.parse.urlsplit(value['url']);port=url.port
        if url.scheme not in ('http','socks5') or not url.hostname or not port or url.path not in ('','/') or url.query or url.fragment or len(value['url'])>4096:raise ValueError()
    except ValueError:raise Blocked('route_config_invalid') from None
    return value['url']  # RAM/private disposable settings only, never output/argv.


def probe(client,super_id,tab,kind,video):
    tip={'light':'youtube-state','normal':'youtube-reference-state','decode':'youtube-decode-state'}[kind]
    command={'light':'youtube_snapshot','normal':'youtube_reference_snapshot','decode':'youtube_decode_snapshot'}[kind]
    client.js('window.__paired=[];true')
    params={'deystvie':command,'id':tab}
    if kind!='light':params['video_id']=video
    call(client,'obychnaya_komanda',id=super_id,komanda=params)
    expression='window.__paired.find(e=>e.super_id==='+json.dumps(super_id)+'&&e.data?.id==='+str(tab)+'&&e.data?.tip==='+json.dumps(tip)+')'
    result=wait_for(lambda:client.js(expression),'Paired native observation unavailable',8).get('data',{})
    state=result.get('state')
    if not isinstance(state,dict):raise Blocked('native_'+kind+'_unavailable')
    return state


def audio_policy(state):
    volume=state.get('volume')
    if type(volume) not in (int,float) or not math.isfinite(volume) or volume!=1 or state.get('muted') is not False or state.get('default_sink') is not True:
        raise Blocked('audio_output_policy_mismatch_or_unknown')
    return {'volume':volume,'muted':False,'default_sink':True}


def video_state(state,args,*,require_audio=True):
    for field in ('position','duration','decoded','dropped','video_width','video_height'):
        value=state.get(field)
        if type(value) not in (int,float) or not math.isfinite(value) or value<0:raise Blocked('video_observation_invalid')
    if abs(state['duration']-args.expected_duration)>2:raise Blocked('public_content_duration_mismatch')
    if (state['video_width'],state['video_height'])!=(args.width,args.height):raise Blocked('normal_or_light_quality_mismatch')
    if state.get('seeking') is True:raise Blocked('playback_seeking')
    if require_audio:audio_policy(state)
    return {field:state[field] for field in ('position','duration','decoded','dropped','video_width','video_height','playing') if field in state}


def decode_state(state):
    if (state.get('codec') not in ('vp8','vp9','av1','h264','hevc') or state.get('decoder') not in ('ffmpeg','vpx','aom')
            or state.get('platform_decoder') is not False or state.get('decode_mode')!='software'
            or state.get('hardware_activity_proven') is not False or state.get('video_pipelines')!=1):
        raise Blocked('same_known_software_decoder_not_proven')
    return {key:state[key] for key in ('codec','decoder','platform_decoder','decode_mode','hardware_activity_proven','video_pipelines')}


def native_members(client,identifier):
    value=call(client,'super_lifecycle_snapshot')
    if value.get('schema')!=1 or not isinstance(value.get('engines'),list) or len(value['engines'])!=1:raise Blocked('owned_schema_invalid')
    engine=value['engines'][0]
    if engine.get('id')!=identifier or engine.get('phase')!='Running' or engine.get('owned_complete') is not True or engine.get('tab_count')!=1 or engine.get('live_count')!=1:raise Blocked('owned_scope_incomplete')
    members=engine.get('owned');clock='windows_filetime' if sys.platform=='win32' else 'linux_ticks'
    if not isinstance(members,list) or not 0<len(members)<=128:raise Blocked('owned_members_invalid')
    cleaned=[];seen=set()
    for member in members:
        pid,birth=member.get('pid'),member.get('birth')
        if type(pid)is not int or pid<=0 or pid in seen or member.get('clock')!=clock or not isinstance(birth,str) or not birth.isdigit() or kernel_birth(pid,clock)!=birth:raise Blocked('owned_identity_invalid')
        cleaned.append({'pid':pid,'birth':birth,'clock':clock});seen.add(pid)
    return cleaned


def normal_key(device,key):
    if key not in ('k','m'):raise Blocked('normal_key_invalid')
    # No blind video/consent click. Require real CEF keyboard focus before the
    # single ordinary public-player keyboard shortcut.
    if sys.platform=='win32':
        import ctypes
        from ctypes import wintypes as w
        class Info(ctypes.Structure):_fields_=[('cbSize',w.DWORD),('flags',w.DWORD),('active',w.HWND),('focus',w.HWND),('capture',w.HWND),('menu',w.HWND),('move',w.HWND),('caret',w.HWND),('rect',w.RECT)]
        user=ctypes.windll.user32;user.GetForegroundWindow.restype=w.HWND
        pid=w.DWORD();thread=user.GetWindowThreadProcessId(user.GetForegroundWindow(),ctypes.byref(pid));info=Info();info.cbSize=ctypes.sizeof(info)
        if not user.GetGUIThreadInfo(thread,ctypes.byref(info)) or not info.focus:raise Blocked('normal_cef_focus_unproven')
        user.GetWindowThreadProcessId(info.focus,ctypes.byref(pid));pid=pid.value
    else:
        env={**os.environ,'DISPLAY':device.display}
        xid=subprocess.check_output(['xdotool','getwindowfocus'],env=env,text=True,stderr=subprocess.DEVNULL,timeout=3).strip()
        pid=int(subprocess.check_output(['xdotool','getwindowpid',xid],env=env,text=True,stderr=subprocess.DEVNULL,timeout=3).strip())
    process=psutil.Process(pid);owner=psutil.Process(device.app.pid)
    if pid not in {p.pid for p in owner.children(recursive=True)} or Path(process.exe()).resolve()!=device.exe.resolve() or not any(a=='--nb-dvizhok' or a.startswith('--type=') for a in process.cmdline()):raise Blocked('normal_cef_focus_unproven')
    if sys.platform=='win32':
        import pyautogui;pyautogui.press(key)
    else:subprocess.run(['xdotool','key','--clearmodifiers',key],env=env,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=3)


def metrics(samples,video,args):
    if len(samples)<19 or any(row.get('native_members_complete') is not True or row.get('skipped_stale_native_members',0)!=0 for row in samples):raise Blocked('resource_samples_incomplete')
    elapsed=samples[-1]['elapsed_seconds']-samples[0]['elapsed_seconds']
    media=video[-1]['position']-video[0]['position'];frames=video[-1]['decoded']-video[0]['decoded'];drops=video[-1]['dropped']-video[0]['dropped']
    if elapsed<89 or media<89 or abs(elapsed-media)>2 or frames<=0 or drops<0 or drops>frames:raise Blocked('playback_window_not_continuous')
    for previous,current,left,right in zip(video,video[1:],samples,samples[1:]):
        if current['decoded']<previous['decoded'] or current['dropped']<previous['dropped'] or abs((current['position']-previous['position'])-(right['elapsed_seconds']-left['elapsed_seconds']))>1:
            raise Blocked('playback_interval_or_counter_discontinuity')
    fps=frames/media
    if abs(fps-args.expected_fps)>max(2,args.expected_fps*.1):raise Blocked('observed_fps_mismatch')
    return dict(cpu_core_percent=100*sum(r['owned']['cpu_seconds_delta'] for r in samples[1:])/elapsed,
                peak_private_bytes=max(r['owned']['private_bytes'] for r in samples),peak_rss_bytes=max(r['owned']['rss_bytes'] for r in samples),
                measurement_start_position=video[0]['position'],measurement_end_position=video[-1]['position'],peak_processes=max(r['owned']['processes'] for r in samples),observed_fps=fps,dropped_ratio=drops/frames,measurement_seconds=elapsed,media_seconds=media)


MEASUREMENT_START_POSITION=60
MIN_STABLE_WARMUP_SECONDS=30
WARMUP_MAX_SECONDS=90

def warmup_to_segment(snapshot,args,result,*,clock=time.monotonic,sleep=time.sleep):
    """One predeclared segment; no seeking, user actions, retries or best-arm selection."""
    began=clock();stable_wall=began;stable_position=None;previous=None;previous_wall=None
    events=[];buffer_start=None
    while True:
        state=snapshot();now=clock();video_state(state,args)
        result['last_player_state']={k:v for k,v in state.items() if k in ('position','duration','decoded','dropped','playing','video_width','video_height','audio_position','audio_paused','video_paused','hidden')}
        result['warmup_wall_seconds']=now-began;result['warmup_buffer_events']=events
        if now-began>WARMUP_MAX_SECONDS:raise Blocked('warmup_deadline')
        playing=state.get('playing') is True
        if previous is not None and playing and previous.get('playing') is True:
            delta=state['position']-previous['position'];elapsed=now-previous_wall
            if delta<0 or delta>elapsed+1:raise Blocked('warmup_clock_discontinuity')
            if delta<=0:playing=False
        if not playing:
            stable_wall=None;stable_position=None
            if buffer_start is None:
                buffer_start=now
                events.append({'position':state['position'],'elapsed_seconds':now-began,'duration_seconds':0,'resumed_without_input':False})
                if len(events)>64:raise Blocked('warmup_event_bound')
            events[-1]['duration_seconds']=now-buffer_start
        else:
            if buffer_start is not None:
                events[-1]['duration_seconds']=now-buffer_start;events[-1]['resumed_without_input']=True;buffer_start=None
            if stable_wall is None:stable_wall=now;stable_position=state['position']
            elif stable_position is None:stable_position=state['position']
        if state['position']>=MEASUREMENT_START_POSITION:
            if state['position']>MEASUREMENT_START_POSITION+.5:raise Blocked('fixed_segment_start_missed')
            if not playing or stable_wall is None or now-stable_wall<MIN_STABLE_WARMUP_SECONDS or state['position']-stable_position<MIN_STABLE_WARMUP_SECONDS:raise Blocked('continuous_warmup_incomplete_at_fixed_segment')
            result['warmup_continuous_seconds']=now-stable_wall
            result['warmup_continuous_media_seconds']=state['position']-stable_position
            return state
        previous=state;previous_wall=now
        sleep(.1 if state['position']>=58 else .5)


def arm(args,proxy,mode,index,out,*,device_factory=None,close_owned=None,private_parent=None,window_manager_factory=None,state_probe=None):
    # Optional dependency injection for the local Home entry: every default
    # below preserves the existing GHA behavior (temp root, stock
    # Device/OwnedWindowManager, close_for_privacy, shared probe()).
    root=(Path(private_parent)/('arm-'+str(index))) if private_parent else Path(tempfile.mkdtemp(prefix='nb-player-'))
    if private_parent:root.mkdir(parents=True,mode=0o700,exist_ok=False)
    device=wm=sampler=None;result={'index':index,'mode':mode,'stage':'startup'}
    started=time.monotonic()
    try:
        make_device=device_factory or (lambda device_root,exe,cef: WindowsDevice(device_root,exe,cef) if sys.platform=='win32' else Device('paired',device_root,exe,cef))
        device=make_device(root/'device',Path(args.exe).resolve(),Path(args.cef).resolve())
        identifier='paired-'+str(index);url='https://www.youtube.com/watch?v='+args.video_id
        item=super_record(identifier,'Paired public video',url,proxy=proxy)
        folder=device.data/'profili'/identifier;folder.mkdir(parents=True)
        (folder/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':url,'zagolovok':'Public video','otlozhena':True}],'aktivnaya':1,'sleduyushchaya':2}))
        for name in ('vkladki.json','sohranennye.json'):(device.data/name).write_text(json.dumps([item]))
        (device.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False,'memory_saver':False}))
        client=device.start()
        if sys.platform!='win32':wm=(window_manager_factory or OwnedWindowManager)(device,root/'wm');device.player_wm=wm
        client.js('window.__paired=[];window.__pairedReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{if(["youtube-state","youtube-reference-state","youtube-decode-state","youtube-result"].includes(e.payload?.data?.tip))window.__paired.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__pairedReady=true);true')
        js_wait(client,'window.__pairedReady','Native paired listener missing')
        snapshot=(lambda kind: state_probe(client,identifier,tab,kind)) if state_probe else (lambda kind: probe(client,identifier,tab,kind,args.video_id))
        switch_super(client,identifier)
        if client.js('!!document.querySelector(".pusto button")'):click_control(client,'.pusto button',20)
        session=wait_for(lambda:rows(client,identifier) if any(v.get('browser_id',0)>0 and not v.get('zagruzka') for v in rows(client,identifier)['vkladki']) else None,'Public native page not ready',45);tab=session['aktivnaya']
        result['stage']='content-admission'
        if mode=='light':
            call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_light','id':tab})
            deadline=time.monotonic()+60;state=None
            while time.monotonic()<deadline:
                try:
                    observed=snapshot(mode)
                    result['admission_video_state']={k:v for k,v in observed.items() if k in ('configured','playing','position','duration','decoded','dropped','video_width','video_height','volume','muted','default_sink') and (type(v) is bool or type(v) in (int,float) and math.isfinite(v))}
                    # The shipped document can answer before media metadata is
                    # ready. Observe the SAME extraction; never start another.
                    duration=observed.get('duration')
                    if observed.get('configured') is True and type(duration) in (int,float) and math.isfinite(duration) and duration>0:
                        state=observed;break
                    time.sleep(.2)
                except (Blocked,NativeFailure) as error:
                    if isinstance(error,Blocked) and str(error).startswith('sidecar_failure_'):raise
                    time.sleep(.5)
            if state is None:raise Blocked('real_extraction_unavailable_or_challenged')
        else:state=snapshot(mode)
        video_state(state,args,require_audio=False)
        if mode=='normal' and state.get('muted') is True:
            if state.get('ready_state',0)<2:raise Blocked('normal_audio_user_action_unavailable')
            normal_key(device,'m')
            state=wait_for(lambda:(lambda value:value if value.get('muted') is False else None)(snapshot(mode)),'Normal physical unmute not observed',8)
        audio_policy(state)
        if not state.get('playing'):
            if mode=='light':physical(client,device,'play',{})
            elif state.get('paused') is True and state.get('ready_state',0)>=2:normal_key(device,'k')
            else:raise Blocked('normal_user_play_required')
        state=wait_for(lambda:(lambda s:s if s.get('playing') and s['position']>0 else None)(snapshot(mode)),'Real public playback missing',15)
        decode=decode_state(snapshot('decode'));result['decode']=decode
        result['startup_wall_seconds']=time.monotonic()-started
        result['startup_cpu_total_proven']=False
        result['audio_policy']=audio_policy(state);result['audio_policy_proven']=True;result['physical_audio_capture_proven']=False
        def playback_observation(value):
            fields=('configured','disposed','hidden','playing','position','duration','decoded','dropped','video_width','video_height','video_paused','audio_paused','audio_position','skew','volume','muted','default_sink','quality_count','quality_index')
            return {k:v for k,v in value.items() if k in fields and (type(v) is bool or type(v) in (int,float) and math.isfinite(v))}
        def stalled(value):
            # Remains an acceptance failure even if buffering recovers. This
            # bounded read-only trace distinguishes recovery from a dead player.
            result['stall_observations']=[playback_observation(value)]
            until=time.monotonic()+15
            while time.monotonic()<until:
                time.sleep(.5);current=snapshot(mode)
                result['stall_observations'].append(playback_observation(current))
                if current.get('playing') and current.get('position',0)>value.get('position',0)+1:
                    result['resumed_without_input']=True;break
            raise Blocked('public_playback_stalled')
        result['stage']='warmup'
        warmup_to_segment(lambda:snapshot(mode),args,result)
        result['preparation_wall_seconds']=time.monotonic()-started
        result['stage']='measurement';sampler=Sampler(device.app.pid,out/('arm-'+str(index)+'-samples.jsonl'),args.memory_cap_mib*1024*1024);sampler.started=time.monotonic()
        samples=[];video=[];stable_identities=None;route_generation=None;began=time.monotonic()
        for step in range(19):
            delay=began+step*5-time.monotonic()
            if delay>0:time.sleep(delay)
            state=snapshot(mode);video.append(video_state(state,args))
            result['last_player_state']=playback_observation(state)
            if not state.get('playing'):stalled(state)
            members=native_members(client,identifier)
            session=rows(client,identifier);selected=next((v for v in session['vkladki'] if v['id']==tab),None)
            if not selected or selected.get('route_pending') or session['aktivnaya']!=tab:raise Blocked('route_or_selected_tab_changed')
            generation=selected.get('route_generation')
            if type(generation)is not int:raise Blocked('route_generation_unavailable')
            if route_generation is None:route_generation=generation
            elif route_generation!=generation:raise Blocked('route_generation_changed')
            clock='windows_filetime' if sys.platform=='win32' else 'linux_ticks'
            identities={(m['pid'],m['birth']) for m in members}
            owner=psutil.Process(device.app.pid)
            for process in [owner]+owner.children(recursive=True):identities.add((process.pid,kernel_birth(process.pid,clock)))
            if stable_identities is None:stable_identities=identities
            elif identities!=stable_identities:raise Blocked('process_churn_prevents_complete_cpu_accounting')

            with sampler.guard:sampler.native_members=members;sampler.native_members_complete=True;sampler.native_members_at=time.monotonic()
            samples.append(sampler.sample())
        # Sampler.sample is synchronous here; avoid a competing unrefreshed
        # thread. Persist only its existing safe OS-metrics record format.
        sampler.destination.write_text(''.join(json.dumps(v)+'\n' for v in samples));result['metrics']=metrics(samples,video,args);result['route_generation']=route_generation
        result['first_video_state']=video[0];result['last_video_state']=video[-1];result['measurement_samples']=len(samples)
        if decode_state(snapshot('decode'))!=decode:raise Blocked('decoder_changed')
        (close_owned or close_for_privacy)(client,device,result)
        result['stage']='complete';return result
    except Exception as error:
        result['blocked_reason']=str(error) if isinstance(error,Blocked) else 'native_acceptance_unavailable'
        result['failure_class']=type(error).__name__;return result
    finally:
        cleanup=True
        for obj in (device,wm):
            if obj:
                try:obj.close()
                except Exception:cleanup=False
        if private_parent and result.get('stage')!='complete':result['private_diagnostics_retained']=True
        else:
            try:shutil.rmtree(root)
            except OSError:cleanup=False
        result['cleanup_completed']=cleanup
        if not cleanup:result['stage']='cleanup';result['blocked_reason']='cleanup_failed'


def summarize_comparison(report,args):
    if [a.get('mode') for a in report['arms']]!=list(ORDER):raise Blocked('four_ordered_arms_required')
    if not all(a.get('stage')=='complete' and a.get('cleanup_completed') is True and a.get('owned_exit',{}).get('parent_normal_exit') is True and a['owned_exit'].get('all_known_native_executions_exited') is True for a in report['arms']):raise Blocked('normal_owned_exit_unproven')
    decoders=[a['decode'] for a in report['arms']]
    if any(d!=decoders[0] for d in decoders):raise Blocked('codec_or_software_decoder_mismatch')
    starts=[a['metrics']['measurement_start_position'] for a in report['arms']]
    if min(starts)<MEASUREMENT_START_POSITION or max(starts)>MEASUREMENT_START_POSITION+2 or max(starts)-min(starts)>2:raise Blocked('same_content_playback_segment_unproven')
    observed=[a['metrics']['observed_fps'] for a in report['arms']]
    if max(observed)-min(observed)>max(1,args.expected_fps*.05):raise Blocked('paired_observed_fps_mismatch')
    grouped={mode:[a['metrics'] for a in report['arms'] if a['mode']==mode] for mode in ('light','normal')}
    summary={mode:{key:statistics.median(r[key] for r in values) for key in ('cpu_core_percent','peak_private_bytes','peak_rss_bytes','dropped_ratio','observed_fps')} for mode,values in grouped.items()}
    if summary['normal']['cpu_core_percent']<=0 or summary['normal']['peak_private_bytes']<=0:raise Blocked('baseline_metrics_invalid')
    cpu=summary['light']['cpu_core_percent']/summary['normal']['cpu_core_percent'];memory=summary['light']['peak_private_bytes']/summary['normal']['peak_private_bytes']
    if not all(a.get('audio_policy_proven') is True and a.get('audio_policy')=={'volume':1,'muted':False,'default_sink':True} for a in report['arms']):raise Blocked('audio_output_policy_unproven')
    report['provenance']['audio_output_policy_match_proven']=True
    report.update(completed=True,measurement_valid=True,comparison_valid=True,accepted_measurement=True,
                  diagnostic_only=False,acceptance_passed=True,audio_policy_proven=True,physical_audio_capture_proven=False,
                  audio_evidence_scope='actual_element_volume_unmuted_default_sink_policy_not_physical_speaker_or_PCM_proof',summary=summary,cpu_ratio=cpu,private_ratio=memory,
                  improvement_observed=cpu<1 and memory<1,
                  drop_regression_observed=summary['light']['dropped_ratio']>summary['normal']['dropped_ratio']+.01,
                  claim_scope='observed_software_same_vm_four_arms_audio_policy_matched_not_general_hardware_performance')


def run(args):
    require(os.environ.get('GITHUB_ACTIONS')=='true','Real GHA runner required')
    if not re.fullmatch('[A-Za-z0-9_-]{11}',args.video_id) or not math.isfinite(args.expected_duration) or not 180<=args.expected_duration<=604800 or not 1<=args.expected_fps<=120 or not 1<=args.width<=16384 or not 1<=args.height<=16384 or not 256<=args.memory_cap_mib<=4096:raise Blocked('experiment_arguments_invalid')
    proxy=route_file(args.route_config);out=Path(args.out).resolve();out.mkdir(parents=True,exist_ok=False)
    manifest=Path(args.components_manifest)
    if manifest.is_symlink() or not manifest.is_file() or manifest.stat().st_size>131072:raise Blocked('component_manifest_input_invalid')
    manifest_sha=digest(manifest)
    exe=Path(args.exe).resolve();cef=Path(args.cef).resolve()/('libcef.dll' if sys.platform=='win32' else 'libcef.so')
    provenance={'component_manifest_input_sha256':manifest_sha,'exe_sha256':digest(exe),'cef_sha256':digest(cef),'system':platform.system(),'machine':platform.machine(),'logical_cpus':psutil.cpu_count(),'video_id':args.video_id,'route_mode':'proxy' if proxy else 'direct','hardware_acceleration_setting':False,'audio_output_policy_match_proven':False,'physical_audio_capture_proven':False,'normal_guard_scope':'observed_ad_markers_and_public_player_id_not_universal_ad_proof','order':list(ORDER),'warmup_seconds':30,'min_continuous_stable_seconds':30,'warmup_max_elapsed_seconds':90,'measurement_start_content_position':60,'measurement_seconds':90,'sample_period_seconds':5,'private_metric':'windows_private_commit' if sys.platform=='win32' else 'linux_uss'}
    report={'schema':1,'completed':False,'measurement_valid':False,'comparison_valid':False,'accepted_measurement':False,'diagnostic_only':True,'acceptance_passed':False,'fixture_used':False,'provenance':provenance,'arms':[]}
    try:
        for index,mode in enumerate(ORDER):
            if digest(exe)!=provenance['exe_sha256'] or digest(cef)!=provenance['cef_sha256'] or digest(manifest)!=manifest_sha:raise Blocked('same_binary_changed')
            result=arm(args,proxy,mode,index,out);report['arms'].append(result)
            if result['stage']!='complete' or not result.get('cleanup_completed') or not result.get('owned_exit',{}).get('parent_normal_exit'):raise Blocked(result.get('blocked_reason','normal_owned_exit_unproven'))
        summarize_comparison(report,args)
    except Exception as error:report['blocked_reason']=str(error) if isinstance(error,Blocked) else 'paired_experiment_unavailable';report['failure_class']=type(error).__name__
    (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    # Honest policy-matched measurements succeed even without a resource win.
    # Missing/mismatched native audio fields block before measurement acceptance.
    return 0 if report['measurement_valid'] else 1


def self_test():
    with tempfile.TemporaryDirectory() as directory:
        path=Path(directory)/'route.json';path.write_text('{"mode":"direct"}');require(route_file(path)=='','Direct route parsing failed')
        path.write_text('{"mode":"direct","mode":"proxy"}')
        try:route_file(path)
        except Blocked:pass
        else:raise AssertionError('Duplicate route keys accepted')
    require(audio_policy({'volume':1,'muted':False,'default_sink':True})=={'volume':1,'muted':False,'default_sink':True},'Audio policy parsing failed')
    for policy in ({},{'volume':1,'muted':True,'default_sink':True},{'volume':.5,'muted':False,'default_sink':True},{'volume':1,'muted':False,'default_sink':False}):
        try:audio_policy(policy)
        except Blocked:pass
        else:raise AssertionError('Unknown or mismatched audio policy accepted')
    try:decode_state({'codec':'vp9','decoder':'vpx','platform_decoder':True,'decode_mode':'platform','hardware_activity_proven':False,'video_pipelines':1})
    except Blocked:pass
    else:raise AssertionError('Platform comparison accepted')
    return {'self_test':True,'source_only':True,'native_execution':False}


if __name__=='__main__':
    if sys.argv[1:]==['--self-test']:print(json.dumps(self_test()));raise SystemExit(0)
    parser=argparse.ArgumentParser()
    for key in ('exe','cef','components-manifest','route-config','video-id','out'):parser.add_argument('--'+key,required=True)
    parser.add_argument('--expected-duration',type=float,required=True);parser.add_argument('--expected-fps',type=float,required=True);parser.add_argument('--width',type=int,required=True);parser.add_argument('--height',type=int,required=True);parser.add_argument('--memory-cap-mib',type=int,default=1024)
    raise SystemExit(run(parser.parse_args()))
