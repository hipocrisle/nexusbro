"""The actual device constructor must remain private under a shared-workspace umask."""
import os,sys,tempfile,unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).parent/'gui'))
from youtube_paired_home import BenchmarkDevice
class Permissions(unittest.TestCase):
 def test_group_writable_umask_does_not_make_fixture_public(self):
  with tempfile.TemporaryDirectory() as d:
   previous=os.umask(0o002)
   try:
    device=BenchmarkDevice(Path(d)/'device',Path('/not-launched'),Path('/not-used'))
    self.assertEqual(device.root.stat().st_mode&0o777,0o700)
    self.assertEqual(device.data.stat().st_mode&0o777,0o700)
    marker=device.data/'.youtube-benchmark-opt-in'
    self.assertEqual(marker.stat().st_mode&0o777,0o600)
    self.assertEqual(marker.read_bytes(),b'nexusbro-local-youtube-benchmark-v1\n')
   finally:os.umask(previous)
if __name__=='__main__':unittest.main()
