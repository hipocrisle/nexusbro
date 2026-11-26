#!/usr/bin/env python3
"""Explicit isolated Linux ABBA benchmark, using the same native measurements as CI.
No GHA spoofing, owner cookies, real desktop or sandbox override. The native
opt-in is an owned 0600 marker inside an owned 0700 test profile under ~/AI.
Unknown codec, quality, audio policy, process ownership or blocked YouTube fails.
"""
import argparse, hashlib, json, math, os, re, signal, sys, time
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parent))
from security_native import NativeFailure, require
from youtube_paired_native import ORDER, arm, Blocked, digest, summarize_comparison
from youtube_embed_home_probe import validate_private_root, validate_inside_root, SandboxedDevice, LocalWindowManager, local_close
from youtube_light_home_probe import sidecar_enum
TIPS={'light':'youtube-state','normal':'youtube-reference-state','decode':'youtube-decode-state'}
COMMANDS={'light':'youtube_snapshot','normal':'youtube_reference_snapshot','decode':'youtube_decode_snapshot'}
def make_state_probe(video_id):
    """state_probe(client, identifier, tab, kind) -> raw state dict.

    Preserves async youtube-result receipts (no buffer clearing), reads only
    new events by monotonic index, maps shipped failure strings only through
    sidecar_enum. Raises Blocked/NativeFailure like the shared probe()."""
    def state_probe(client, identifier, tab, kind):
        tip = TIPS[kind]
        params = {'deystvie': COMMANDS[kind], 'id': tab}
        if kind != 'light':
            params['video_id'] = video_id
        start = client.js('window.__paired.length')
        if type(start) is not int:
            raise Blocked('native_paired_buffer_unavailable')
        from youtube_paired_native import call
        call(client, 'obychnaya_komanda', id=identifier, komanda=params)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            data = client.js(
                '(()=>{const b=window.__paired;let best=-1;'
                'for(let i=' + str(start) + ';i<b.length;i++){const e=b[i];'
                'if(e&&e.super_id===' + json.dumps(identifier) + '&&e.data&&e.data.id===' + str(tab) +
                '&&e.data.tip===' + json.dumps(tip) + ')best=i;}'
                'if(best>=0){const d=b[best].data;if(b.length>512)b.splice(0,b.length-512);return d;}'
                'for(let i=' + str(start) + ';i<b.length;i++){const e=b[i];'
                'if(e&&e.super_id===' + json.dumps(identifier) + '&&e.data&&e.data.id===' + str(tab) +
                '&&e.data.tip==="youtube-result"){const d=e.data;if(b.length>512)b.splice(0,b.length-512);return d;}}'
                'return null;})()')
            if isinstance(data, dict):
                if data.get('tip') == 'youtube-result':
                    # start=false + error=null is SUCCESS completion, not failure.
                    if data.get('start') is False and data.get('error') is None:
                        time.sleep(.1)
                        continue
                    raise Blocked('native_light_' + sidecar_enum(data.get('error')))
                state = data.get('state')
                if isinstance(state, dict):
                    return state
                code=data.get('code')
                if isinstance(code,str) and re.fullmatch('[a-z][a-z0-9_]{0,95}',code):raise Blocked('native_'+kind+'_'+code)
                raise Blocked('native_' + kind + '_unavailable')
            time.sleep(.1)
        raise Blocked('native_' + kind + '_unavailable')
    return state_probe



class BenchmarkDevice(SandboxedDevice):
    def __init__(self, root, exe, cef):
        super().__init__('paired-local',root,exe,cef)
        self.data.chmod(0o700)
        marker=self.data/'.youtube-benchmark-opt-in'
        with marker.open('xb') as stream:stream.write(b'nexusbro-local-youtube-benchmark-v1\n')
        marker.chmod(0o600)


