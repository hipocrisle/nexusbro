"""Accept HTTP origin-form and same-origin absolute-form in the own fixture.
Not a proxy: foreign authorities, credentials and malformed targets are refused.
"""
from urllib.parse import urlsplit
def fixture_target(raw,origin):
 if not isinstance(raw,str) or len(raw)>8192:return None,'invalid'
 try:
  target=urlsplit(raw);expected=urlsplit(origin)
  if target.fragment or target.username is not None or target.password is not None:return None,'invalid'
  if target.scheme or target.netloc:
   if target.scheme!=expected.scheme or target.netloc!=expected.netloc:return None,'absolute_foreign'
   form='absolute_same_origin'
  elif raw.startswith('/') and not raw.startswith('//'):form='origin'
  else:return None,'invalid'
  return target.path,form
 except ValueError:return None,'invalid'
