"""Actual functional observer protocol; no browser or network."""
import sys,unittest
from pathlib import Path
from types import SimpleNamespace
sys.path.insert(0,str(Path(__file__).parent/'gui'))
from youtube_paired_native import functional_playback_window,Blocked
class Protocol(unittest.TestCase):
 args=SimpleNamespace(expected_duration=1343.6,width=1280,height=720)
 def run_window(self,position,playing=lambda t:True):
  clock=[0.];result={}
  def sleep(t):clock[0]+=t
  def sample():
   p=position(clock[0]);return dict(position=p,duration=1343.6,decoded=int(p*30),dropped=0,video_width=1280,video_height=720,playing=playing(clock[0]),volume=1,muted=False,default_sink=True)
  functional_playback_window(sample,self.args,result,clock=lambda:clock[0],sleep=sleep)
  return result
 def test_actual_two_minutes_required(self):
  r=self.run_window(lambda t:t)['functional_playback'];self.assertTrue(r['completed']);self.assertEqual(r['played_media_seconds'],120)
 def test_recovered_buffer_is_reported(self):
  r=self.run_window(lambda t:t if t<40 else 40 if t<43 else t-3,lambda t:not 40<=t<43)['functional_playback']
  self.assertTrue(r['buffer_events'][0]['resumed_without_input']);self.assertGreaterEqual(r['wall_seconds'],123)
 def test_dead_player_never_passes(self):
  with self.assertRaisesRegex(Blocked,'stall_deadline'):self.run_window(lambda t:0,lambda t:False)
 def test_forward_seek_never_substitutes_for_viewing(self):
  with self.assertRaisesRegex(Blocked,'clock_discontinuity'):self.run_window(lambda t:t+(50 if t>=2 else 0))
if __name__=='__main__':unittest.main()
