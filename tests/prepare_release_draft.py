"""CI-only: wait for actual candidate evidence, then prepare an immutable draft.

The workflow supplies its repository GITHUB_TOKEN, preventing a recursive tag
build. This helper never publishes or edits an existing public release.
"""
import hashlib,json,math,os,pathlib,re,subprocess,tempfile,time


def gh(*args):
    result=subprocess.run(['gh',*map(str,args)],capture_output=True,text=True)
    if result.returncode:raise RuntimeError('GitHub request failed (exit '+str(result.returncode)+')')
    return result.stdout


def optional_api(path):
    result=subprocess.run(['gh','api',path],capture_output=True,text=True)
    if result.returncode:
        if '(HTTP 404)' in result.stderr:return None
        raise RuntimeError('GitHub optional read failed; not treated as missing')
    return json.loads(result.stdout)


def validate_resource(value,source,build,test_source):
    assert all(value.get(k) is True for k in ('completed','acceptance_passed','hibernation_acceptance')),'Resource acceptance incomplete'
    assert value.get('baseline') is False and value.get('diagnostic_pilot') is False,'Resource diagnostic cannot substitute for acceptance'
    assert value.get('source')==source and str(value.get('build_run'))==build and value.get('test_source')==test_source,'Resource source mismatch'
    assert value.get('platform')=='win32' and value.get('requested_seconds')==7200,'Wrong resource platform or duration'
    elapsed=value.get('wall_seconds');assert type(elapsed) in (int,float) and math.isfinite(elapsed) and elapsed>=7200,'Two real hours required'
    assert type(value.get('samples_count')) is int and value['samples_count']>=1437,'Incomplete resource samples'



def validate_paired(value, source, build):
    """An incomplete or diagnostic comparison must block all release writes."""
    assert value.get('accepted') is True, 'Mandatory paired YouTube measurement missing'
    assert value.get('source') == source and str(value.get('build_run')) == build, 'Paired candidate mismatch'
    assert value.get('diagnostic_only') is False, 'Diagnostic playback is not paired acceptance'
    assert value.get('comparison_valid') is True and value.get('measurement_valid') is True, 'Paired comparison incomplete'
    assert value.get('order') == ['light', 'normal', 'normal', 'light'], 'Four paired arms required'
    assert value.get('normal_owned_exit_all') is True, 'Paired process cleanup unproven'
    assert value.get('same_video_quality_decoder') is True, 'Paired playback conditions unproven'
    assert value.get('raw_report_sha256') and re.fullmatch('[0-9a-f]{64}', value['raw_report_sha256']), 'Paired evidence digest missing'



def validate_paired_report(value, source, build):
    assert all(value.get(k) is True for k in ('completed','measurement_valid','comparison_valid','accepted_measurement','acceptance_passed')), 'Raw paired report incomplete'
    assert value.get('diagnostic_only') is False and value.get('fixture_used') is False, 'Raw diagnostic cannot substitute for public playback'
    assert value.get('desktop_source')==source and str(value.get('producer_run'))==build, 'Raw paired source mismatch'
    provenance=value.get('provenance',{})
    assert provenance.get('order')==['light','normal','normal','light'] and provenance.get('video_id')=='M7lc1UVf-VE', 'Wrong paired protocol'
    assert provenance.get('warmup_seconds')==30 and provenance.get('measurement_seconds')==90 and provenance.get('sample_period_seconds')==5, 'Paired windows shortened'
    assert provenance.get('min_continuous_stable_seconds')==30 and provenance.get('warmup_max_elapsed_seconds')==90 and provenance.get('measurement_start_content_position')==60, 'Fixed segment warmup protocol missing'
    assert provenance.get('audio_output_policy_match_proven') is True, 'Paired audio policy unknown'
    assert re.fullmatch('[0-9a-f]{64}',provenance.get('exe_sha256','')), 'Paired binary digest missing'
    assert provenance.get('expected_width')==1280 and provenance.get('expected_height')==720, 'Paired quality differs from declared 720p protocol'
    duration=provenance.get('expected_duration');fps=provenance.get('expected_fps')
    assert type(duration) in (int,float) and math.isfinite(duration) and duration>=180 and type(fps) in (int,float) and math.isfinite(fps) and 1<=fps<=120, 'Paired media metadata missing'
    arms=value.get('arms',[])
    assert [a.get('mode') for a in arms]==provenance['order'], 'Missing paired arms'
    decoder=arms[0].get('decode');assert isinstance(decoder,dict) and decoder.get('decode_mode')=='software' and decoder.get('platform_decoder') is False, 'Paired decoder unknown'
    starts=[]
    for arm in arms:
        assert arm.get('warmup_continuous_seconds',0)>=30 and arm.get('warmup_continuous_media_seconds',0)>=30 and 30<=arm.get('warmup_wall_seconds',0)<=90, 'Continuous warmup incomplete'
        assert type(arm.get('preparation_wall_seconds')) in (int,float) and math.isfinite(arm['preparation_wall_seconds']) and arm['preparation_wall_seconds']>=arm['warmup_wall_seconds'], 'Preparation cost omitted'
        assert arm.get('stage')=='complete' and arm.get('cleanup_completed') is True and arm.get('decode')==decoder, 'Paired arm incomplete or decoder mismatch'
        exit=arm.get('owned_exit',{})
        assert exit.get('parent_normal_exit') is True and exit.get('all_known_native_executions_exited') is True, 'Paired exit incomplete'
        assert arm.get('audio_policy_proven') is True and arm.get('audio_policy')=={'volume':1,'muted':False,'default_sink':True}, 'Paired audio mismatch'
        metrics=arm.get('metrics',{})
        for key in ('measurement_seconds','media_seconds','measurement_start_position','measurement_end_position','observed_fps','cpu_core_percent','peak_private_bytes','peak_rss_bytes','dropped_ratio'):
            v=metrics.get(key);assert type(v) in (int,float) and math.isfinite(v) and v>=0, 'Paired metric absent or invalid'
        assert metrics['measurement_seconds']>=89 and metrics['media_seconds']>=89, 'Paired measurement too short'
        assert type(arm.get('measurement_samples')) is int and arm['measurement_samples']>=19, 'Paired samples missing'
        assert abs(metrics['observed_fps']-fps)<=max(2,fps*.1), 'Paired frame rate mismatch'
        first,last=arm.get('first_video_state',{}),arm.get('last_video_state',{})
        for snapshot in (first,last):
            assert snapshot.get('video_width')==1280 and snapshot.get('video_height')==720 and snapshot.get('playing') is True, 'Actual paired geometry or playback mismatch'
            assert type(snapshot.get('duration')) in (int,float) and math.isfinite(snapshot['duration']) and abs(snapshot['duration']-duration)<=2, 'Actual paired duration mismatch'
        assert last.get('decoded',0)>first.get('decoded',0) and last.get('position',0)-first.get('position',0)>=89, 'Paired frame/clock advancement missing'
        starts.append(metrics['measurement_start_position'])
    assert min(starts)>=60 and max(starts)<=62 and max(starts)-min(starts)<=2, 'Paired video sections differ'


