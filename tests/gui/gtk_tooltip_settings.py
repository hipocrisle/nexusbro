"""Read standard GTK tooltip settings on an already owned private CI display.

Separate GTK client, no windows shown, no property writes or tooltip query.
Only bounded booleans/integers leave the subprocess.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

_QUERY = r'''
import ctypes as c,json
k=c.CDLL('libgtk-3.so.0');g=c.CDLL('libgobject-2.0.so.0')
k.gtk_init_check.argtypes=[c.c_void_p,c.c_void_p];k.gtk_init_check.restype=c.c_int
assert k.gtk_init_check(None,None)
k.gtk_settings_get_default.restype=c.c_void_p
settings=k.gtk_settings_get_default();assert settings
g.g_object_get.argtypes=[c.c_void_p,c.c_char_p]
enabled=c.c_int();timeout=c.c_uint();browse=c.c_uint()
for name,value in [(b'gtk-enable-tooltips',enabled),(b'gtk-tooltip-timeout',timeout),(b'gtk-tooltip-browse-timeout',browse)]:
 g.g_object_get(settings,name,c.byref(value),c.c_void_p())
assert enabled.value in (0,1) and timeout.value<=120000 and browse.value<=120000
print(json.dumps({'gtk_settings_observed':True,'gtk_enabled':bool(enabled.value),'gtk_timeout_ms':timeout.value,'gtk_browse_timeout_ms':browse.value}))
'''

def observe(device):
    if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform == 'win32' or device.x is None or device.x.poll() is not None:
        return {'gtk_settings_observed': False}
    env={**os.environ,'DISPLAY':device.display,
         'XDG_CONFIG_HOME':str(device.root/'config'),
         'XDG_DATA_HOME':str(device.root/'xdg-data'),
         'XDG_CACHE_HOME':str(device.root/'cache')}
    try:
        result=subprocess.run([sys.executable,'-c',_QUERY],env=env,capture_output=True,text=True,timeout=5)
        if result.returncode or len(result.stdout)>1024:return {'gtk_settings_observed':False}
        value=json.loads(result.stdout)
        if type(value.get('gtk_enabled')) is not bool:return {'gtk_settings_observed':False}
        return value
    except (OSError,ValueError,subprocess.TimeoutExpired):
        return {'gtk_settings_observed':False}
