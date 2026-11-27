"""Deterministic tests of the real fixed-segment warmup state machine; no browser."""
import sys,unittest
from pathlib import Path
from types import SimpleNamespace
sys.path.insert(0,str(Path(__file__).parent/'gui'))
from youtube_paired_native import warmup_to_segment,reference_playback_window,Blocked
class Clock:
 def __init__(self):self.value=0.
 def now(self):return self.value
 def sleep(self,seconds):self.value+=seconds
class Protocol(unittest.TestCase):
 args=SimpleNamespace(expected_duration=1343.6,width=1280,height=720)
 def snapshot(self,position,playing=True):return dict(position=position,duration=1343.6,decoded=int(position*30),dropped=0,video_width=1280,video_height=720,playing=playing,volume=1,muted=False,default_sink=True)
 def test_early_self_recovered_buffer_is_reported_before_stable_warmup(self):
  c=Clock();result={}
  def observe():
   if c.value<13.5:return self.snapshot(c.value)
   if c.value<14.5:return self.snapshot(13.5,False)
   return self.snapshot(c.value-1)
  warmup_to_segment(observe,self.args,result,clock=c.now,sleep=c.sleep)
  self.assertGreaterEqual(result['warmup_continuous_seconds'],30)
  self.assertGreaterEqual(result['warmup_continuous_media_seconds'],30)
  self.assertEqual(len(result['warmup_buffer_events']),1)
  self.assertTrue(result['warmup_buffer_events'][0]['resumed_without_input'])
  self.assertGreaterEqual(result['last_player_state']['position'],60)
  self.assertLessEqual(result['last_player_state']['position'],60.5)
 def test_late_buffer_does_not_move_the_fixed_segment_to_a_better_one(self):
  c=Clock();result={}
  def observe():
   if c.value<45:return self.snapshot(c.value)
   if c.value<46:return self.snapshot(45,False)
   return self.snapshot(c.value-1)
  with self.assertRaisesRegex(Blocked,'continuous_warmup_incomplete'):warmup_to_segment(observe,self.args,result,clock=c.now,sleep=c.sleep)
 def test_nonadvancing_player_hits_bounded_deadline(self):
  c=Clock()
  with self.assertRaisesRegex(Blocked,'warmup_deadline'):warmup_to_segment(lambda:self.snapshot(0,False),self.args,{},clock=c.now,sleep=c.sleep)
 def test_forward_seek_is_never_accepted_as_warmup(self):
  c=Clock()
  with self.assertRaisesRegex(Blocked,'clock_discontinuity'):warmup_to_segment(lambda:self.snapshot(c.value+(20 if c.value>=5 else 0)),self.args,{},clock=c.now,sleep=c.sleep)
class ReferenceWindow(unittest.TestCase):
 args=Protocol.args
 snapshot=Protocol.snapshot
 def test_startup_buffer_recovers_without_input_then_eight_continuous_seconds(self):
  c=Clock();result={}
  def observe():return self.snapshot(max(0,c.value-2),c.value>=2)
  last=reference_playback_window(observe,self.args,result,clock=c.now,sleep=c.sleep)
  self.assertGreaterEqual(result['reference_continuous_wall_seconds'],8)
  self.assertGreaterEqual(last['position'],8)
  self.assertEqual(result['reference_startup_buffer_samples'],4)
 def test_reference_permanent_pause_is_bounded_and_never_passes(self):
  c=Clock()
  with self.assertRaisesRegex(Blocked,'continuous_playback_deadline'):
   reference_playback_window(lambda:self.snapshot(0,False),self.args,{},clock=c.now,sleep=c.sleep)
  self.assertLessEqual(c.value,30.5)
 def test_reference_playing_flag_without_advancing_frames_fails(self):
  c=Clock()
  with self.assertRaisesRegex(Blocked,'clock_or_frames_stalled'):
   reference_playback_window(lambda:self.snapshot(0),self.args,{},clock=c.now,sleep=c.sleep)
 def test_reference_late_pause_restarts_the_continuous_window(self):
  c=Clock();result={}
  def observe():return self.snapshot(c.value if c.value<7 else max(7,c.value-1),not(7<=c.value<8))
  reference_playback_window(observe,self.args,result,clock=c.now,sleep=c.sleep)
  self.assertGreaterEqual(c.value,16)
 def test_slow_snapshot_cannot_pass_after_the_deadline(self):
  c=Clock()
  def observe():
   if c.value>0:c.value+=35
   return self.snapshot(c.value)
  with self.assertRaisesRegex(Blocked,'continuous_playback_deadline'):
   reference_playback_window(observe,self.args,{},clock=c.now,sleep=c.sleep)
if __name__=='__main__':unittest.main()