NATIVE_REQUIRED={
 'runtime':('Extract installer and check actual Windows screen','Native Windows media playback lock and normal exit','Native Windows saved-download open and folder actions','Native Windows per-tab routes and trusted HTTPS'),
 'Native Linux from existing matching binary':('Verify native media and normal owned exit with typed diagnostics','Verify ordinary download status and saved-file actions','Native per-tab routes, isolated profiles and trusted HTTPS','Native encrypted vault, UI lock and two-device sync','Native browser essentials from exact existing binary'),
}
def validate_native_jobs(jobs):
    for name,required in NATIVE_REQUIRED.items():
        matches=[j for j in jobs if j.get('name')==name]
        assert len(matches)==1 and matches[0].get('status')=='completed' and matches[0].get('conclusion')=='success', 'Fresh native job incomplete: '+name
        for title in required:
            steps=[s for s in matches[0].get('steps',[]) if s.get('name')==title]
            assert len(steps)==1 and steps[0].get('status')=='completed' and steps[0].get('conclusion')=='success', 'Mandatory native check missing or skipped: '+title


def ensure_draft(repo,tag,source,notes,invoke=gh,read=optional_api):
    assert re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+',repo) and re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+',tag)
    assert re.fullmatch(r'[0-9a-f]{40}',source) and pathlib.Path(notes).is_file()
    ref=read(f'repos/{repo}/git/ref/tags/{tag}')
    if ref is not None:assert ref.get('object',{}).get('type')=='commit' and ref['object'].get('sha')==source,'Immutable tag differs from accepted build'
    release=read(f'repos/{repo}/releases/tags/{tag}')
    if release is not None:assert release.get('draft') is True,'Refuse replacing a public release'
    if ref is None:
        invoke('api','--method','POST',f'repos/{repo}/git/refs','-f','ref=refs/tags/'+tag,'-f','sha='+source)
        ref=read(f'repos/{repo}/git/ref/tags/{tag}')
        assert ref and ref.get('object',{}).get('type')=='commit' and ref['object'].get('sha')==source,'Created tag identity unconfirmed'
    if release is None:
        invoke('release','create',tag,'--repo',repo,'--verify-tag','--target',source,'--title','NexusBro '+tag[1:],'--draft','--notes-file',str(notes))
    release=read(f'repos/{repo}/releases/tags/{tag}')
    assert release and release.get('draft') is True,'Draft identity unconfirmed'


