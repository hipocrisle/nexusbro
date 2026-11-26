"""Required new native checks must execute in the reuse jobs used for publication."""
import unittest
from pathlib import Path
import yaml
class Wiring(unittest.TestCase):
 def test_windows_reuse_includes_media_and_real_saved_file_actions(self):
  jobs=yaml.safe_load(Path('.github/workflows/build.yml').read_text())['jobs']
  steps=jobs['runtime']['steps'];names=[s.get('name') for s in steps]
  for name in ('Fetch controlled media for exact Windows package acceptance','Native Windows media playback lock and normal exit','Native Windows saved-download open and folder actions'):
   self.assertEqual(names.count(name),1)
  self.assertLess(names.index('Native Windows saved-download open and folder actions'),names.index('Native Windows per-tab routes and trusted HTTPS'))
  self.assertNotIn('Native Windows media playback lock and normal exit',[s.get('name') for s in jobs['build']['steps']])
 def test_full_build_windows_download_step_is_not_run_on_linux(self):
  jobs=yaml.safe_load(Path('.github/workflows/build.yml').read_text())['jobs']
  step=next(s for s in jobs['build']['steps'] if s.get('name')=='Native Windows saved-download open and folder actions')
  self.assertEqual(step['if'],"matrix.os == 'windows-latest'")
 def test_linux_reuse_includes_changed_download_behavior(self):
  job=yaml.safe_load(Path('.github/workflows/build.yml').read_text())['jobs']['linux-runtime']
  names=[s.get('name') for s in job['steps']]
  self.assertIn('Verify native media and normal owned exit with typed diagnostics',names)
  self.assertIn('Verify ordinary download status and saved-file actions',names)
if __name__=='__main__':unittest.main()
