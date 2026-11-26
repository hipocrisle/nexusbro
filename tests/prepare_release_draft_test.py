"""Pure publication contract tests: no GitHub calls, no tags/releases created."""
import pathlib,tempfile,unittest
from types import SimpleNamespace
from unittest.mock import patch
from prepare_release_draft import ensure_draft,validate_resource,validate_paired,optional_api,validate_native_jobs,NATIVE_REQUIRED,validate_runtime_bindings,validate_light_binary
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
