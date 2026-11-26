"""Reject plausible but incomplete ABBA reports before granting acceptance."""
import copy,sys,unittest
from pathlib import Path
from types import SimpleNamespace
sys.path.insert(0,str(Path(__file__).parent/'gui'))
from youtube_paired_native import summarize_comparison,Blocked,ORDER
class Comparison(unittest.TestCase):
 def report(self):
  arms=[]
  for mode in ORDER:
   arms.append(dict(mode=mode,stage='complete',cleanup_completed=True,owned_exit={'parent_normal_exit':True,'all_known_native_executions_exited':True},decode={'codec':'vp9','decoder':'vpx','decode_mode':'software'},audio_policy_proven=True,audio_policy={'volume':1,'muted':False,'default_sink':True},metrics={'measurement_start_position':31,'cpu_core_percent':10,'peak_private_bytes':100,'peak_rss_bytes':140,'dropped_ratio':0,'observed_fps':30}))
  return {'provenance':{},'arms':arms}
 def test_complete_comparison_can_accept_without_improvement(self):
  r=self.report();summarize_comparison(r,SimpleNamespace(expected_fps=30))
  self.assertTrue(r['accepted_measurement']);self.assertFalse(r['improvement_observed'])
 def test_missing_or_mismatched_evidence_blocks(self):
  cases=[]
  r=self.report();r['arms'].pop();cases.append(r)
  r=self.report();r['arms'][1]['mode']='light';cases.append(r)
  r=self.report();r['arms'][1]['decode']['codec']='h264';cases.append(r)
  r=self.report();r['arms'][1]['owned_exit']['all_known_native_executions_exited']=False;cases.append(r)
  r=self.report();r['arms'][1]['cleanup_completed']=False;cases.append(r)
  r=self.report();r['arms'][1]['metrics']['measurement_start_position']=44;cases.append(r)
  r=self.report();r['arms'][1]['audio_policy']['muted']=True;cases.append(r)
  for r in cases:
   with self.subTest(report=r),self.assertRaises(Blocked):summarize_comparison(r,SimpleNamespace(expected_fps=30))
if __name__=='__main__':unittest.main()
