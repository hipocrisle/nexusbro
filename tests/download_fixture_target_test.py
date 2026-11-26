import sys,unittest,http.client
from pathlib import Path
sys.path.insert(0,str(Path(__file__).parent/'gui'))
from download_fixture_target import fixture_target
from downloads_library_native import Fixture
class Targets(unittest.TestCase):
 def test_foreign_targets_are_not_a_proxy(self):
  for raw in ('http://evil.invalid/page.html','http://user@127.0.0.1:123/page.html','//evil.invalid/page.html','http://[bad/'):
   self.assertIsNone(fixture_target(raw,'http://127.0.0.1:123')[0])
 def test_real_http_origin_and_absolute_form_reach_same_page(self):
  f=Fixture();origin=f.start()
  try:
   from urllib.parse import urlsplit
   u=urlsplit(origin)
   for target in ('/page.html',origin+'/page.html'):
    c=http.client.HTTPConnection(u.hostname,u.port,timeout=3);c.request('GET',target);r=c.getresponse()
    self.assertEqual(r.status,200);self.assertIn(b'dl fixture',r.read());c.close()
   self.assertEqual(f.counts()['page'],2)
   self.assertEqual(f.target_counts()['absolute_same_origin'],1)
   self.assertEqual(f.target_counts()['origin'],1)
   c=http.client.HTTPConnection(u.hostname,u.port,timeout=3);c.request('GET','http://foreign.invalid/page.html');r=c.getresponse()
   self.assertEqual(r.status,404);r.read();c.close();self.assertEqual(f.counts()['page'],2)
  finally:f.close()
if __name__=='__main__':unittest.main()