def run(args):
    require(sys.platform=='linux' and os.getuid()==os.geteuid()==args.expect_uid and args.expect_uid!=0,'Local uid/platform invalid')
    require(os.environ.get('GITHUB_ACTIONS')!='true','Local benchmark forbids GHA spoofing')
    require(not os.environ.get('WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS'),'Local benchmark preserves sandbox')
    require(args.allow_local_virtual_display,'Explicit private virtual-display consent required')
    root=validate_private_root(args.private_root,args.expect_uid)
    exe=validate_inside_root(args.exe,root,'exe');cef=validate_inside_root(args.cef,root,'cef')
    require(digest(exe)==args.exe_sha256,'Candidate binary mismatch')
    require(len(args.desktop_source)==40 and all(c in '0123456789abcdef' for c in args.desktop_source),'Source invalid')
    require(args.video_id=='M7lc1UVf-VE','Only fixed public sample admitted')
    require(math.isfinite(args.expected_duration) and args.expected_duration>=180 and 1<=args.expected_fps<=120,'Expected media metadata invalid')
    require(1<=args.width<=16384 and 1<=args.height<=16384 and 256<=args.memory_cap_mib<=4096,'Geometry or memory budget invalid')
    out=root/'out'/('paired-'+str(time.time_ns()));out.mkdir(parents=True,mode=0o700)
    parent=out/'arms';parent.mkdir(mode=0o700)
    report={'schema':1,'completed':False,'measurement_valid':False,'comparison_valid':False,'accepted_measurement':False,'diagnostic_only':True,'acceptance_passed':False,'fixture_used':False,'arms':[],
      'desktop_source':args.desktop_source,'producer_run':args.producer_run,'sandbox_preserved':True,'local_execution':True,
      'provenance':{'exe_sha256':digest(exe),'cef_sha256':digest(cef/'libcef.so'),'video_id':args.video_id,'expected_width':args.width,'expected_height':args.height,'expected_duration':args.expected_duration,'expected_fps':args.expected_fps,'route_mode':'direct','order':list(ORDER),'hardware_acceleration_setting':False,'audio_output_policy_match_proven':False,'physical_audio_capture_proven':False,'warmup_seconds':30,'measurement_seconds':90,'sample_period_seconds':5,'private_metric':'linux_uss'}}
    began=time.monotonic()
    def expired(signum,frame):raise Blocked('bounded_arm_deadline')
    previous=signal.signal(signal.SIGALRM,expired)
    try:
        for index,mode in enumerate(ORDER):
            require(digest(exe)==args.exe_sha256 and digest(cef/'libcef.so')==report['provenance']['cef_sha256'],'Same binary changed')
            signal.alarm(330)
            try:
                result=arm(args,'',mode,index,out,device_factory=BenchmarkDevice,close_owned=local_close,private_parent=parent,window_manager_factory=LocalWindowManager,state_probe=make_state_probe(args.video_id))
            finally:signal.alarm(0)
            report['arms'].append(result)
            if result.get('stage')!='complete' or result.get('cleanup_completed') is not True:raise Blocked(result.get('blocked_reason','arm_incomplete'))
        summarize_comparison(report,args)
        report['provenance']['audio_output_policy_match_proven']=True
    except Exception as error:
        report['blocked_reason']=str(error) if isinstance(error,Blocked) else 'local_paired_unavailable'
        report['failure_class']=type(error).__name__
    finally:
        signal.signal(signal.SIGALRM,previous)
        report['wall_seconds']=round(time.monotonic()-began,3)
        (out/'result.json').write_text(json.dumps(report,indent=2,allow_nan=False)+'\n')
    print(json.dumps({'completed':report['completed'],'comparison_valid':report['comparison_valid'],'blocked_reason':report.get('blocked_reason'),'arms':len(report['arms'])}))
    return 0 if report['accepted_measurement'] else 1

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('private-root','exe','cef','exe-sha256','desktop-source'):p.add_argument('--'+name,required=True)
    p.add_argument('--producer-run',type=int,required=True);p.add_argument('--expect-uid',type=int,required=True)
    p.add_argument('--allow-local-virtual-display',action='store_true');p.add_argument('--video-id',default='M7lc1UVf-VE')
    p.add_argument('--expected-duration',type=float,default=1343.6);p.add_argument('--expected-fps',type=float,default=30)
    p.add_argument('--width',type=int,default=1280);p.add_argument('--height',type=int,default=720);p.add_argument('--memory-cap-mib',type=int,default=1024)
    raise SystemExit(run(p.parse_args()))
