"""CI-only: wait for actual candidate evidence, then prepare an immutable draft.

The workflow supplies its repository GITHUB_TOKEN, preventing a recursive tag
build. This helper never publishes or edits an existing public release.
"""
import json,math,os,pathlib,re,subprocess,tempfile,time


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
    validate_paired(state.get('paired_public_measurement',{}),source,build)
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
