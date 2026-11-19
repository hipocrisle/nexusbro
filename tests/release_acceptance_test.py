"""Offline publication gate cases; never contacts GitHub or publishes assets."""
import ast
import pathlib
import unittest

source=pathlib.Path(__file__).with_name('publish_tested_release.py')
function=next(node for node in ast.parse(source.read_text()).body if isinstance(node,ast.FunctionDef) and node.name=='linux_accepted')
namespace={}
exec(compile(ast.fix_missing_locations(ast.Module(body=[function],type_ignores=[])),str(source),'exec'),namespace)
accepted=namespace['linux_accepted']

class LinuxAcceptance(unittest.TestCase):
 def test_original_linux_build_passes_without_replacement(self):
  self.assertTrue(accepted([{'name':'build (ubuntu-24.04)', 'conclusion':'success'}],'123','',''))
 def test_windows_or_failed_linux_never_substitutes_for_native_pass(self):
  self.assertFalse(accepted([{'name':'build (windows-latest)', 'conclusion':'success'},{'name':'build (ubuntu-24.04)','conclusion':'failure'}],'123','',''))
 def test_fresh_native_pass_must_name_exact_candidate(self):
  self.assertTrue(accepted([{'name':'build (ubuntu-24.04)','conclusion':'failure'}],'123','123','success'))
  self.assertFalse(accepted([], '123','124','success'))
 def test_failed_cancelled_skipped_native_jobs_block_publication(self):
  for result in ('failure','cancelled','skipped',''):
   with self.subTest(result=result):self.assertFalse(accepted([], '123','123',result))
 def test_explicit_recheck_failure_blocks_even_after_old_linux_success(self):
  jobs=[{'name':'build (ubuntu-24.04)','conclusion':'success'}]
  self.assertFalse(accepted(jobs,'123','123','failure'))
  self.assertFalse(accepted(jobs,'123','124','success'))

if __name__=='__main__':unittest.main()