def main():
    assert os.environ.get('GITHUB_ACTIONS')=='true' and os.environ.get('GITHUB_EVENT_NAME')=='workflow_dispatch','Preparation is CI-only'
    repo=os.environ['REPO'];tag=os.environ['TAG'];build=os.environ['BUILD_RUN']
    assert repo==os.environ['GITHUB_REPOSITORY'] and re.fullmatch(r'[1-9][0-9]*',build)
    assert os.environ.get('LINUX_RUNTIME_BUILD')==build and os.environ.get('LINUX_RUNTIME_RESULT')=='success','Fresh exact-source Linux acceptance required'
    version=tag.removeprefix('v');assert re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+',tag)
    assert json.loads(pathlib.Path('desktop/src-tauri/tauri.conf.json').read_text())['version']==version,'Candidate version mismatch'
    state=json.loads(pathlib.Path('docs/verification',version,'acceptance-state.json').read_text())
    metadata=json.loads(gh('run','view',build,'--repo',repo,'--json','status,headSha'))
    assert metadata['status']=='completed' and state['candidate']['source']==metadata['headSha'] and str(state['candidate']['build_run'])==build
    source=metadata['headSha']
    current=os.environ['GITHUB_RUN_ID'];assert re.fullmatch('[1-9][0-9]*',current)
    native=json.loads(gh('api',f'repos/{repo}/actions/runs/{current}/jobs?filter=latest&per_page=100'))
    assert native.get('total_count',101)<=100, 'Native job listing incomplete'
    validate_native_jobs(native['jobs'])
    validate_paired(state.get('paired_public_measurement',{}),source,build)
    paired_path=pathlib.Path('docs/verification',version,'paired-home-result.json')
    assert paired_path.is_file() and not paired_path.is_symlink() and paired_path.stat().st_size<=1024*1024, 'Raw paired report missing'
    paired_bytes=paired_path.read_bytes()
    assert hashlib.sha256(paired_bytes).hexdigest()==state['paired_public_measurement']['raw_report_sha256'], 'Raw paired evidence digest mismatch'
    validate_paired_report(json.loads(paired_bytes),source,build)
    # Bind the measured ELF to the actual producer artifact, not just a SHA
    # string supplied by the measurement report. Download only on the CI VM.
    paired_report=json.loads(paired_bytes)
    with tempfile.TemporaryDirectory(prefix='nb-paired-binary-') as directory:
        gh('run','download',build,'--repo',repo,'--name','nexusbro-linux-runtime','--dir',directory)
        binaries=[p for p in pathlib.Path(directory).rglob('*') if p.is_file()]
        assert len(binaries)==1 and binaries[0].name=='nexusbro' and binaries[0].stat().st_size<=128*1024*1024, 'Unexpected producer binary artifact'
        assert hashlib.sha256(binaries[0].read_bytes()).hexdigest()==paired_report['provenance']['exe_sha256'], 'Measured binary differs from actual build artifact'
    light=json.loads(pathlib.Path('docs/verification',version,'light-home-summary.json').read_text())
    assert light.get('desktop_source')==source and str(light.get('producer_run'))==build
    assert light.get('completed') is True and light.get('playing_observed') is True and light.get('extraction_completed') is True and light.get('decoded_frames_advanced') is True and light.get('position_delta_seconds',0)>=8
    delta=light['position_delta_seconds'];assert type(delta) in (int,float) and math.isfinite(delta) and delta>=8
    assert light['last_light_state']['decoded']>light['first_light_state']['decoded'] and light['last_light_state']['position']-light['first_light_state']['position']>=8
    assert light.get('owned_exit',{}).get('parent_normal_exit') is True and light['owned_exit'].get('all_known_native_executions_exited') is True,'Public light playback exit unproven'
    resource=str(state['resource_soak_pending']['run']);assert re.fullmatch('[1-9][0-9]*',resource)
    deadline=time.monotonic()+2700
    while True:
        record=json.loads(gh('run','view',resource,'--repo',repo,'--json','status,conclusion,headSha'))
        if record['status']=='completed':
            assert record['conclusion']=='success','Resource run did not pass';break
        assert time.monotonic()<deadline,'Resource acceptance wait expired'
        time.sleep(30)
    with tempfile.TemporaryDirectory(prefix='nb-release-resource-') as directory:
        gh('run','download',resource,'--repo',repo,'--name','resource-soak-windows-two-hours','--dir',directory)
        files=list(pathlib.Path(directory).rglob('results.json'));assert len(files)==1 and files[0].stat().st_size<=2*1024*1024
        value=json.loads(files[0].read_text());validate_resource(value,source,build,record['headSha'])
    out=pathlib.Path('.dev-check/publish');out.mkdir(parents=True,exist_ok=True)
    receipt={k:value[k] for k in ('source','test_source','build_run','platform','completed','acceptance_passed','hibernation_acceptance','wall_seconds','requested_seconds','samples_count','exe_sha256','package_sha256')}
    receipt['run']=resource;(out/'resource-acceptance.json').write_text(json.dumps(receipt,indent=2)+'\n')
    ensure_draft(repo,tag,source,pathlib.Path('docs','RELEASE-'+version+'.md'))
    print(json.dumps({'candidate_draft_prepared':True,'resource_acceptance_verified':True,'public_light_playback_evidence_verified':True,'published':False}))

if __name__=='__main__':main()
