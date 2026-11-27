"""Pure publication contract tests: no GitHub calls, no tags/releases created."""
import pathlib,tempfile,unittest
from types import SimpleNamespace
from unittest.mock import patch
from prepare_release_draft import ensure_draft,validate_resource,validate_paired,optional_api,validate_native_jobs,NATIVE_REQUIRED,validate_runtime_bindings,validate_light_binary,release_gates,validate_functional_playback
SOURCE='a'*40
class Evidence(unittest.TestCase):
 def test_both_native_inputs_and_short_light_binary_are_bound(self):
  validate_runtime_bindings('123','123','123')
  for linux,windows in [('123','122'),('122','123'),('123',None)]:
   with self.subTest(linux=linux,windows=windows),self.assertRaises(AssertionError):validate_runtime_bindings('123',linux,windows)
  validate_light_binary({'binary_sha256':'b'*64},'b'*64)
  for light in ({},{'binary_sha256':'c'*64}):
   with self.subTest(light=light),self.assertRaises(AssertionError):validate_light_binary(light,'b'*64)

 def test_successful_job_without_new_native_steps_never_accepts(self):
  jobs=[dict(name=name,status='completed',conclusion='success',steps=[dict(name=title,status='completed',conclusion='success') for title in titles]) for name,titles in NATIVE_REQUIRED.items()]
  validate_native_jobs(jobs)
  import copy
  missing=copy.deepcopy(jobs);missing[0]['steps'].pop(2)
  with self.assertRaisesRegex(AssertionError,'Mandatory native check'):validate_native_jobs(missing)
  skipped=copy.deepcopy(jobs);skipped[0]['steps'][2]['conclusion']='skipped'
  with self.assertRaisesRegex(AssertionError,'Mandatory native check'):validate_native_jobs(skipped)
 def functional_fixture(self):
  state={'position':0.25,'duration':1343.6,'decoded':10,'dropped':0,'video_width':1280,'video_height':720,'playing':True}
  last={'position':120.6,'duration':1343.6,'decoded':3627,'dropped':0,'video_width':1280,'video_height':720,'playing':True}
  return {'desktop_source':SOURCE,'producer_run':123,'binary_sha256':'b'*64,
   'performance_comparison_accepted':False,'quality_selection':'default_vp9',
   'functional_playback':{'completed':True,'played_media_seconds':120.35,'wall_seconds':134.8,
    'buffer_events':[{'position':15.6,'duration_seconds':3.6,'resumed_without_input':True}],
    'first_state':state,'last_state':last},
   'decode':{'codec':'vp9','decoder':'vpx','platform_decoder':False,'decode_mode':'software','hardware_activity_proven':False,'video_pipelines':1},
   'owned_exit':{k:True for k in ('parent_normal_exit','all_known_native_executions_exited','actual_app_identity_bound','actual_app_execution_exited','launcher_exit_zero')},
   'cleanup_completed':True}
 def test_release_gates_default_is_strict_full(self):
  self.assertEqual(release_gates({}),('full',True,True))
  self.assertEqual(release_gates({'release_policy':{'mode':'functional','performance_claim':False,'memory_process_changed':False}}),('functional',False,False))
  self.assertEqual(release_gates({'release_policy':{'mode':'functional','performance_claim':False,'memory_process_changed':True}}),('functional',False,True))
  self.assertEqual(release_gates({'release_policy':{'mode':'functional','performance_claim':True,'memory_process_changed':False}}),('functional',True,False))
  self.assertEqual(release_gates({'release_policy':{'mode':'full','performance_claim':False,'memory_process_changed':False}}),('full',True,True))
  for bad in ({'release_policy':'functional'},{'release_policy':{'mode':'x','performance_claim':False,'memory_process_changed':False}},{'release_policy':{'mode':'functional','performance_claim':'no','memory_process_changed':False}}):
   with self.subTest(bad=bad),self.assertRaises(AssertionError):release_gates(bad)
 def test_functional_playback_good_payload_passes(self):
  validate_functional_playback(self.functional_fixture(),SOURCE,'123','b'*64)
 def test_functional_playback_refuses_short_or_unbounded(self):
  import copy,math
  good=self.functional_fixture()
  cases=[]
  short=copy.deepcopy(good);short['functional_playback']['played_media_seconds']=119.9;cases.append(short)
  long_wall=copy.deepcopy(good);long_wall['functional_playback']['wall_seconds']=181.5;cases.append(long_wall)
  nan_wall=copy.deepcopy(good);nan_wall['functional_playback']['wall_seconds']=float('nan');cases.append(nan_wall)
  nan_played=copy.deepcopy(good);nan_played['functional_playback']['played_media_seconds']=float('nan');cases.append(nan_played)
  long_stall=copy.deepcopy(good);long_stall['functional_playback']['buffer_events']=[{'position':1,'duration_seconds':20.1,'resumed_without_input':True}];cases.append(long_stall)
  nan_stall=copy.deepcopy(good);nan_stall['functional_playback']['buffer_events']=[{'position':1,'duration_seconds':float('nan'),'resumed_without_input':True}];cases.append(nan_stall)
  no_resume=copy.deepcopy(good);no_resume['functional_playback']['buffer_events']=[{'position':1,'duration_seconds':2,'resumed_without_input':False}];cases.append(no_resume)
  for bad in cases:
   with self.subTest(bad=bad['functional_playback']),self.assertRaises(AssertionError):validate_functional_playback(bad,SOURCE,'123','b'*64)
 def test_functional_playback_refuses_wrong_bindings_and_claims(self):
  import copy
  good=self.functional_fixture()
  cases=[]
  wrong_sha=copy.deepcopy(good);wrong_sha['binary_sha256']='c'*64;cases.append(wrong_sha)
  wrong_source=copy.deepcopy(good);wrong_source['desktop_source']='f'*40;cases.append(wrong_source)
  claimed=copy.deepcopy(good);claimed['performance_comparison_accepted']=True;cases.append(claimed)
  wrong_quality=copy.deepcopy(good);wrong_quality['quality_selection']='av1_manual';cases.append(wrong_quality)
  wrong_decode=copy.deepcopy(good);wrong_decode['decode']['decoder']='dav1d';cases.append(wrong_decode)
  no_exit=copy.deepcopy(good);no_exit['owned_exit']['launcher_exit_zero']=False;cases.append(no_exit)
  dirty=copy.deepcopy(good);dirty['cleanup_completed']=False;cases.append(dirty)
  mismatch=copy.deepcopy(good);mismatch['functional_playback']['played_media_seconds']=125.0;cases.append(mismatch)
  not_playing=copy.deepcopy(good);not_playing['functional_playback']['last_state']['playing']=False;cases.append(not_playing)
  frozen=copy.deepcopy(good);frozen['functional_playback']['last_state']['decoded']=10;cases.append(frozen)
  incomplete=copy.deepcopy(good);incomplete['functional_playback']['completed']=False;cases.append(incomplete)
  for bad in cases:
   with self.subTest(bad=bad),self.assertRaises(AssertionError):validate_functional_playback(bad,SOURCE,'123','b'*64)
 def test_missing_or_diagnostic_paired_blocks_release(self):
  good=dict(accepted=True,source=SOURCE,build_run='123',diagnostic_only=False,comparison_valid=True,measurement_valid=True,order=['light','normal','normal','light'],normal_owned_exit_all=True,same_video_quality_decoder=True,raw_report_sha256='d'*64)
  validate_paired(good,SOURCE,'123')
  for value in ({},dict(good,accepted=False),dict(good,diagnostic_only=True),dict(good,source='c'*40),dict(good,order=['light','normal']),dict(good,normal_owned_exit_all=False),dict(good,raw_report_sha256='')):
   with self.subTest(value=value),self.assertRaises(AssertionError):validate_paired(value,SOURCE,'123')
 def test_service_error_is_not_missing_reference(self):
  with patch('prepare_release_draft.subprocess.run',return_value=SimpleNamespace(returncode=1,stderr='gh: server failure (HTTP 500)',stdout='')):
   with self.assertRaises(RuntimeError):optional_api('repos/owner/repo/git/ref/tags/v0.2.27')
  with patch('prepare_release_draft.subprocess.run',return_value=SimpleNamespace(returncode=1,stderr='gh: Not Found (HTTP 404)',stdout='')):
   self.assertIsNone(optional_api('repos/owner/repo/git/ref/tags/v0.2.27'))

 def test_real_source_and_two_hours_required(self):
  value=dict(completed=True,acceptance_passed=True,hibernation_acceptance=True,baseline=False,diagnostic_pilot=False,source=SOURCE,build_run='123',test_source='b'*40,platform='win32',requested_seconds=7200,wall_seconds=7200.1,samples_count=1438)
  validate_resource(value,SOURCE,'123','b'*40)
  for key,bad in [('wall_seconds',7199.9),('source','c'*40),('diagnostic_pilot',True),('baseline',True),('samples_count',12),('acceptance_passed',False)]:
   with self.subTest(key=key),self.assertRaises(AssertionError):validate_resource(dict(value,**{key:bad}),SOURCE,'123','b'*40)
 def test_existing_public_release_never_written(self):
  with tempfile.TemporaryDirectory() as d:
   notes=pathlib.Path(d)/'notes';notes.write_text('notes');writes=[]
   def read(path):return {'object':{'type':'commit','sha':SOURCE}} if '/git/ref/' in path else {'draft':False}
   with self.assertRaises(AssertionError):ensure_draft('owner/repo','v0.2.27',SOURCE,notes,lambda *a:writes.append(a),read)
   self.assertEqual(writes,[])
 def test_existing_wrong_tag_never_retargeted(self):
  with tempfile.TemporaryDirectory() as d:
   notes=pathlib.Path(d)/'notes';notes.write_text('notes');writes=[]
   with self.assertRaises(AssertionError):ensure_draft('owner/repo','v0.2.27',SOURCE,notes,lambda *a:writes.append(a),lambda p:{'object':{'type':'commit','sha':'c'*40}})
   self.assertEqual(writes,[])
 def test_missing_candidate_becomes_only_a_draft(self):
  with tempfile.TemporaryDirectory() as d:
   notes=pathlib.Path(d)/'notes';notes.write_text('notes');state={};writes=[]
   def read(path):return state.get('ref') if '/git/ref/' in path else state.get('release')
   def invoke(*args):
    writes.append(args)
    if args[0]=='api':state['ref']={'object':{'type':'commit','sha':SOURCE}}
    else:state['release']={'draft':True}
   ensure_draft('owner/repo','v0.2.27',SOURCE,notes,invoke,read)
   self.assertEqual(len(writes),2);self.assertIn('--draft',writes[1]);self.assertNotIn('--draft=false',writes[1])
if __name__=='__main__':unittest.main()
