"""Metadata-only provenance regression tests; no network or binary downloads."""
import base64,json,unittest
from pathlib import Path
from unittest.mock import patch
from controlled_media_provider import verify
SOURCE='a'*40
class Provider(unittest.TestCase):
 def records(self):
  text=(Path(__file__).parents[1]/'.github/workflows/build.yml').read_text()
  return [
   {'head_repository':{'full_name':'hipocrisle/nexusbro'},'event':'workflow_dispatch','path':'.github/workflows/build.yml','head_sha':SOURCE},
   {'jobs':[{'name':'youtube-media-fixture','conclusion':'success'}]},
   {'artifacts':[{'id':42,'name':'youtube-controlled-media','expired':False,'digest':'sha256:'+'b'*64}]},
   {'encoding':'base64','content':base64.b64encode(text.encode()).decode()}]
 def call(self,records):
  with patch('controlled_media_provider.subprocess.check_output',side_effect=[json.dumps(v).encode() for v in records]):
   return verify('123','hipocrisle/nexusbro')
 def test_owned_successful_generation_is_reusable(self):
  self.assertTrue(self.call(self.records())['provenance_verified'])
 def test_foreign_source_and_incomplete_generation_are_refused(self):
  for field,value in [('head_repository',{'full_name':'foreign/repo'}),('event','pull_request'),('path','other.yml')]:
   r=self.records();r[0][field]=value
   with self.subTest(field=field),self.assertRaises(AssertionError):self.call(r)
  r=self.records();r[1]['jobs'][0]['conclusion']='cancelled'
  with self.assertRaisesRegex(AssertionError,'job_not_verified'):self.call(r)
 def test_changed_recipe_and_unverified_digest_are_refused(self):
  r=self.records();r[3]['content']=base64.b64encode(base64.b64decode(r[3]['content']).replace(b'rate=12',b'rate=13')).decode()
  with self.assertRaisesRegex(AssertionError,'recipe_changed'):self.call(r)
  r=self.records();r[2]['artifacts'][0]['digest']=''
  with self.assertRaisesRegex(AssertionError,'artifact_unverified'):self.call(r)
if __name__=='__main__':unittest.main()
