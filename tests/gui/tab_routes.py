#!/usr/bin/env python3
"""Native per-tab route acceptance, on a disposable GitHub desktop only.

Requires a built app, CEF, and --tls-cert/--tls-key for 127.0.0.1 signed by a
CA already trusted by that disposable runner. No ignore-certificate flags,
production changes, simulated Tauri replies, or external network probes.
Unexecuted/missing prerequisites always leave completed=false and exit nonzero.
"""
import argparse
import json
import os
import re
import pathlib
import socket
import subprocess
import sys
import time
import urllib.request
import urllib.parse

import psutil
import websocket
from client import Client
from route_fixture import Fixture
from route_socks_fixture import SocksFixture
from stun_fixture import StunFixture
from security_native import Device, call, js_wait, painted, port, require, settings, super_record, wait_for


class InspectorNoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        # An owned local inspector has no legitimate redirect destination.
        return None


class WindowsClient(Client):
    """WebView2 inspector reads shell IPC; all UI clicks are physical input."""
    def __init__(self, inspector, diagnostic=None):
        self.seq = 0
        diagnostic = diagnostic if diagnostic is not None else {}
        diagnostic['stage'] = 'http_target_list'
        # A Python/WinINET proxy must never receive an owned inspector request.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), InspectorNoRedirect())
        with opener.open('http://127.0.0.1:' + str(inspector) + '/json/list', timeout=2) as response:
            diagnostic['http_status'] = response.status
            pages = json.load(response)
        diagnostic['targets'] = [{'type': p.get('type'), 'title': str(p.get('title', ''))[:80],
                                  'url_scheme': urllib.parse.urlsplit(p.get('url', '')).scheme,
                                  'has_websocket': bool(p.get('webSocketDebuggerUrl'))} for p in pages[:10]]
        self.ws = None
        for page in pages:
            if page.get('type') != 'page' or not page.get('webSocketDebuggerUrl'):
                continue
            endpoint = urllib.parse.urlsplit(page['webSocketDebuggerUrl'])
            require(endpoint.scheme == 'ws' and endpoint.hostname in ('127.0.0.1', 'localhost')
                    and endpoint.port == inspector and not endpoint.username and not endpoint.password,
                    'Inspector advertised a WebSocket outside the owned loopback port')
            diagnostic['stage'] = 'websocket_connect'
            # websocket-client can fall back to proxy environment variables
            # even with http_proxy_host=None; its preconnected-socket path
            # bypasses proxy discovery entirely. Never resolve a target host.
            transport = socket.create_connection(('127.0.0.1', inspector), timeout=5)
            try:
                candidate = websocket.create_connection(page['webSocketDebuggerUrl'], socket=transport,
                    timeout=5, suppress_origin=True, http_proxy_host=None,
                    http_no_proxy=['127.0.0.1', 'localhost'])
            except Exception:
                transport.close()
                raise
            self.ws = candidate
            diagnostic['stage'] = 'shell_ipc_probe'
            try:
                if self.js('!!window.__TAURI_INTERNALS__?.invoke'):
                    diagnostic['stage'] = 'ready'
                    return
                diagnostic['shell_ipc_present'] = False
            except (OSError, RuntimeError, websocket.WebSocketException) as error:
                diagnostic['probe_error_type'] = type(error).__name__
            candidate.close()
            self.ws = None
        raise ConnectionError('No real Tauri WebView2 shell inspector')

    def js(self, expression):
        self.seq += 1
        sequence = self.seq
        self.ws.send(json.dumps({'id': sequence, 'method': 'Runtime.evaluate',
                                 'params': {'expression': expression, 'returnByValue': True}}))
        while True:
            reply = json.loads(self.ws.recv())
            if reply.get('id') != sequence:
                continue
            require(not reply.get('error') and not reply.get('result', {}).get('exceptionDetails'),
                    'Real WebView2 inspector evaluation failed')
            return reply.get('result', {}).get('result', {}).get('value')

    def mouse(self, x, y, button=1):
        import pyautogui
        label = self.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        origin = self.command('plugin:window|inner_position', label=label)
        scale = self.js('devicePixelRatio||1')
        pyautogui.click(round(origin['x'] + x * scale), round(origin['y'] + y * scale),
                        button={1: 'left', 2: 'middle', 3: 'right'}[button])


def painted_interaction_point(client, *, offset=(50,30), color=(123,31,162)):
    """Wait for native CEF paint at the same physical point mouse() uses.

    A server/JS receipt proves loading, not compositor presentation. Read only
    the OS screenshot here; the following single real click must still produce
    the fixture's trusted-event receipt.
    """
    from PIL import ImageGrab

    def visible():
        point = client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect(),p=' + json.dumps(offset) + ';return [r.x+p[0],r.y+p[1]]})()')
        label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        origin = client.command('plugin:window|inner_position', label=label)
        scale = client.js('devicePixelRatio||1')
        pixel = (round(origin['x'] + point[0] * scale), round(origin['y'] + point[1] * scale))
        with ImageGrab.grab(**({'xdisplay': client.display} if sys.platform != 'win32' else {})) as shot:
            if not (0 <= pixel[0] < shot.width and 0 <= pixel[1] < shot.height):
                return None
            return point if shot.getpixel(pixel)[:3] == color else None

    return wait_for(visible, 'Actual CEF interaction button was not painted at the physical click point', 20)


def windows_token_elevated():
    import ctypes
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    advapi = ctypes.WinDLL('advapi32', use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    advapi.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    advapi.GetTokenInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p,
                                         wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
    token, elevated, length = wintypes.HANDLE(), wintypes.DWORD(), wintypes.DWORD()
    if not advapi.OpenProcessToken(kernel.GetCurrentProcess(), 0x0008, ctypes.byref(token)):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        if not advapi.GetTokenInformation(token, 20, ctypes.byref(elevated), ctypes.sizeof(elevated), ctypes.byref(length)):
            raise ctypes.WinError(ctypes.get_last_error())
        return bool(elevated.value)
    finally:
        kernel.CloseHandle(token)


class OwnedInspectorPolicy:
    """Runner-only, exact-executable WebView2 override with typed restoration.

    Microsoft documents that elevated hosts ignore env/HKCU overrides but honor
    HKLM: https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/security
    ("For an elevated host app, use appropriate override flags"). No app change.
    """
    path = r'Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments'

    def __init__(self, executable, inspector, elevated):
        require(sys.platform == 'win32' and os.environ.get('GITHUB_ACTIONS') == 'true',
                'Inspector policy is limited to disposable GitHub Windows runners')
        import winreg
        require(executable.lower() == 'nexusbro.exe', 'Inspector policy must name only the owned NexusBro executable')
        self.registry, self.name = winreg, executable
        self.hive = winreg.HKEY_LOCAL_MACHINE if elevated else winreg.HKEY_CURRENT_USER
        self.scope = 'HKLM' if elevated else 'HKCU'
        self.access = winreg.KEY_QUERY_VALUE | winreg.KEY_SET_VALUE | winreg.KEY_WOW64_64KEY
        self.value = '--remote-debugging-port=' + str(inspector)
        self.previous, self.created_key, self.applied = None, False, False

    def apply(self):
        r = self.registry
        try:
            key = r.OpenKey(self.hive, self.path, 0, self.access)
        except FileNotFoundError:
            key = r.CreateKeyEx(self.hive, self.path, 0, self.access)
            self.created_key = True
        with key:
            try:
                self.previous = r.QueryValueEx(key, self.name)
            except FileNotFoundError:
                self.previous = None
            self.applied = True  # Restore even if writing/readback fails.
            r.SetValueEx(key, self.name, 0, r.REG_SZ, self.value)
            current, kind = r.QueryValueEx(key, self.name)
            require(current == self.value and kind == r.REG_SZ, 'Owned inspector policy readback mismatch')
        return {'policy_scope': self.scope, 'policy_named_exe': self.name, 'policy_port_matches': True,
                'previous_named_value_present': self.previous is not None}

    def restore(self):
        if not self.applied:
            return
        r = self.registry
        with r.OpenKey(self.hive, self.path, 0, self.access) as key:
            if self.previous is None:
                try:
                    r.DeleteValue(key, self.name)
                except FileNotFoundError:
                    pass
            else:
                value, kind = self.previous
                r.SetValueEx(key, self.name, 0, kind, value)
            empty = r.QueryInfoKey(key)[:2] == (0, 0)
        if self.created_key and empty:
            r.DeleteKeyEx(self.hive, self.path, r.KEY_WOW64_64KEY)
        self.applied = False


class WindowsDevice:
    def __init__(self, root, exe, cef):
        self.root, self.exe, self.cef = root, exe, cef
        self.data = root / 'data'
        self.data.mkdir(parents=True)
        self.inspector = port()
        self.app = self.client = self.log = None
        self.inspector_policy = None
        self.inspector_diagnostic = {'port': self.inspector, 'attempts': 0,
                                     'proxy_policy': 'owned_loopback_direct'}

    def start(self):
        import ctypes
        from pywinauto import Desktop
        require(sys.platform == 'win32' and os.environ.get('GITHUB_ACTIONS') == 'true',
                'Native Windows inspector fixture requires a disposable GitHub runner')
        elevated = windows_token_elevated()
        self.inspector_diagnostic['token_elevated'] = elevated
        self.inspector_policy = OwnedInspectorPolicy(self.exe.name, self.inspector, elevated)
        self.inspector_diagnostic.update({'stage': 'policy_override', 'policy_scope': self.inspector_policy.scope,
                                         'policy_named_exe': self.inspector_policy.name})
        try:
            self.inspector_diagnostic.update(self.inspector_policy.apply())
        except Exception:
            self.inspector_policy.restore()
            raise
        ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
        env = {**os.environ, 'NEXUSBRO_TEST_DATA': str(self.data), 'NEXUSBRO_PROBA': '0',
               'CEF_PATH': str(self.cef), 'PATH': str(self.cef) + os.pathsep + os.environ.get('PATH', ''),
               'WEBVIEW2_USER_DATA_FOLDER': str(self.root / 'webview2'),
               'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS': '--remote-debugging-port=' + str(self.inspector)}
        if os.environ.get('NEXUSBRO_TEST_COLD_CEF')=='1':
            env.pop('CEF_PATH',None);env['PATH']=os.environ.get('PATH','')
        self.log = (self.root / ('app-' + str(time.time_ns()) + '.log')).open('wb')
        self.app = subprocess.Popen([str(self.exe)], env=env, stdout=self.log, stderr=subprocess.STDOUT)

        def ready():
            require(self.app.poll() is None, 'Owned native application exited during startup')
            self.inspector_diagnostic['attempts'] += 1
            try:
                self.client = WindowsClient(self.inspector, self.inspector_diagnostic)
                return self.client
            except (OSError, ValueError, websocket.WebSocketException) as error:
                self.inspector_diagnostic['last_error_type'] = type(error).__name__
                # Only local transport/parser errors, never request headers or
                # arbitrary process environment, enter the diagnostic artifact.
                self.inspector_diagnostic['last_error'] = str(error)[:300]
                return None
        client = wait_for(ready, 'WebView2 shell inspector did not become available', 45)
        windows = wait_for(lambda: Desktop(backend='uia').windows(process=self.app.pid, visible_only=True),
                           'Owned native Windows desktop window absent', 20)
        window = next((w for w in windows if 'NexusBro' in w.window_text()), windows[0])
        window.restore()
        from ctypes import wintypes
        user = ctypes.WinDLL('user32', use_last_error=True)
        user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user.MonitorFromWindow.argtypes = [wintypes.HWND, wintypes.DWORD]
        user.MonitorFromWindow.restype = wintypes.HANDLE
        user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                      ctypes.c_int, ctypes.c_int, wintypes.UINT]
        user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]

        class MonitorInfo(ctypes.Structure):
            _fields_ = [('size', wintypes.DWORD), ('monitor', wintypes.RECT),
                        ('work', wintypes.RECT), ('flags', wintypes.DWORD)]

        user.GetMonitorInfoW.argtypes = [wintypes.HANDLE, ctypes.POINTER(MonitorInfo)]
        handle, pid = wintypes.HWND(window.handle), wintypes.DWORD()
        require(user.GetWindowThreadProcessId(handle, ctypes.byref(pid)) and pid.value == self.app.pid,
                'Native window geometry target is not owned by this test application')
        monitor = MonitorInfo()
        monitor.size = ctypes.sizeof(monitor)
        require(user.GetMonitorInfoW(user.MonitorFromWindow(handle, 2), ctypes.byref(monitor)),
                'Cannot obtain the owned window monitor work area')
        area = monitor.work
        width, height = min(1200, area.right - area.left - 40), min(820, area.bottom - area.top - 40)
        require(width >= 900 and height >= 560, 'Runner monitor cannot fit the required native browser window')
        require(user.SetWindowPos(handle, None, area.left + 20, area.top + 20, width, height, 0x0004 | 0x0040),
                'Native SetWindowPos failed for the owned application window')
        actual = wintypes.RECT()
        require(user.GetWindowRect(handle, ctypes.byref(actual)), 'Cannot read back the native window rectangle')
        # DWM invisible borders and DPI rounding may differ from the requested
        # rectangle; require a usable visible window without exact pixel equality.
        require(actual.right - actual.left >= 900 and actual.bottom - actual.top >= 560
                and actual.left >= area.left - 16 and actual.top >= area.top - 16
                and actual.right <= area.right + 16 and actual.bottom <= area.bottom + 16,
                'Owned application window is outside the usable monitor area')
        self.inspector_diagnostic['window_rect'] = [actual.left, actual.top, actual.right, actual.bottom]
        self.inspector_diagnostic['monitor_work_rect'] = [area.left, area.top, area.right, area.bottom]
        window.set_focus()
        return client

    def startup_diagnostics(self):
        diagnostic = dict(self.inspector_diagnostic)
        diagnostic['owned_processes'] = []
        if not self.app:
            return diagnostic
        try:
            root = psutil.Process(self.app.pid)
            for process in [root] + root.children(recursive=True):
                try:
                    relevant = ('--remote-debugging-port', '--remote-debugging-address',
                                '--user-data-dir', '--type=', '--embedded-browser-webview=')
                    flags = [arg[:500] for arg in process.cmdline() if arg.startswith(relevant)]
                    entry = {'pid': process.pid, 'name': process.name(), 'flags': flags}
                    try:
                        entry['inspector_listeners'] = [c.laddr.port for c in process.net_connections(kind='tcp')
                                                       if c.status == psutil.CONN_LISTEN and c.laddr.port == self.inspector]
                    except (psutil.NoSuchProcess, psutil.AccessDenied) as error:
                        entry['listener_error'] = type(error).__name__
                    diagnostic['owned_processes'].append(entry)
                except (psutil.NoSuchProcess, psutil.AccessDenied) as error:
                    diagnostic['owned_processes'].append({'pid': process.pid, 'error': type(error).__name__})
        except (psutil.NoSuchProcess, psutil.AccessDenied) as error:
            diagnostic['process_error'] = type(error).__name__
        return diagnostic

    def stop_app(self):
        try:
            self._stop_app()
        finally:
            if self.inspector_policy:
                self.inspector_policy.restore()

    def _stop_app(self):
        if self.client:
            self.client.close()
            self.client = None
        if self.app:
            try:
                process = psutil.Process(self.app.pid)
                owned = process.children(recursive=True) + [process]
                for child in reversed(owned):
                    try:
                        child.terminate()
                    except psutil.NoSuchProcess:
                        pass
                _, alive = psutil.wait_procs(owned, timeout=6)
                for child in alive:
                    try:
                        child.kill()
                    except psutil.NoSuchProcess:
                        pass
                psutil.wait_procs(alive, timeout=4)
            except psutil.NoSuchProcess:
                pass
            self.app = None
        if self.log:
            self.log.close()
            self.log = None

    close = stop_app


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding='utf-8')


def rows(client, owner, timeout=40):
    return call(client, 'obychnye_spisok', id=owner, timeout=timeout)


def row(client, owner, identifier, timeout=40):
    return next(t for t in rows(client, owner, timeout)['vkladki'] if t['id'] == identifier)


def click_control(client, selector, timeout):
    # Scrolling/geometry reads do not invoke application handlers. The final
    # action remains a real desktop mouse click through Client/WindowsClient.
    js_wait(client, '(()=>{const e=document.querySelector(' + json.dumps(selector) + ');'
            'if(!e)return false;e.scrollIntoView({block:"nearest",inline:"nearest",behavior:"instant"});'
            'const r=e.getBoundingClientRect(),x=r.x+r.width/2,y=r.y+r.height/2,h=document.elementFromPoint(x,y);'
            'return r.width>0&&r.height>0&&x>=0&&x<innerWidth&&y>=0&&y<innerHeight&&!!h&&(h===e||e.contains(h))})()',
            'Native UI control did not become physically reachable: ' + selector, timeout)
    client.click(selector)


def switch_super(client, owner, timeout=30):
    deadline = time.monotonic() + timeout
    remaining = lambda: max(.1, deadline - time.monotonic())
    selector = '.vkladka[data-super-id="' + owner + '"]'
    # IPC pereklyuchit updates native state but an already-running engine does
    # not emit the shell selection event. Only the actual sidebar click also
    # runs App's setAktivnaya handler; waiting after raw IPC cannot replace it.
    click_control(client, selector, remaining())
    active = '.vkladka.aktivna[data-super-id="' + owner + '"]'
    js_wait(client, '!!document.querySelector(' + json.dumps(active) + ')'
            '&&!!document.querySelector(".obychnaya[aria-selected=true]")',
            'Physical Super switch did not mount the requested shell owner', remaining())
    wait_for(lambda: call(client, 'aktivnaya', timeout=remaining()) == owner
             and any(v['id'] == owner and v['zapushchena'] for v in call(client, 'spisok', timeout=remaining())),
             'Physical Super switch did not start/select its native engine', remaining())


def target_dom(client, owner, identifier, timeout=20, route_button=False):
    deadline = time.monotonic() + timeout
    remaining = lambda: max(.1, deadline - time.monotonic())
    expected_url = row(client, owner, identifier, remaining())['adres']
    super_selector = '.vkladka.aktivna[data-super-id="' + owner + '"]'
    tab_selector = '.obychnaya[data-id="' + str(identifier) + '"][aria-selected=true]'
    expression = ('(()=>{const s=document.querySelector(' + json.dumps(super_selector) + '),'
                  't=document.querySelector(' + json.dumps(tab_selector) + ');'
                  'return !!s&&!!t&&(t.getAttribute("title")||"").includes(' + json.dumps(expected_url) + ')'
                  + ('&&!!document.querySelector(' + json.dumps('button[aria-label="Путь вкладки"]:not(:disabled)') + ')' if route_button else '')
                  + '})()')
    js_wait(client, expression, 'Shell owner/tab/document does not match the intended native target', remaining())


def activate_saved(client, owner):
    switch_super(client, owner)
    # Restored selected rows are lazy. A real tab click wakes the actual
    # selected row after the physical Super switch has mounted its shell.
    select(client, owner, rows(client, owner)['aktivnaya'])


def select(client, owner, identifier, timeout=30, *, allow_loading=False):
    deadline = time.monotonic() + timeout
    remaining = lambda: max(.1, deadline - time.monotonic())
    switch_super(client, owner, remaining())
    click_control(client, '.obychnaya[data-id="' + str(identifier) + '"]', remaining())
    def selected_loaded():
        current = rows(client, owner, remaining())
        return current['aktivnaya'] == identifier and any(t['id'] == identifier and (allow_loading or not t['zagruzka']) for t in current['vkladki'])
    wait_for(selected_loaded, 'Native tab did not finish activation', remaining())
    target_dom(client, owner, identifier, remaining())


def choose(client, label):
    client.click('[role="combobox"][aria-label="Выберите путь вкладки"]')
    js_wait(client, '!!document.querySelector(".vybor-popup [role=option]")', 'Route dropdown did not open')
    # Attribute is a selector marker only; never invoke React or synthesize clicks.
    found = client.js('(()=>{const e=Array.from(document.querySelectorAll(".vybor-popup [role=option]")).find(e=>e.textContent.trim()===' + json.dumps(label) + ');if(!e)return false;e.setAttribute("data-route-test-choice","true");e.scrollIntoView({block:"nearest"});return true})()')
    require(found, 'Saved route absent from actual UI: ' + label)
    client.click('[data-route-test-choice="true"]')


def route_toolbar_geometry(point,origin,scale,viewport):
    """Same rounding as Client.click()->mouse(), with a toolbar-only crop."""
    import math
    values=[*point,origin.get('x'),origin.get('y'),scale,*viewport]
    require(all(type(v) in (int,float) and math.isfinite(v) for v in values),'Route observer geometry unavailable')
    require(0<scale<=4 and 0<viewport[0]<=4096 and 0<viewport[1]<=4096,'Route observer geometry unavailable')
    expected=(round(origin['x']+point[0]*scale),round(origin['y']+point[1]*scale))
    box=(round(origin['x']),round(origin['y']),round(origin['x']+viewport[0]*scale),
         round(origin['y']+min(130,viewport[1])*scale))
    require(0<box[2]-box[0]<=4096 and 0<box[3]-box[1]<=520,'Route observer geometry unavailable')
    return expected,box


def route_native_hit(device,expected):
    """Read-only Xlib queries on this fixture's private display. Export no IDs."""
    import ctypes as c
    from ctypes.util import find_library
    result={'available':False,'complete':False,'chain':[],'focus_role':'other'}
    require(os.environ.get('GITHUB_ACTIONS')=='true' and sys.platform!='win32'
            and device.x is not None and device.x.poll() is None,'Route observer private display unavailable')
    require(device.app is not None and device.app.poll() is None,'Route observer owner unavailable')
    from resource_soak import kernel_birth
    family={device.app.pid}|{p.pid for p in psutil.Process(device.app.pid).children(recursive=True)}
    require(len(family)<=256,'Route observer family bound')
    owned={}
    for pid in family:
        try:owned[pid]=kernel_birth(pid,'linux_ticks')
        except psutil.NoSuchProcess:pass
    require(device.app.pid in owned,'Route observer owner unavailable')
    library=find_library('X11');require(bool(library),'Route observer X11 unavailable')
    x=c.CDLL(library);Window=c.c_ulong;Display=c.c_void_p
    class Attributes(c.Structure):
        _fields_=[('x',c.c_int),('y',c.c_int),('width',c.c_int),('height',c.c_int),('border_width',c.c_int),
                  ('depth',c.c_int),('visual',c.c_void_p),('root',Window),('window_class',c.c_int),
                  ('bit_gravity',c.c_int),('win_gravity',c.c_int),('backing_store',c.c_int),
                  ('backing_planes',c.c_ulong),('backing_pixel',c.c_ulong),('save_under',c.c_int),
                  ('colormap',c.c_ulong),('map_installed',c.c_int),('map_state',c.c_int),
                  ('all_event_masks',c.c_long),('your_event_mask',c.c_long),('do_not_propagate_mask',c.c_long),
                  ('override_redirect',c.c_int),('screen',c.c_void_p)]
    x.XOpenDisplay.argtypes=[c.c_char_p];x.XOpenDisplay.restype=Display
    x.XCloseDisplay.argtypes=[Display];x.XDefaultRootWindow.argtypes=[Display];x.XDefaultRootWindow.restype=Window
    x.XInternAtom.argtypes=[Display,c.c_char_p,c.c_int];x.XInternAtom.restype=c.c_ulong
    x.XGetWindowAttributes.argtypes=[Display,Window,c.POINTER(Attributes)];x.XGetWindowAttributes.restype=c.c_int
    x.XTranslateCoordinates.argtypes=[Display,Window,Window,c.c_int,c.c_int,c.POINTER(c.c_int),c.POINTER(c.c_int),c.POINTER(Window)];x.XTranslateCoordinates.restype=c.c_int
    x.XQueryPointer.argtypes=[Display,Window,c.POINTER(Window),c.POINTER(Window),c.POINTER(c.c_int),c.POINTER(c.c_int),c.POINTER(c.c_int),c.POINTER(c.c_int),c.POINTER(c.c_uint)];x.XQueryPointer.restype=c.c_int
    x.XGetInputFocus.argtypes=[Display,c.POINTER(Window),c.POINTER(c.c_int)]
    x.XQueryTree.argtypes=[Display,Window,c.POINTER(Window),c.POINTER(Window),c.POINTER(c.POINTER(Window)),c.POINTER(c.c_uint)];x.XQueryTree.restype=c.c_int
    x.XGetWindowProperty.argtypes=[Display,Window,c.c_ulong,c.c_long,c.c_long,c.c_int,c.c_ulong,c.POINTER(c.c_ulong),c.POINTER(c.c_int),c.POINTER(c.c_ulong),c.POINTER(c.c_ulong),c.POINTER(c.c_void_p)];x.XGetWindowProperty.restype=c.c_int
    x.XFree.argtypes=[c.c_void_p];x.XSync.argtypes=[Display,c.c_int]
    x.XSetErrorHandler.argtypes=[c.c_void_p];x.XSetErrorHandler.restype=c.c_void_p
    shape_name=find_library('Xext');shape=c.CDLL(shape_name) if shape_name else None
    class Rectangle(c.Structure):
        _fields_=[('x',c.c_short),('y',c.c_short),('width',c.c_ushort),('height',c.c_ushort)]
    if shape:
        shape.XShapeQueryExtension.argtypes=[Display,c.POINTER(c.c_int),c.POINTER(c.c_int)];shape.XShapeQueryExtension.restype=c.c_int
        shape.XShapeQueryVersion.argtypes=[Display,c.POINTER(c.c_int),c.POINTER(c.c_int)];shape.XShapeQueryVersion.restype=c.c_int
        shape.XShapeGetRectangles.argtypes=[Display,Window,c.c_int,c.POINTER(c.c_int),c.POINTER(c.c_int)];shape.XShapeGetRectangles.restype=c.POINTER(Rectangle)
    display=x.XOpenDisplay(os.fsencode(device.display));require(bool(display),'Route observer display unavailable')
    failed=[False];handler_type=c.CFUNCTYPE(c.c_int,Display,c.c_void_p)
    @handler_type
    def error_handler(_display,_error):failed[0]=True;return 0
    previous=x.XSetErrorHandler(c.cast(error_handler,c.c_void_p))
    try:
        root=x.XDefaultRootWindow(display)
        shape_supported=False
        if shape:
            event_base=c.c_int();error_base=c.c_int();major=c.c_int();minor=c.c_int()
            shape_supported=bool(shape.XShapeQueryExtension(display,c.byref(event_base),c.byref(error_base))
                and shape.XShapeQueryVersion(display,c.byref(major),c.byref(minor))
                and (major.value>1 or major.value==1 and minor.value>=1))
        types={}
        for name in ('NORMAL','DIALOG','TOOLTIP','POPUP_MENU','DROPDOWN_MENU','UTILITY','DOCK','DESKTOP'):
            atom=x.XInternAtom(display,('_NET_WM_WINDOW_TYPE_'+name).encode(),1)
            if atom:types[atom]=name.lower()
        type_atom=x.XInternAtom(display,b'_NET_WM_WINDOW_TYPE',1);pid_atom=x.XInternAtom(display,b'_NET_WM_PID',1)
        def property_values(window,atom,limit):
            if not atom:return []
            actual=c.c_ulong();fmt=c.c_int();count=c.c_ulong();remaining=c.c_ulong();data=c.c_void_p()
            status=x.XGetWindowProperty(display,window,atom,0,limit,0,0,c.byref(actual),c.byref(fmt),c.byref(count),c.byref(remaining),c.byref(data))
            try:
                if status!=0 or fmt.value!=32 or count.value>limit or remaining.value or not data:return []
                return [c.cast(data,c.POINTER(c.c_ulong))[i] for i in range(count.value)]
            finally:
                if data:x.XFree(data)
        def parent(window):
            native_root=Window();up=Window();children=c.POINTER(Window)();count=c.c_uint()
            ok=x.XQueryTree(display,window,c.byref(native_root),c.byref(up),c.byref(children),c.byref(count))
            if children:x.XFree(children)
            return up.value if ok else 0
        def role(window):
            visited=set()
            for _ in range(16):
                if not window or window==root or window in visited:break
                visited.add(window);pids=property_values(window,pid_atom,1)
                if pids:
                    pid=pids[0]
                    if pid not in owned:return 'other'
                    try:
                        if kernel_birth(pid,'linux_ticks')!=owned[pid]:return 'other'
                        args=psutil.Process(pid).cmdline()
                        if len(args)>128 or kernel_birth(pid,'linux_ticks')!=owned[pid]:return 'other'
                        return 'cef' if any(a=='--nb-dvizhok' or a.startswith('--type=') for a in args) else 'ownedApp'
                    except (psutil.NoSuchProcess,psutil.AccessDenied):return 'other'
                window=parent(window)
            return 'other'
        def info(window):
            attrs=Attributes();ok=x.XGetWindowAttributes(display,window,c.byref(attrs))
            values=property_values(window,type_atom,16)
            item={'role':role(window),'window_type':next((types[v] for v in values if v in types),'other' if values else 'none'),
                  'override_redirect':bool(attrs.override_redirect) if ok else False,'mapped':attrs.map_state==2 if ok else False}
            if item['role']=='ownedApp' and item['window_type']=='tooltip':
                item.update(input_shape_supported=shape_supported,input_shape_observed=False)
                if shape_supported:
                    count=c.c_int(-1);ordering=c.c_int();rectangles=shape.XShapeGetRectangles(display,window,2,c.byref(count),c.byref(ordering))
                    try:
                        x.XSync(display,0)
                        if 0<=count.value<=256 and (count.value==0 or bool(rectangles)) and not failed[0]:
                            item.update(input_shape_observed=True,input_rectangles=count.value,input_empty=count.value==0)
                    finally:
                        if rectangles:x.XFree(rectangles)
            return item
        # TranslateCoords checks ShapeInput in X server dispatch.c. Its hit
        # chain intentionally skips empty-input tooltips. Independently observe
        # mapped visual root children, in topmost-first stacking order.
        native_root=Window();up=Window();children=c.POINTER(Window)();child_count=c.c_uint()
        tree_ok=x.XQueryTree(display,root,c.byref(native_root),c.byref(up),c.byref(children),c.byref(child_count))
        result.update(visual_scan_complete=bool(tree_ok and child_count.value<=256),mapped_owned_tooltips=[],nonowned_cover=False)
        try:
            top_found=False
            if result['visual_scan_complete']:
                for index in reversed(range(child_count.value)):
                    window=children[index];attrs=Attributes()
                    if not x.XGetWindowAttributes(display,window,c.byref(attrs)) or attrs.map_state!=2:continue
                    covers=attrs.x<=expected[0]<attrs.x+attrs.width and attrs.y<=expected[1]<attrs.y+attrs.height
                    native_role=role(window)
                    if covers and not top_found:
                        result['visual_top_role']=native_role;result['nonowned_cover']=native_role=='other';top_found=True
                    # Read hint/input properties only after held-birth ownership.
                    if native_role!='ownedApp':continue
                    hints=property_values(window,type_atom,16)
                    if not any(types.get(value)=='tooltip' for value in hints):continue
                    if len(result['mapped_owned_tooltips'])>=8:result['visual_scan_complete']=False;break
                    item=info(window);item.update(covers_expected=covers)
                    result['mapped_owned_tooltips'].append(item)
        finally:
            if children:x.XFree(children)
        current=root;visited={root};complete=False
        for _ in range(16):
            dx=c.c_int();dy=c.c_int();child=Window()
            if not x.XTranslateCoordinates(display,root,current,expected[0],expected[1],c.byref(dx),c.byref(dy),c.byref(child)):break
            if not child.value:complete=True;break
            if child.value in visited:break
            visited.add(child.value);current=child.value;result['chain'].append(info(current))
        focus=Window();revert=c.c_int();x.XGetInputFocus(display,c.byref(focus),c.byref(revert));result['focus_role']=role(focus.value)
        native_root=Window();child=Window();rx=c.c_int();ry=c.c_int();wx=c.c_int();wy=c.c_int();mask=c.c_uint()
        if x.XQueryPointer(display,root,c.byref(native_root),c.byref(child),c.byref(rx),c.byref(ry),c.byref(wx),c.byref(wy),c.byref(mask)):
            delta=(rx.value-expected[0],ry.value-expected[1]);bounded=all(abs(v)<=16384 for v in delta)
            result.update(cursor_observed=True,cursor_delta_bounded=bounded,cursor_at_expected=delta==(0,0),pointer_buttons_down=bool(mask.value & 0x1f00))
            if bounded:result.update(cursor_dx=delta[0],cursor_dy=delta[1])
        x.XSync(display,0);result.update(available=True,complete=complete and not failed[0],query_error=failed[0])
        return result
    finally:
        x.XSync(display,0);x.XCloseDisplay(display);x.XSetErrorHandler(previous)


def route_native_screenshot_allowed(hit):
    return (isinstance(hit,dict) and hit.get('available') is True and hit.get('complete') is True
            and hit.get('visual_scan_complete') is True and hit.get('nonowned_cover') is False)


def block_unknown_route_screenshot(client,hit=None):
    # Only Linux native diagnostic callers use this guard. It has no effect on
    # the independent Windows path, which has no corresponding X11 evidence.
    if not route_native_screenshot_allowed(hit):
        client.route_forbid_failure_screenshot=True
        if isinstance(hit,dict) and hit.get('nonowned_cover') is True:
            client.route_screenshot_block_reason='foreign_cover'
        elif getattr(client,'route_screenshot_block_reason',None)!='foreign_cover':
            client.route_screenshot_block_reason='incomplete_observation'


def covering_native_tooltips(hit):
    if not isinstance(hit,dict) or hit.get('available') is not True or hit.get('complete') is not True or hit.get('visual_scan_complete') is not True or hit.get('nonowned_cover') is not False:return []
    values=hit.get('mapped_owned_tooltips',[])
    if not isinstance(values,list) or len(values)>8:return []
    return [v for v in values if isinstance(v,dict) and v.get('role')=='ownedApp'
            and v.get('window_type')=='tooltip' and v.get('mapped') is True
            and v.get('override_redirect') is True and v.get('covers_expected') is True]


def tooltip_input_is_empty(hit):
    values=covering_native_tooltips(hit)
    return bool(values) and all(v.get('input_shape_supported') is True and v.get('input_shape_observed') is True
        and type(v.get('input_rectangles')) is int and v['input_rectangles']==0 and v.get('input_empty') is True for v in values)


def route_tooltip_hover_point(element):
    """Single interior text hover for the real tab-row title (CSS pixels)."""
    left,top,width,height=element
    require(width>2 and height>4,'Natural title element has no safe interior')
    return left+width/2,top+height/2


def tooltip_observation_facts(hit):
    """Compact factual aggregate of one native observation (no window IDs)."""
    tooltips = hit.get('mapped_owned_tooltips') if isinstance(hit, dict) else None
    mapped = isinstance(tooltips, list) and bool(tooltips)
    covering = mapped and any(v.get('covers_expected') is True for v in tooltips if isinstance(v, dict))
    return {'mapped': mapped, 'covering': covering,
            'complete': hit.get('complete') is True, 'scan': hit.get('visual_scan_complete') is True,
            'foreign': hit.get('nonowned_cover') is True}


def tooltip_trace_records(device):
    """Export only fixed observer enums and bounded integers, never app log text."""
    records=[]
    pattern=re.compile(r'NB_TOOLTIP_TRACE event=(has|trigger|text|realize|map|unmap) observed=([01]) requested=([01]) has=([01]) enabled=([01]) timeout_ms=([0-9]{1,6}) browse_ms=([0-9]{1,6}) text_present=([01])(?: mapped=([01]) visible=([01]) hooks=([01]) tick_ms=([0-9]{1,6}))?')
    for path in sorted(device.root.glob('app-*.log')):
        with path.open('rb') as stream:
            stream.seek(0,2);stream.seek(max(0,stream.tell()-524288))
            lines=stream.read(524288).decode('utf-8',errors='replace').splitlines()
        for line in lines:
            match=pattern.fullmatch(line)
            if not match:continue
            groups=match.groups();event=groups[0];values=list(map(int,groups[1:8]))
            if values[4]>120000 or values[5]>120000:continue
            row=dict(zip(('event','observed','requested','has','enabled','timeout_ms','browse_ms','text_present'),[event,*values]))
            if groups[8] is not None:
                extra=list(map(int,groups[8:]))
                if extra[3]>600000:continue
                row.update(dict(zip(('mapped','visible','hooks','tick_ms'),extra)))
            records.append(row)
            if len(records)==64:return records
    return records


def natural_route_tooltip_hover(client):
    # Any pre-observation or query failure leaves native ownership unknown.
    # Preserve the original exception and never repeat a physical action.
    try:return _natural_route_tooltip_hover(client)
    except Exception:
        block_unknown_route_screenshot(client)
        raise


def _natural_route_tooltip_hover(client):
    """One real hover, then read-only observation. No retries or synthetic tooltip."""
    device=client.route_trace_device
    require(os.environ.get('GITHUB_ACTIONS')=='true' and sys.platform!='win32'
            and device.x is not None and device.x.poll() is None,'Route tooltip requires private Linux fixture')
    geometry=client.js(r'(()=>{const b=document.querySelector(".tab-route-button"),t=document.querySelector(".obychnaya[data-id=\"13\"]"),name=t?.querySelector(".obychnaya-imya"),a=document.querySelector("input[aria-label=\"Адрес страницы\"]"),r=b?.getBoundingClientRect(),q=name?.getBoundingClientRect(),ar=a?.getBoundingClientRect();if(!r||!q||!ar)return null;const n=[Math.round(ar.x+ar.width/2),Math.round(ar.y+ar.height/2)],h=[Math.round(q.x+q.width/2),Math.round(q.y+q.height/2)];return !document.elementFromPoint(...n)?.closest("[title]")&&document.elementFromPoint(...h)?.closest("[title]")===t?{point:[Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)],element:[q.x,q.y,q.width,q.height],neutral:n,title:!!t.getAttribute("title"),scale:devicePixelRatio||1,viewport:[innerWidth,innerHeight]}:null})()')
    label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    require(label=='main' and geometry and geometry['title'],'Route tooltip natural title unavailable')
    origin=client.command('plugin:window|inner_position',label=label)
    expected,_=route_toolbar_geometry(geometry['point'],origin,geometry['scale'],geometry['viewport'])
    left,top,width,height=geometry['element']
    require(width>2 and height>4 and left>=0 and top>=0 and left+width<=geometry['viewport'][0]
            and top+height<=geometry['viewport'][1],'Route tooltip tab outside viewport')
    env={**os.environ,'DISPLAY':device.display}
    current=subprocess.check_output(['xdotool','getmouselocation','--shell'],env=env,text=True,timeout=3)
    require(len(current)<=512,'Route tooltip cursor unavailable')
    cursor={}
    for line in current.splitlines():
        key,_,value=line.partition('=')
        if key in ('X','Y') and value.lstrip('-').isdecimal():cursor[key]=int(value)
    require(set(cursor)=={'X','Y'},'Route tooltip cursor unavailable')
    # The accepted reference uses the tab title above the toolbar. Avoid the
    # close button's separate title by hovering the actual label interior.
    # Overlap is still proved by native observation, never inferred here.
    hover_css_x, hover_css_y = route_tooltip_hover_point((left, top, width, height))
    neutral_point = (round(origin['x'] + geometry['neutral'][0] * geometry['scale']),
                     round(origin['y'] + geometry['neutral'][1] * geometry['scale']))
    candidates = [(round(origin['x'] + (hover_css_x + offset) * geometry['scale']),
                   round(origin['y'] + hover_css_y * geometry['scale'])) for offset in (1, -1)]
    hover = next((point for point in candidates if max(abs(point[0] - cursor['X']), abs(point[1] - cursor['Y'])) > 1), None)
    require(hover is not None, 'Route tooltip hover point unavailable')
    receipt = {'move_count': 0, 'neutral_move_count': 0, 'neutral_exit_observed': False, 'observations': 0, 'mapped_covering_tooltip_observed': False,
               'journal': {'mapped_seen': 0, 'covering_seen': 0, 'incomplete': 0, 'scan_incomplete': 0, 'foreign_cover': 0}}
    client.route_open_native = {'hover': receipt}
    # select() just clicked this tab. An in-row motion is not a fresh hover
    # after that click. Establish a separately counted, observed pointer exit
    # before the one tooltip-triggering hover. No clicks, retries or JS events.
    # The previous run observed keyboard focus in CEF while hovering GTK.
    # Establish the real shell focus through its ordinary native command once;
    # this does not create a tooltip, mutate title, or generate a DOM event.
    if hasattr(device,'tooltip_trace_library'):
        # Verify the observer is mapped in the actual native shell process,
        # not merely the dbus launcher. Paths/PIDs/mappings never enter receipts.
        family=[psutil.Process(device.app.pid),*psutil.Process(device.app.pid).children(recursive=True)]
        candidates=[]
        for process in family:
            try:
                if pathlib.Path(process.exe()).resolve()!=device.exe.resolve():continue
                if any(a=='--nb-dvizhok' or a.startswith('--type=') for a in process.cmdline()):continue
                candidates.append(process)
            except psutil.NoSuchProcess:continue
        require(len(candidates)==1,'Exact native shell for GTK observer is ambiguous')
        from resource_soak import kernel_birth
        process=candidates[0];birth=kernel_birth(process.pid,'linux_ticks')
        with open('/proc/'+str(process.pid)+'/maps','rb') as stream:maps=stream.read(4*1024*1024+1)
        require(len(maps)<=4*1024*1024 and kernel_birth(process.pid,'linux_ticks')==birth
                and str(pathlib.Path(device.tooltip_trace_library).resolve()).encode() in maps,
                'Read-only GTK observer is not mapped in exact native shell')
        receipt['native_trace_library_mapped']=True
        (device.root/'tooltip-trace.arm').touch(exist_ok=False)
    call(client,'fokus_obolochke')
    initial=route_native_hit(device,expected)
    block_unknown_route_screenshot(client,initial)
    require(initial.get('available') is True and initial.get('complete') is True
            and initial.get('nonowned_cover') is False,'Neutral pointer ownership unavailable')
    require(initial.get('focus_role') == 'ownedApp', 'Native shell focus before tooltip hover unconfirmed')
    subprocess.run(['xdotool','mousemove',str(neutral_point[0]),str(neutral_point[1])],env=env,check=True,
                   stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=3)
    receipt['neutral_move_count']=1
    js_wait(client,"!document.querySelector('.tab-route-button').matches(':hover')",
            'Pointer did not leave the route button before tooltip hover',5)
    neutral=route_native_hit(device,neutral_point)
    block_unknown_route_screenshot(client,neutral)
    require(neutral.get('available') is True and neutral.get('complete') is True
            and neutral.get('nonowned_cover') is False and neutral.get('cursor_at_expected') is True,
            'Neutral pointer position unconfirmed')
    receipt['neutral_exit_observed']=True
    # GTK 3.24.41 has BROWSE_DISABLE_TIMEOUT=500ms after hiding an old popup.
    # The previous trace entered the next target only18ms after unmap. Let the
    # old state expire over a title-free address input before the ONE hover.
    began=time.monotonic();time.sleep(.6)
    quiescent=route_native_hit(device,expected)
    require(quiescent.get('available') is True and quiescent.get('complete') is True
            and quiescent.get('visual_scan_complete') is True and quiescent.get('nonowned_cover') is False
            and not quiescent.get('mapped_owned_tooltips'),'Old native tooltip did not quiesce before fresh title hover')
    receipt['neutral_quiescence_observed']=True
    receipt['neutral_wait_ms']=round((time.monotonic()-began)*1000)
    subprocess.run(['xdotool', 'mousemove', str(hover[0]), str(hover[1])], env=env, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)
    receipt['move_count'] = 1
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and receipt['observations'] < 50:
        hit = route_native_hit(device, expected); receipt['observations'] += 1; receipt['last_hit'] = hit
        facts = tooltip_observation_facts(hit)
        receipt['journal']['mapped_seen'] += 1 if facts['mapped'] else 0
        receipt['journal']['covering_seen'] += 1 if facts['covering'] else 0
        receipt['journal']['incomplete'] += 1 if not facts['complete'] else 0
        receipt['journal']['scan_incomplete'] += 1 if not facts['scan'] else 0
        receipt['journal']['foreign_cover'] += 1 if facts['foreign'] else 0
        block_unknown_route_screenshot(client, hit)
        require(hit.get('nonowned_cover') is False, 'Route tooltip target covered by foreign window')
        if covering_native_tooltips(hit): receipt['mapped_covering_tooltip_observed'] = True; return
        time.sleep(.08)
    # Failure diagnosis only, using the SAME existing ownership/foreign-cover
    # screenshot gate; no tooltip synthesis, repositioning, or acceptance change.
    dom=client.js(r'(()=>{const b=document.querySelector(".tab-route-button"),t=document.querySelector(".obychnaya[data-id=\"13\"]");return {dom_title_element_hovered:!!t?.matches(":hover"),dom_has_focus:document.hasFocus(),dom_button_hovered:!!b?.matches(":hover"),dom_button_enabled:!!b&&!b.disabled,dom_title_present:!!b?.getAttribute("title"),dom_pointer_on_svg:!!b?.querySelector("svg:hover")}})()')
    if isinstance(dom,dict):
        receipt.update({key:value for key,value in dom.items() if key in {'dom_has_focus','dom_button_hovered','dom_button_enabled','dom_title_present','dom_pointer_on_svg','dom_title_element_hovered'} and type(value)is bool})
    from gtk_tooltip_settings import observe as observe_gtk_settings
    receipt['gtk_settings']=observe_gtk_settings(device)
    client.route_open_native['failure']=route_toolbar_observation(client,'hover')
    journal = receipt['journal']
    require(False, 'Mapped owned native tooltip covering route target not observed; '
                   f"facts: observations={receipt['observations']} "
                   f"mapped_seen={journal['mapped_seen']} covering_seen={journal['covering_seen']} "
                   f"incomplete={journal['incomplete']} scan_incomplete={journal['scan_incomplete']} "
                   f"foreign_cover={journal['foreign_cover']}")


def route_toolbar_observation(client,phase):
    """No pointer/focus changes. Only a top-130px crop of an owned private Xvfb."""
    if sys.platform=='win32':return {'available':False,'unsupported_platform':True}
    result={'available':False,'crop_saved':False}
    try:
        device=client.route_trace_device;out=client.route_trace_out
        require(os.environ.get('GITHUB_ACTIONS')=='true' and device.x is not None and device.x.poll() is None
                and client.display==device.display,'Route observer private display unavailable')
        label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        require(label=='main','Route observer owner unavailable')
        geometry=client.js('(()=>{const b=document.querySelector(".tab-route-button"),r=b?.getBoundingClientRect();return r?{point:[Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)],scale:devicePixelRatio||1,viewport:[innerWidth,innerHeight]}:null})()')
        origin=client.command('plugin:window|inner_position',label=label)
        expected,box=route_toolbar_geometry(geometry['point'],origin,geometry['scale'],geometry['viewport'])
        if phase=='before':client.route_open_expected_point=expected
        original=getattr(client,'route_open_expected_point',expected)
        result['expected_point_unchanged']=expected==original
        screen=subprocess.check_output(['xdotool','getdisplaygeometry'],env={**os.environ,'DISPLAY':device.display},text=True,timeout=3).split()
        require(len(screen)==2 and all(v.isdecimal() for v in screen),'Route observer screen unavailable')
        require(0<=box[0]<box[2]<=int(screen[0]) and 0<=box[1]<box[3]<=int(screen[1]),'Route observer crop outside owned screen')
        result['hit_test']=route_native_hit(device,original)
        block_unknown_route_screenshot(client,result['hit_test'])
        require(route_native_screenshot_allowed(result['hit_test']),'Route observer native ownership not established')
        from PIL import ImageGrab
        with ImageGrab.grab(bbox=box,xdisplay=device.display) as shot:
            require(shot.size==(box[2]-box[0],box[3]-box[1]),'Route observer crop unavailable')
            shot.save(out/('route-open-'+phase+'.png'))
        result.update(available=True,crop_saved=True,crop_width=box[2]-box[0],crop_height=box[3]-box[1])
    except Exception:
        block_unknown_route_screenshot(client)
        result['observation_failed']=True
    return result


# Diagnostic observers only. No React handlers, synthetic events or IPC replies.
ROUTE_OPEN_DIAGNOSTIC = r'''
(()=>{
 const expectedOwner=__OWNER__,expectedTab=__TAB__,internal=window.__TAURI_INTERNALS__;
 if(!internal||typeof internal.invoke!=='function')return false;
 const previous=window.__routeOpenDiagnostic;if(previous?.stop)try{previous.stop()}catch{}
 const original=internal.invoke,types=['pointerdown','pointerup','pointercancel','mousedown','mouseup','click'];
 const state={trace:[],counts:Object.fromEntries(types.map(t=>[t,{trusted_route:0,trusted_other:0,untrusted:0}])),restore_verified:false,observation_failed:false,native_spy_installed:false};
 const started=performance.now();let live=true,observer=null;
 const ownRoute=e=>!!(e instanceof Element&&e.closest('.tab-route-button'));
 const snapshot=()=>{
  const button=document.querySelector('.tab-route-button'),selected=document.querySelector('.vkladka.aktivna[data-super-id]'),tab=document.querySelector('.obychnaya[aria-selected=true]');
  const modals=Array.from(document.querySelectorAll('.zatemnenie,[role=dialog],[role=alertdialog],.browser-lock-screen,.settings-overlay,.security-overlay')).filter(e=>!e.closest('[data-tab-route-root]'));
  let hit=false,inView=false;if(button){const r=button.getBoundingClientRect(),x=r.x+r.width/2,y=r.y+r.height/2;inView=r.width>0&&r.height>0&&x>=0&&y>=0&&x<innerWidth&&y<innerHeight;hit=inView&&button.contains(document.elementFromPoint(x,y));}
  const routeState=button?.getAttribute('data-route-state');
  return {button_present:!!button,button_enabled:!!button&&!button.disabled,button_busy:button?.getAttribute('aria-busy')==='true',route_state:['pending','error','applied','unknown'].includes(routeState)?routeState:'unknown',active_owner_match:selected?.getAttribute('data-super-id')===expectedOwner,active_tab_match:tab?.getAttribute('data-id')===String(expectedTab),point_hits_route:hit,center_in_view:inView,other_modals:Math.min(modals.length,32),visible_other_modals:Math.min(modals.filter(e=>e.getClientRects().length>0).length,32),route_roots:Math.min(document.querySelectorAll('[data-tab-route-root]').length,4),route_comboboxes:Math.min(document.querySelectorAll('[data-tab-route-root] [role=combobox]').length,16)};
 };
 const record=(event,extra={})=>{try{if(!live)return;state.trace.push({event,ms:Math.min(60000,Math.max(0,Math.round(performance.now()-started))),...snapshot(),...extra});if(state.trace.length>32)state.trace.shift()}catch{state.observation_failed=true}};
 function wrapped(command,args){
  const tracked=command==='pokazat_dvizhki'||command==='fokus_obolochke';
  const extra={command,phase:'request'};try{if(command==='pokazat_dvizhki'&&typeof args?.vidno==='boolean')extra.visible=args.vidno}catch{}
  if(tracked)record('invoke',extra);
  let result;try{result=Reflect.apply(original,this,arguments)}catch(error){if(tracked)record('invoke',{command,phase:'rejected'});throw error}
  if(tracked)try{result.then(()=>record('invoke',{command,phase:'resolved'}),()=>record('invoke',{command,phase:'rejected'}))}catch{}
  return result;
 }
 const capture=e=>{try{if(!live||!types.includes(e.type))return;const route=ownRoute(e.target),count=state.counts[e.type],key=e.isTrusted?(route?'trusted_route':'trusted_other'):'untrusted';count[key]=Math.min(32,count[key]+1);record(e.type,{phase:'capture',trusted:e.isTrusted===true,target_is_route:route,default_prevented:e.defaultPrevented===true})}catch{}};
 const bubble=e=>{try{if(live&&types.includes(e.type))record(e.type,{phase:'bubble',trusted:e.isTrusted===true,target_is_route:ownRoute(e.target),default_prevented:e.defaultPrevented===true})}catch{}};
 const stop=()=>{try{record('finished');live=false;observer?.disconnect();for(const t of types){document.removeEventListener(t,capture,true);document.removeEventListener(t,bubble,false)}if(internal.invoke===wrapped){internal.invoke=original;state.restore_verified=true}}catch{}return state;};
 window.__routeOpenDiagnostic={stop,state};
 try{
  try{internal.invoke=wrapped;state.native_spy_installed=internal.invoke===wrapped}catch{state.native_spy_installed=false}for(const t of types){document.addEventListener(t,capture,{capture:true,passive:true});document.addEventListener(t,bubble,{capture:false,passive:true})}
  let before=snapshot();observer=new MutationObserver(records=>{try{
   const delta={route_added:0,route_removed:0,other_modal_added:0,other_modal_removed:0};
   const count=(nodes,removed)=>{for(const node of Array.from(nodes).slice(0,32)){if(!(node instanceof Element))continue;const candidates=[node,...Array.from(node.querySelectorAll('[data-tab-route-root],.zatemnenie,[role=dialog],[role=alertdialog],.browser-lock-screen,.settings-overlay,.security-overlay')).slice(0,32)];for(const element of candidates){if(element.matches('[data-tab-route-root]')){const key=removed?'route_removed':'route_added';delta[key]=Math.min(32,delta[key]+1)}else if(element.matches('.zatemnenie,[role=dialog],[role=alertdialog],.browser-lock-screen,.settings-overlay,.security-overlay')&&!element.closest('[data-tab-route-root]')){const key=removed?'other_modal_removed':'other_modal_added';delta[key]=Math.min(32,delta[key]+1)}}}};
   for(const mutation of records.slice(0,64)){count(mutation.addedNodes,false);count(mutation.removedNodes,true)}
   const now=snapshot();if(Object.values(delta).some(Boolean)||['other_modals','visible_other_modals','route_roots','route_comboboxes'].some(k=>now[k]!==before[k]))record('dom_change',delta);before=now;
  }catch{state.observation_failed=true}});observer.observe(document.body,{childList:true,subtree:true});record('armed');return true;
 }catch{state.observation_failed=true;stop();return false}
})()
'''


def safe_route_open_trace(value):
    """Whitelist only fixed observer fields; never export titles, URLs or errors."""
    if not isinstance(value,dict):return {'observation_failed':True,'trace':[]}
    bools={'button_present','button_enabled','button_busy','active_owner_match','active_tab_match',
           'point_hits_route','center_in_view','visible','trusted','target_is_route','default_prevented'}
    integers={'ms','other_modals','visible_other_modals','route_roots','route_comboboxes','route_added','route_removed','other_modal_added','other_modal_removed'}
    enums={'event':{'armed','finished','invoke','dom_change','pointerdown','pointerup','pointercancel','mousedown','mouseup','click'},
           'phase':{'capture','bubble','request','resolved','rejected'},
           'command':{'pokazat_dvizhki','fokus_obolochke'},'route_state':{'pending','error','applied','unknown'}}
    result={'trace':[],'counts':{},'restore_verified':value.get('restore_verified') is True,
            'observation_failed':value.get('observation_failed') is True,
            'native_spy_installed':value.get('native_spy_installed') is True}
    trace=value.get('trace',[])
    for row in trace[-32:] if isinstance(trace,list) else []:
        if not isinstance(row,dict) or row.get('event') not in enums['event']:continue
        clean={}
        for key,item in row.items():
            if key in bools and type(item)is bool:clean[key]=item
            elif key in integers and type(item)is int and 0<=item<=60000:clean[key]=item
            elif key in enums and isinstance(item,str) and item in enums[key]:clean[key]=item
        result['trace'].append(clean)
    counts=value.get('counts',{})
    if isinstance(counts,dict):
        for event in ('pointerdown','pointerup','pointercancel','mousedown','mouseup','click'):
            row=counts.get(event)
            if isinstance(row,dict):result['counts'][event]={key:number for key,number in row.items() if key in ('trusted_route','trusted_other','untrusted') and type(number)is int and 0<=number<=32}
    return result


def safe_route_open_native(value,depth=0):
    """Retain native proof fields only, never actual IDs, coordinates or paths."""
    if not isinstance(value,dict) or depth>8:return {}
    booleans={'available','complete','query_error','visual_scan_complete','nonowned_cover','override_redirect',
        'mapped','input_shape_supported','input_shape_observed','input_empty','covers_expected',
        'cursor_observed','cursor_delta_bounded','cursor_at_expected','expected_point_unchanged','crop_saved',
        'observation_failed','unsupported_platform','mapped_covering_tooltip_observed',
        'dom_has_focus','dom_button_hovered','dom_button_enabled','dom_title_present','dom_pointer_on_svg','dom_title_element_hovered',
        'gtk_settings_observed','gtk_enabled','pointer_buttons_down','native_trace_library_mapped',
        'mapped_tooltip_before_click','input_empty_before_click','one_physical_click','trusted_dom_click','dialog_present','neutral_exit_observed','neutral_quiescence_observed'}
    limits={'neutral_wait_ms':(600,5000),'gtk_timeout_ms':(0,120000),'gtk_browse_timeout_ms':(0,120000),'cursor_dx':(-16384,16384),'cursor_dy':(-16384,16384),'input_rectangles':(0,256),
        'crop_width':(1,4096),'crop_height':(1,520),'observations':(0,50),'move_count':(0,1),'neutral_move_count':(0,1)}
    enums={'role':{'ownedApp','cef','other'},'focus_role':{'ownedApp','cef','other'},
        'visual_top_role':{'ownedApp','cef','other'},'window_type':{'normal','dialog','tooltip','popup_menu',
        'dropdown_menu','utility','dock','desktop','other','none'}}
    result={}
    for key,item in value.items():
        if key in booleans and type(item)is bool:result[key]=item
        elif key in limits and type(item)is int and limits[key][0]<=item<=limits[key][1]:result[key]=item
        elif key in enums and isinstance(item,str) and item in enums[key]:result[key]=item
        elif key in {'before','failure','hover','regression','hit_test','last_hit','gtk_settings'} and isinstance(item,dict):
            result[key]=safe_route_open_native(item,depth+1)
        elif key=='journal' and isinstance(item,dict):
            result[key]={name:count for name,count in item.items()
                if name in {'mapped_seen','covering_seen','incomplete','scan_incomplete','foreign_cover'}
                and type(count)is int and 0<=count<=50}
        elif key in {'chain','mapped_owned_tooltips'} and isinstance(item,list):
            result[key]=[safe_route_open_native(row,depth+1) for row in item[:16 if key=='chain' else 8] if isinstance(row,dict)]
    return result


def persist_route_open_report(report,client,device=None):
    # Reporting must never replace an actual native acceptance exception.
    try:
        device=device if device is not None else getattr(client,'route_trace_device',None)
        saved=getattr(device,'route_open_report',{}) if device is not None else {}
        snapshot=dict(saved) if isinstance(saved,dict) else {}
        if hasattr(client,'route_open_trace'):snapshot['route_open_diagnostic']=safe_route_open_trace(client.route_open_trace)
        if hasattr(client,'route_open_native'):snapshot['route_open_native']=safe_route_open_native(client.route_open_native)
        if snapshot and device is not None:device.route_open_report=snapshot
        if isinstance(report,dict):report.update(snapshot)
    except Exception:
        try:
            if isinstance(report,dict):report['route_open_reporting_failed']=True
        except Exception:pass


def route_open_trace_begin(client,owner,identifier):
    client.route_open_trace={'observation_failed':True,'trace':[]}
    try:
        script=ROUTE_OPEN_DIAGNOSTIC.replace('__OWNER__',json.dumps(owner)).replace('__TAB__',str(identifier))
        return client.js(script) is True
    except Exception:return False


def route_open_trace_finish(client):
    try:client.route_open_trace=safe_route_open_trace(client.js('window.__routeOpenDiagnostic?.stop()'))
    except Exception:client.route_open_trace={'observation_failed':True,'trace':[]}
    # Preserve the successful case before a later Device restart replaces Client.
    persist_route_open_report(None,client)


def route_open_trace_self_test():
    complete=dict(available=True,complete=True,visual_scan_complete=True,nonowned_cover=False)
    assert route_native_screenshot_allowed(complete)
    for bad in (None,{},dict(complete,available=False),dict(complete,complete=False),
                dict(complete,visual_scan_complete=False),dict(complete,nonowned_cover=True),
                dict(complete,complete=1)):
        assert not route_native_screenshot_allowed(bad)
    from types import SimpleNamespace
    from unittest.mock import patch
    unknown=SimpleNamespace();block_unknown_route_screenshot(unknown,dict(complete,complete=False))
    assert unknown.route_forbid_failure_screenshot and unknown.route_screenshot_block_reason=='incomplete_observation'
    foreign=SimpleNamespace();block_unknown_route_screenshot(foreign,dict(complete,nonowned_cover=True))
    assert foreign.route_forbid_failure_screenshot and foreign.route_screenshot_block_reason=='foreign_cover'
    block_unknown_route_screenshot(foreign)
    assert foreign.route_screenshot_block_reason=='foreign_cover'
    windows=SimpleNamespace()
    with patch.object(sys,'platform','win32'):
        assert route_toolbar_observation(windows,'before')['unsupported_platform'] is True
    assert not hasattr(windows,'route_forbid_failure_screenshot')
    failed=SimpleNamespace();original_error=RuntimeError('synthetic-private-error')
    with patch(__name__+'._natural_route_tooltip_hover',side_effect=original_error) as observed:
        try:natural_route_tooltip_hover(failed)
        except RuntimeError as actual:assert actual is original_error
        else:raise AssertionError('Observation exception suppressed')
        assert observed.call_count==1
    assert failed.route_forbid_failure_screenshot and failed.route_screenshot_block_reason=='incomplete_observation'
    private='https://private.invalid/secret title'
    saved={};held_device=SimpleNamespace();first=SimpleNamespace(route_trace_device=held_device,
        route_open_trace={'trace':[{'event':'click','phase':'capture','trusted':True,'target_is_route':True,'url':private}],
                          'counts':{'click':{'trusted_route':1}},'restore_verified':True},
        route_open_native={'before':{'hit_test':{'available':True,'complete':True,'visual_scan_complete':True,
            'nonowned_cover':False,'mapped_owned_tooltips':[{'role':'ownedApp','window_type':'tooltip','mapped':True,
                'input_shape_observed':True,'input_rectangles':0,'input_empty':True,'pid':987654321,'title':private}]}},
            'regression':{'one_physical_click':True,'trusted_dom_click':True,'dialog_present':True},'path':private})
    persist_route_open_report(saved,first)
    persist_route_open_report(saved,SimpleNamespace(route_trace_device=held_device))  # Restart must not erase old proof.
    persist_route_open_report(saved,None,held_device)  # Even a closed latest inspector retains the Device snapshot.
    class Broken:
        @property
        def route_open_trace(self):raise RuntimeError('synthetic report failure')
    persist_route_open_report({},Broken())
    encoded=json.dumps(saved)
    assert private not in encoded and '987654321' not in encoded and '"pid"' not in encoded
    assert saved['route_open_diagnostic']['counts']['click']['trusted_route']==1
    assert saved['route_open_native']['before']['hit_test']['mapped_owned_tooltips'][0]['input_rectangles']==0
    assert saved['route_open_native']['regression']['dialog_present'] is True
    tooltip=dict(role='ownedApp',window_type='tooltip',mapped=True,override_redirect=True,covers_expected=True,
                 input_shape_supported=True,input_shape_observed=True,input_rectangles=0,input_empty=True)
    hit=dict(available=True,complete=True,visual_scan_complete=True,nonowned_cover=False,mapped_owned_tooltips=[tooltip])
    assert tooltip_input_is_empty(hit)
    assert not tooltip_input_is_empty(dict(hit,mapped_owned_tooltips=[dict(tooltip,input_rectangles=1,input_empty=False)]))
    assert not tooltip_input_is_empty(dict(hit,nonowned_cover=True))
    assert not tooltip_input_is_empty(dict(hit,mapped_owned_tooltips=[dict(tooltip,input_shape_observed=False)]))
    assert not tooltip_input_is_empty(dict(hit,mapped_owned_tooltips=[]))
    expected,box=route_toolbar_geometry([10,20],{'x':100,'y':200},1.5,[800,600])
    assert expected==(115,230) and box==(100,200,1300,395)
    try:route_toolbar_geometry([10,20],{'x':0,'y':0},10,[800,600])
    except Exception:pass
    else:raise AssertionError('Unbounded toolbar crop accepted')
    # One interior title point; the hover itself remains physical at runtime.
    assert route_tooltip_hover_point((100,50,200,34))==(200,67)
    assert route_tooltip_hover_point((100,50,8,16))==(104,58)
    try:route_tooltip_hover_point((100,50,1,1))
    except Exception:pass
    else:raise AssertionError('Degenerate title hover accepted')
    facts=tooltip_observation_facts(dict(complete=True,visual_scan_complete=True,nonowned_cover=False,
        mapped_owned_tooltips=[dict(role='ownedApp',window_type='tooltip',mapped=True,covers_expected=True)]))
    assert facts['mapped'] is True and facts['covering'] is True and facts['foreign'] is False
    facts=tooltip_observation_facts(dict(complete=True,visual_scan_complete=True,nonowned_cover=True,mapped_owned_tooltips=[]))
    assert facts['mapped'] is False and facts['foreign'] is True
    facts=tooltip_observation_facts(dict(complete=False,visual_scan_complete=False,nonowned_cover=False))
    assert facts['complete'] is False and facts['scan'] is False
    journal=safe_route_open_native({'hover':{'journal':{'mapped_seen':50,'covering_seen':0,
        'foreign_cover':True,'incomplete':51,'private':'secret'}}})
    assert journal=={'hover':{'journal':{'mapped_seen':50,'covering_seen':0}}}
    import tempfile
    with tempfile.TemporaryDirectory(prefix='nb-tooltip-log-unit-') as folder:
        log=pathlib.Path(folder)/'app-1.log'
        log.write_text('private URL must not escape\nNB_TOOLTIP_TRACE event=trigger observed=1 requested=1 has=1 enabled=1 timeout_ms=500 browse_ms=60 text_present=0\n')
        observed=tooltip_trace_records(SimpleNamespace(root=pathlib.Path(folder)))
        assert observed==[{'event':'trigger','observed':1,'requested':1,'has':1,'enabled':1,'timeout_ms':500,'browse_ms':60,'text_present':0}]
        assert 'private URL' not in json.dumps(observed)
        with log.open('a') as stream:stream.write('NB_TOOLTIP_TRACE event=map observed=0 requested=1 has=0 enabled=0 timeout_ms=0 browse_ms=0 text_present=0 mapped=1 visible=1 hooks=1 tick_ms=500\n')
        lifecycle=tooltip_trace_records(SimpleNamespace(root=pathlib.Path(folder)))
        assert len(lifecycle)==2 and lifecycle[1]['event']=='map' and lifecycle[1]['mapped']==1 and lifecycle[1]['visible']==1 and lifecycle[1]['hooks']==1 and lifecycle[1]['tick_ms']==500

    facts=safe_route_open_native({'hover':{'dom_pointer_on_svg':True,'gtk_settings':{'gtk_settings_observed':True,'gtk_enabled':False,'gtk_timeout_ms':500,'private':'secret'}}})
    assert facts=={'hover':{'dom_pointer_on_svg':True,'gtk_settings':{'gtk_settings_observed':True,'gtk_enabled':False,'gtk_timeout_ms':500}}}
    private='https://private.invalid/secret title'
    receipt=safe_route_open_trace({'trace':[{'event':'invoke','command':'pokazat_dvizhki','phase':'resolved','visible':False,'url':private,'error':private},{'event':private}], 'counts':{'click':{'trusted_route':1,'secret':private}},'restore_verified':True})
    assert private not in json.dumps(receipt) and len(receipt['trace'])==1 and receipt['restore_verified']
    assert safe_route_open_trace({'trace':[{'event':'armed'}]*100})['trace']==[{'event':'armed'}]*32
    # Execute the ACTUAL observer/wrapper with a tiny mock DOM; no browser/network.
    script=ROUTE_OPEN_DIAGNOSTIC.replace('__OWNER__',json.dumps('routes-a')).replace('__TAB__','13')
    harness=r'''
    const assert=x=>{if(!x)throw new Error('observer_contract')};
    global.Element=class{closest(){return this.route?this:null}};
    const button=new Element();button.route=true;button.disabled=false;button.getAttribute=k=>k==='data-route-state'?'applied':'false';button.getBoundingClientRect=()=>({x:1,y:1,width:20,height:20});button.contains=x=>x===button;
    global.innerWidth=1280;global.innerHeight=900;
    const listeners=[];global.document={body:{},querySelector:s=>s==='.tab-route-button'?button:{getAttribute:k=>k==='data-super-id'?'routes-a':'13'},querySelectorAll:()=>[],elementFromPoint:()=>button,addEventListener:(t,f,o)=>listeners.push([t,f,o]),removeEventListener:(t,f)=>{const i=listeners.findIndex(x=>x[0]===t&&x[1]===f);if(i>=0)listeners.splice(i,1)}};
    global.MutationObserver=class{observe(){}disconnect(){}};
    let calls=0,received=null;let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});
    const original=function(){calls++;received={self:this,args:[...arguments]};return promise};
    global.window={__TAURI_INTERNALS__:{invoke:original}};
    assert(eval(SCRIPT)===true);
    const context=window.__TAURI_INTERNALS__,args={vidno:false,private:'https://private.invalid/secret'};
    const returned=context.invoke('pokazat_dvizhki',args);
    assert(returned===promise&&calls===1&&received.self===context&&received.args[1]===args);
    const untracked=context.invoke('private_command',args);assert(untracked===promise&&calls===2);
    const throwingArgs=Object.defineProperty({},'vidno',{get(){throw new Error('private_observer_getter')}});
    assert(context.invoke('pokazat_dvizhki',throwingArgs)===promise&&calls===3);
    for(const [type,fn,options] of [...listeners])if(type==='click')fn({type:'click',target:button,isTrusted:true,defaultPrevented:false});
    resolve('https://private.invalid/result');
    Promise.resolve().then(async()=>{const output=window.__routeOpenDiagnostic.stop();assert(context.invoke===original&&listeners.length===0&&output.native_spy_installed===true);assert(output.counts.click.trusted_route===1);assert(output.trace.some(x=>x.event==='invoke'&&x.phase==='resolved'));
      const error=new Error('https://private.invalid/rejected');const rejected=Promise.reject(error);let rejectCalls=0;const rejectOriginal=()=>{rejectCalls++;return rejected};context.invoke=rejectOriginal;assert(eval(SCRIPT)===true);
      const result=context.invoke('fokus_obolochke');assert(result===rejected&&rejectCalls===1);assert(await result.catch(value=>value===error));
      const second=window.__routeOpenDiagnostic.stop();assert(context.invoke===rejectOriginal&&listeners.length===0&&second.trace.some(x=>x.phase==='rejected'));
      Object.defineProperty(context,'invoke',{value:original,writable:false,configurable:true});assert(eval(SCRIPT)===true);assert(window.__routeOpenDiagnostic.state.native_spy_installed===false);
      for(const [type,fn] of [...listeners])if(type==='click')fn({type:'click',target:button,isTrusted:true,defaultPrevented:false});
      const readonly=window.__routeOpenDiagnostic.stop();assert(context.invoke===original&&listeners.length===0&&readonly.counts.click.trusted_route===1);console.log(JSON.stringify(output))
    }).catch(()=>{process.exitCode=1});
    '''.replace('SCRIPT',json.dumps(script))
    completed=subprocess.run(['node','-'],input=harness,text=True,capture_output=True,timeout=5)
    assert completed.returncode==0 and len(completed.stdout)<=65536,'Offline route observer failed'
    parsed=safe_route_open_trace(json.loads(completed.stdout))
    assert 'private.invalid' not in json.dumps(parsed) and parsed['restore_verified']
    # Compile the ACTUAL embedded native-observer expressions before CI, so
    # Python quote escaping cannot turn a fixture change into a JS syntax bug.
    import ast
    scripts=[]
    for function in ast.parse(pathlib.Path(__file__).read_text()).body:
        if not isinstance(function,ast.FunctionDef) or function.name not in ('_natural_route_tooltip_hover','route_toolbar_observation'):continue
        for node in ast.walk(function):
            if isinstance(node,ast.Call) and isinstance(node.func,ast.Attribute) and node.func.attr=='js' and node.args and isinstance(node.args[0],ast.Constant) and isinstance(node.args[0].value,str):scripts.append(node.args[0].value)
    syntax=subprocess.run(['node','-e','for(const s of JSON.parse(require("fs").readFileSync(0,"utf8")))new Function(s)'],input=json.dumps(scripts),text=True,capture_output=True,timeout=5)
    assert syntax.returncode==0,'Embedded native observer JavaScript syntax is invalid'
    return {'self_test':True,'synthetic_only':True,'native_execution':False}


def apply_ui(client, label, owner, identifier, screenshot=None):
    target_dom(client, owner, identifier, route_button=True)
    require(call(client, 'aktivnaya') == owner and rows(client, owner)['aktivnaya'] == identifier,
            'Route UI target no longer matches the active native owner/tab')
    tracing=identifier==13 and owner=='routes-a'
    regression=tracing and sys.platform!='win32' and not getattr(client,'route_core_only',False)
    if tracing:
        client.route_open_native={}
        if regression:natural_route_tooltip_hover(client)
        client.route_open_native['before']=route_toolbar_observation(client,'before')
        route_open_trace_begin(client,owner,identifier)
    try:
        client.click('button[aria-label="Путь вкладки"]')
        js_wait(client, '!!document.querySelector("[data-tab-route-root] [role=combobox]")', 'Actual route dialog absent')
        if tracing:
            require(client.js('window.__routeOpenDiagnostic?.state?.counts?.click?.trusted_route??-1')==1,
                    'Single physical route click did not reach trusted DOM')
        if regression:
            require(tooltip_input_is_empty(client.route_open_native['before'].get('hit_test')),
                    'Mapped native tooltip input shape was not empty')
            require(client.js('window.__routeOpenDiagnostic?.state?.counts?.click?.trusted_route??-1')==1,
                    'Single physical route click did not reach trusted DOM through tooltip')
            client.route_open_native['regression']={'mapped_tooltip_before_click':True,'input_empty_before_click':True,
                                                   'one_physical_click':True,'trusted_dom_click':True,'dialog_present':True}
    except Exception:
        if tracing:client.route_open_native['failure']=route_toolbar_observation(client,'failure')
        raise
    finally:
        if tracing:route_open_trace_finish(client)
    choose(client, label)
    if screenshot:
        from PIL import ImageGrab
        painted(client)
        ImageGrab.grab(**({'xdisplay':client.display} if sys.platform!='win32' else {})).save(screenshot)
    client.text_button('Применить путь')


def assert_receipt(fixture, case, lane, auth, storage, *, tls=False):
    records = wait_for(lambda: fixture.records(case), 'No actual CEF origin receipt for ' + case, 35)
    require(all(r['lane'] == lane and r['auth'] == auth and r['storage'] == storage and r['tls'] == tls for r in records),
            'Origin route/auth/storage mismatch for ' + case)


def apply_route_ack(client, owner, identifier, label, expected, screenshot=None):
    """Physical selector actions, then authoritative native replacement ACK."""
    before = row(client, owner, identifier)
    apply_ui(client, label, owner, identifier, screenshot=screenshot)
    js_wait(client, '!document.querySelector("[data-tab-route-root]")', 'Route selection stayed open; native application was not confirmed', 40)

    def replacement():
        current = row(client, owner, identifier)
        return current if (current['route'] == expected
                           and current['route_generation'] == before['route_generation'] + 1
                           and current['browser_id'] > 0 and current['browser_id'] != before['browser_id']
                           and not current['route_pending'] and not current.get('route_error')) else None

    return wait_for(replacement, 'Route UI did not produce the acknowledged replacement browser', 40)


def run(args):
    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {'schema': 1, 'completed': False, 'checks': [], 'platform': sys.platform, 'coverage': {'http_ui': 'not_executed', 'https_connect': 'not_executed', 'refusal': 'not_executed', 'cold_restore': 'not_executed', 'transport_down': 'not_executed', 'rtc_dns_udp': 'not_executed', 'historical_cookies': 'not_executed', 'worker_ready_context': 'not_executed', 'worker_startup_pre_admission': 'not_executed'}}
    report['diagnostic_only']=bool(args.core_routes_only or getattr(args,'tooltip_native_trace',None))
    report['tooltip_overlap_required']=not args.core_routes_only
    report['coverage']['native_tooltip_overlap']='not_executed' if not args.core_routes_only else 'not_requested_diagnostic_only'
    device = fixture = stun = socks = None
    try:
        require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Only an ephemeral GitHub runner may run this native test')
        require(args.tls_cert and args.tls_key, 'HTTPS acceptance NOT EXECUTED: supply pretrusted runner-only CA-signed 127.0.0.1 certificate/key')
        stun_host = 'nexusbro-route-stun.test'
        resolved = {answer[4][0] for answer in socket.getaddrinfo(stun_host, None, socket.AF_INET, socket.SOCK_DGRAM)}
        require(resolved == {'127.0.0.1'}, 'Runner prerequisite missing: STUN fixture hostname must resolve only to 127.0.0.1')
        stun = StunFixture()
        fixture = Fixture(args.tls_cert, args.tls_key, 'stun:' + stun_host + ':' + str(stun.port))
        socks = SocksFixture(fixture)
        exe, cef = pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve()
        require(exe.is_file() and cef.is_dir(), 'Built native app/CEF prerequisite missing')
        device = WindowsDevice(out / 'device', exe, cef) if sys.platform == 'win32' else Device('routes', out / 'device', exe, cef)
        if getattr(args,'tooltip_native_trace',None):device.tooltip_trace_library=args.tooltip_native_trace
        config = settings('grafit', 'unused')
        config['vyhody'] = [{'id': 'lane-' + lane, 'nazvanie': 'Fixture ' + lane,
                             'adres': 'http://127.0.0.1:' + str(fixture.proxies[lane]), 'proxy_uzel': ''} for lane in ('A', 'B')]
        config['vyhody'].append({'id':'lane-socks','nazvanie':'Fixture SOCKS5','adres':socks.url,'proxy_uzel':''})
        write_json(device.data / 'nastrojki.json', config)
        supers = {name: super_record(name, name, 'about:blank', proxy=config['vyhody'][0]['adres']) for name in ('routes-a', 'routes-b')}
        supers['routes-direct'] = super_record('routes-direct', 'routes-direct', 'about:blank', proxy='')
        for name in ('routes-historical', 'routes-neighbor', 'routes-worker'):
            supers[name] = super_record(name, name, 'about:blank', proxy=config['vyhody'][0]['adres'])
        write_json(device.data / 'vkladki.json', list(supers.values()))

        def tab(identifier, case, route, seed=None, tls=False, rtc=False):
            return {'id': identifier, 'adres': fixture.url(case, seed, tls, rtc), 'zagolovok': case,
                    'route': route, 'zakreplena': False, 'otlozhena': False, 'privatnaya': False}
        inherit, direct = {'kind': 'inherit'}, {'kind': 'direct'}
        connection = lambda lane: {'kind': 'connection', 'id': 'lane-' + lane}
        tabs = [tab(1, 'seed-a', inherit, 'A'), tab(2, 'seed-b', connection('B'), 'B'),
                tab(3, 'share-a', connection('A')), tab(4, 'seed-direct', direct, 'Direct'),
                tab(5, 'share-direct', direct), tab(6, 'https-b', connection('B'), tls=True),
                tab(7, 'blocked-unknown', connection('missing')), tab(8, 'draft', inherit), tab(9, 'media', inherit),
                tab(10, 'rtc-vpn', inherit, rtc=True), tab(11, 'interactive', inherit), tab(12, 'route-loading', inherit), tab(13, 'socks-http', inherit)]
        # Delayed tabs are activated explicitly, in order, to make first-request cookie witnesses deterministic.
        for t in tabs[1:]:
            t['otlozhena'] = True
        session_path = device.data / 'profili' / 'routes-a' / 'obychnye.json'
        write_json(session_path, {'vkladki': tabs, 'aktivnaya': 1, 'sleduyushchaya': 14, 'zakrytye': []})
        write_json(device.data / 'profili' / 'routes-b' / 'obychnye.json',
                   {'vkladki': [tab(1, 'other-super', inherit)], 'aktivnaya': 1, 'sleduyushchaya': 2, 'zakrytye': []})
        write_json(device.data / 'profili' / 'routes-direct' / 'obychnye.json',
                   {'vkladki': [tab(1, 'direct-super-seed', inherit, 'Direct'),
                                {**tab(2, 'direct-super-share', direct), 'otlozhena': True},
                                {**tab(3, 'rtc-direct', inherit, rtc=True), 'otlozhena': True}],
                    'aktivnaya': 1, 'sleduyushchaya': 4, 'zakrytye': []})
        historical_path = device.data / 'profili' / 'routes-historical' / 'obychnye.json'
        neighbor_path = device.data / 'profili' / 'routes-neighbor' / 'obychnye.json'
        historical_case = 'historical-clear-' + str(time.time_ns())
        neighbor_case = 'historical-neighbor-' + str(time.time_ns())
        write_json(historical_path, {'vkladki': [tab(1, 'historical-inherit', inherit),
                   {**tab(2, 'historical-seed', connection('B'), 'Historical'), 'otlozhena': True}],
                   'aktivnaya': 1, 'sleduyushchaya': 3, 'zakrytye': []})
        # Persistence is opt-in for these two witnesses, leaving every original
        # session-cookie/restart assertion and A/B seed case unchanged.
        saved_history = json.loads(historical_path.read_text(encoding='utf-8'))
        saved_history['vkladki'][1]['adres'] = fixture.url('historical-seed', 'Historical', persist=True)
        write_json(historical_path, saved_history)
        write_json(neighbor_path, {'vkladki': [{**tab(1, 'neighbor-seed', inherit, 'Neighbor'),
                   'adres': fixture.url('neighbor-seed', 'Neighbor', persist=True)}],
                   'aktivnaya': 1, 'sleduyushchaya': 2, 'zakrytye': []})
        write_json(device.data / 'profili' / 'routes-worker' / 'obychnye.json',
                   {'vkladki': [{'id': 1, 'adres': fixture.worker_url('worker-first'), 'zagolovok': 'Worker route fixture',
                                 'route': connection('B'), 'otlozhena': False}],
                    'aktivnaya': 1, 'sleduyushchaya': 2, 'zakrytye': []})
        client = device.start()
        client.route_core_only=args.core_routes_only
        client.route_trace_device=device;client.route_trace_out=out
        activate_saved(client, 'routes-a')
        assert_receipt(fixture, 'seed-a', 'A', 'A', 'A')
        for identifier, case, lane, auth, storage, tls in [
                (2, 'seed-b', 'B', 'B', 'B', False), (3, 'share-a', 'A', 'A', 'A', False),
                (4, 'seed-direct', 'Direct', 'Direct', 'Direct', False),
                (5, 'share-direct', 'Direct', 'Direct', 'Direct', False),
                (6, 'https-b', 'B', 'B', 'empty', True)]:
            select(client, 'routes-a', identifier)
            assert_receipt(fixture, case, lane, auth, storage, tls=tls)
        with fixture.lock:
            require(any(t['lane'] == 'B' and t['port'] == fixture.https for t in fixture.tunnels), 'No actual HTTPS CONNECT witnessed')
            first_b = [r for r in fixture.receipts if r.get('case') == 'seed-b' and r['path'] == '/page']
            require(first_b and first_b[0]['auth'] is None, 'Different route inherited another profile before first navigation')
        report['coverage']['https_connect'] = 'passed'
        report['checks'].append('http_https_connect_same_effective_key_shares_different_key_isolates')
        # The owner's first route choice must work before any website loads.
        before_ids = {t['id'] for t in rows(client, 'routes-a')['vkladki']}
        call(client, 'obychnaya_komanda', id='routes-a', komanda={'deystvie': 'novaya', 'adres': 'about:blank'})

        def owned_blank():
            current = rows(client, 'routes-a')
            return next((t for t in current['vkladki'] if t['id'] not in before_ids
                         and current['aktivnaya'] == t['id'] and t['adres'] == 'about:blank'
                         and t['browser_id'] > 0 and not t['zagruzka'] and not t['route_pending']), None)

        blank = wait_for(owned_blank, 'New owned about:blank tab did not acquire a real browser', 30)
        require(not blank['nazad'], 'Owned blank bootstrap added an artificial Back entry')
        apply_route_ack(client, 'routes-a', blank['id'], 'Fixture B', connection('B'))
        require(not row(client, 'routes-a', blank['id'])['nazad'], 'Routed blank bootstrap added an artificial Back entry')
        blank_case = 'blank-to-b-' + str(time.time_ns())
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': blank['id'], 'adres': fixture.url(blank_case)})
        assert_receipt(fixture, blank_case, 'B', 'B', 'B')
        report['checks'].append('owned_about_blank_actual_route_selector_b_ack_then_b_auth_storage_network')
        start = time.monotonic()
        wait_for(lambda: any(r['time'] > start for r in fixture.records('seed-a')) and any(r['time'] > start for r in fixture.records('seed-b')),
                 'Two differently routed native pages did not run concurrently', 20)
        report['checks'].append('two_native_pages_concurrent_different_routes')
        activate_saved(client, 'routes-b')
        assert_receipt(fixture, 'other-super', 'A', None, 'empty')
        report['checks'].append('cross_super_profile_isolation')
        activate_saved(client, 'routes-direct')
        assert_receipt(fixture, 'direct-super-seed', 'Direct', 'Direct', 'Direct')
        select(client, 'routes-direct', 2)
        assert_receipt(fixture, 'direct-super-share', 'Direct', 'Direct', 'Direct')
        with fixture.lock:
            first_direct = [r for r in fixture.receipts
                            if r.get('case') == 'direct-super-seed' and r['path'] == '/page']
            require(first_direct and first_direct[0]['auth'] is None,
                    'Direct Super inherited another Super authentication profile')
        report['checks'].append('direct_super_inherit_matches_explicit_direct_profile_and_network')
        # Browser DNS + real ICE/STUN positive control precedes the VPN leak
        # check. A fixture-only protocol success cannot satisfy this assertion.
        def rtc_events(case):
            return [r for r in fixture.requests(case) if r['path'] == '/rtc']

        def complete_rtc(case, lane, minimum_seconds=0):
            events = rtc_events(case)
            names = {r['rtc_event'] for r in events}
            if not {'started', 'gathering-done', 'closed'} <= names or names & {'timeout', 'error'}:
                return None
            if any(r['lane'] != lane for r in events):
                return None
            started = next(r['time'] for r in events if r['rtc_event'] == 'started')
            return events if time.monotonic() - started >= minimum_seconds else None

        def start_rtc(owner, identifier, began):
            select(client, owner, identifier, timeout=max(.1, 15 - (time.monotonic() - began)))

        direct_started = time.monotonic()
        start_rtc('routes-direct', 3, direct_started)
        def direct_rtc_witness():
            events = complete_rtc('rtc-direct', 'Direct')
            return events if (events and any(p['binding'] for p in stun.since(direct_started))
                              and any(r['candidate_type'] == 'srflx' and r['candidate_address'] == '203.0.113.10'
                                      for r in events)) else None
        wait_for(direct_rtc_witness, 'Direct hostname ICE did not produce actual STUN UDP + mapped srflx candidate',
                 max(.1, 15 - (time.monotonic() - direct_started)))
        report['checks'].append('direct_hostname_rtc_real_udp_and_srflx_positive_control')
        # Direct PC has closed after its final HTTP receipt. No remote SDP is
        # ever installed; the documentation-only mapped IP is not contacted.
        time.sleep(.3)
        vpn_started = time.monotonic()
        start_rtc('routes-a', 10, vpn_started)
        wait_for(lambda: complete_rtc('rtc-vpn', 'A', minimum_seconds=5),
                 'VPN ICE did not genuinely start/complete/close within bounded no-UDP observation',
                 max(.1, 15 - (time.monotonic() - vpn_started)))
        require(not stun.since(vpn_started), 'VPN profile leaked non-proxied UDP to hostname STUN fixture')
        report['coverage']['rtc_dns_udp'] = 'passed'
        report['checks'].append('vpn_hostname_rtc_completed_no_udp_at_least_five_seconds')
        select(client, 'routes-a', 7)
        wait_for(lambda: row(client, 'routes-a', 7).get('route_error'), 'Unknown saved ID was not explicitly unavailable')
        require(not fixture.requests('blocked-unknown'), 'Unknown ID fell back to a network route')
        report['checks'].append('unknown_connection_id_fail_closed')
        # Real UI change and native ACK: route/generation/browser identity are read only after the transaction.
        select(client, 'routes-a', 5)
        before = row(client, 'routes-a', 5)
        apply_ui(client, 'Fixture B', 'routes-a', 5)
        js_wait(client, '!document.querySelector("[data-tab-route-root]")', 'Route UI did not close after ACK', 40)
        def acknowledged_replacement():
            current = row(client, 'routes-a', 5)
            if (current['route'] == connection('B')
                    and current['route_generation'] == before['route_generation'] + 1
                    and current['browser_id'] != before['browser_id']
                    and not current['route_pending'] and not current.get('route_error')):
                return current
            return None
        after = wait_for(acknowledged_replacement,
                         'Native ACK was not followed by the authoritative replacement session row', 40)
        require(after['route'] == connection('B') and after['route_generation'] == before['route_generation'] + 1
                and after['browser_id'] != before['browser_id'] and not after['route_pending'] and not after.get('route_error'),
                'Actual route UI did not commit an acknowledged replacement browser')
        wait_for(lambda: any(r['lane'] == 'B' and r['auth'] == 'B' and r['storage'] == 'B' for r in fixture.records('share-direct')),
                 'Acknowledged route UI change did not reach B profile/network')
        report['coverage']['http_ui'] = 'passed'
        report['checks'].append('actual_route_selector_native_ack_network_and_profile')
        # A real unsaved-data beforeunload refusal still cancels graceful close.
        select(client, 'routes-a', 8)
        assert_receipt(fixture, 'draft', 'A', 'A', 'A')
        bounds = client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return [r.x,r.y]})()')
        if sys.platform=='win32':
            client.mouse(bounds[0]+50,bounds[1]+30)
            import pyautogui
            pyautogui.write('synthetic unsaved draft',interval=.04)
        else:
            # Require the actual synthetic input's pixels and a held native
            # CEF identity, not only a successful HTTP receipt or shell PID.
            from youtube_paired_native import focus_normal_cef
            from x11_owned_cef_focus import NativeX11
            from resource_soak import kernel_birth
            report['draft_focus_setup']=focus_normal_cef(client,device)
            point=painted_interaction_point(client,offset=(50,30),color=(227,242,253))
            client.mouse(*point)
            def focused_selected_engine():
                native=NativeX11(device.display)
                try:pid=native.pid_of(native.input_focus())
                finally:native.close()
                state=client.command('super_lifecycle_snapshot')
                engines=[e for e in state.get('engines',[]) if e.get('id')=='routes-a']
                if state.get('schema')!=1 or len(engines)!=1 or engines[0].get('owned_complete') is not True:return False
                return any(m.get('pid')==pid and m.get('clock')=='linux_ticks' and m.get('birth')==kernel_birth(pid,'linux_ticks') for m in engines[0].get('owned',[]))
            wait_for(focused_selected_engine,'Draft keyboard focus not bound to selected native engine',10)
            subprocess.run(['xdotool','type','--clearmodifiers','--','synthetic unsaved draft'],env={**os.environ,'DISPLAY':device.display},check=True)
        wait_for(lambda:fixture.records('draft-typed'),'Real draft input did not reach the page')
        before_draft=row(client,'routes-a',8)
        apply_ui(client,'Fixture B','routes-a',8)
        wait_for(lambda:row(client,'routes-a',8).get('route_error'),'Actual beforeunload did not cancel route change',40)
        after_draft=row(client,'routes-a',8)
        require(after_draft['route']==before_draft['route'] and after_draft['browser_id']==before_draft['browser_id']
                and after_draft['route_generation']==before_draft['route_generation']+1 and not after_draft['route_pending'],
                'Beforeunload refusal changed the unsaved document or profile')
        js_wait(client,'document.querySelector("[data-tab-route-root] [role=alert]")?.textContent.includes("Сохраните")',
                'Actual refusal did not show a safe actionable reason',20)
        client.text_button('Отмена')
        report['checks'].append('actual_beforeunload_route_refusal_preserves_unsaved_page_and_shows_reason')
        report['coverage']['refusal']='passed'

        # Blank forms, canvas/sessionStorage and an ordinary trusted click are
        # valid pages for an explicit route reload, unlike automatic eviction.
        select(client,'routes-a',11)
        assert_receipt(fixture,'interactive','A','A','A')
        point=painted_interaction_point(client)
        client.mouse(*point)
        wait_for(lambda:fixture.records('route-interaction'),'Actual ordinary CEF interaction did not occur')
        apply_route_ack(client,'routes-a',11,'Fixture B',connection('B'),screenshot=out/'native-route-selector-compact.png')
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':11,'adres':fixture.url('interactive-after')})
        assert_receipt(fixture,'interactive-after','B','B','B')
        apply_route_ack(client,'routes-a',11,'Fixture A',connection('A'))
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':11,'adres':fixture.url('interactive-returned')})
        assert_receipt(fixture,'interactive-returned','A','A','A')
        report['checks'].append('interactive_page_with_form_canvas_session_storage_manual_path_change_and_auth_isolation')

        # Explicitly changing a media page reloads that target. Background
        # automatic media protection is exercised independently by AUTO tests.
        select(client,'routes-a',9)
        assert_receipt(fixture,'media','A','A','A')
        # Loading receipts do not prove compositor presentation or delivery of
        # the physical gesture to CEF. Observe paint, then click exactly once.
        client.mouse(*painted_interaction_point(client,offset=(140,80),color=(24,85,65)))
        try:
            wait_for(lambda:fixture.records('media-running'),'Actual CEF audio did not start')
        finally:
            report['media_start']={
                'physical_click_count':1,
                'trusted_click_received':bool(fixture.records('media-trusted-click')),
                'context_created':bool(fixture.records('media-context-created')),
                'resume_resolved':bool(fixture.records('media-resume-resolved')),
                'running_received':bool(fixture.records('media-running')),
                'start_error_received':bool(fixture.records('media-start-error')),
            }
        apply_route_ack(client,'routes-a',9,'Fixture B',connection('B'))
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':9,'adres':fixture.url('media-reloaded')})
        assert_receipt(fixture,'media-reloaded','B','B','B')
        require(row(client,'routes-a',8)['browser_id']==before_draft['browser_id'],'Changing another route closed the protected draft')
        report['checks'].append('user_requested_media_route_reload_changes_only_target_not_protected_neighbor')

        # Keep the old document genuinely loading; changing its path must not
        # require that the currently failing connection first finishes loading.
        select(client,'routes-a',12,allow_loading=True)
        wait_for(lambda:fixture.loading_started.is_set() and row(client,'routes-a',12)['zagruzka'],
                 'Controlled unfinished subresource did not keep the real page loading',20)
        apply_route_ack(client,'routes-a',12,'Fixture B',connection('B'))
        wait_for(lambda:any(r['lane']=='B' for r in fixture.requests('route-loading')),
                 'Route change of a loading page never reached the selected proxy',20)
        fixture.release_loading()
        wait_for(lambda:not row(client,'routes-a',12)['zagruzka'],'New route page did not finish after releasing its subresource',20)
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':12,'adres':fixture.url('loading-after')})
        assert_receipt(fixture,'loading-after','B','B','B')
        report['checks'].append('manual_path_change_during_real_unfinished_load_no_direct_fallback')
        # Match the reported SOCKS path with real RFC1929 authentication and
        # hostname forwarding, not just an HTTP-proxy substitute.
        select(client,'routes-a',13)
        assert_receipt(fixture,'socks-http','A','A','A')
        apply_route_ack(client,'routes-a',13,'Fixture SOCKS5',{'kind':'connection','id':'lane-socks'})
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':13,'adres':fixture.url('socks-http-after')})
        assert_receipt(fixture,'socks-http-after','Socks',None,'empty')
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':13,'adres':fixture.url('socks-seeded','Socks')})
        assert_receipt(fixture,'socks-seeded','Socks','Socks','Socks')
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':13,'adres':fixture.url('socks-https',tls=True)})
        assert_receipt(fixture,'socks-https','Socks','Socks','empty',tls=True)
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':13,'adres':fixture.url('socks-hostname').replace('127.0.0.1','localhost',1)})
        assert_receipt(fixture,'socks-hostname','Socks',None,'empty')
        require(any(e.get('authenticated') and e.get('atyp')==3 and e.get('host')=='localhost' for e in socks.events),
                'Actual SOCKS5 transport did not receive the hostname via ATYP=3')
        require(any(e.get('authenticated') and e.get('port')==fixture.https for e in socks.events),
                'Trusted HTTPS did not pass through authenticated SOCKS5')
        apply_route_ack(client,'routes-a',13,'Fixture A',connection('A'))
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':13,'adres':fixture.url('socks-returned-a')})
        assert_receipt(fixture,'socks-returned-a','A','A','A')
        apply_route_ack(client,'routes-a',13,'Fixture SOCKS5',{'kind':'connection','id':'lane-socks'})
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':13,'adres':fixture.url('socks-returned-s')})
        assert_receipt(fixture,'socks-returned-s','Socks','Socks','Socks')
        socks.close()
        before_socks_down=row(client,'routes-a',13)
        socks_down_url=fixture.url('socks-down')
        call(client,'obychnaya_komanda',id='routes-a',komanda={'deystvie':'pereyti','id':13,'adres':socks_down_url})
        wait_for(lambda:row(client,'routes-a',13)['adres']==socks_down_url and not row(client,'routes-a',13)['zagruzka'],
                 'Closed SOCKS5 did not produce a finished network error',40)
        require(not fixture.requests('socks-down'),'Unavailable SOCKS5 silently fell back to Direct')
        require(row(client,'routes-a',13)['route']==before_socks_down['route'] and row(client,'routes-a',13)['browser_id']==before_socks_down['browser_id'],
                'Unavailable SOCKS5 changed the selected route or profile')
        report['socks_transport_witness']={'authenticated':True,'hostname_atyp3':True,'trusted_https':True,'profile_auth_isolated':True,'no_direct_fallback':True}
        report['checks'].append('native_authenticated_socks5_http_https_hostname_and_profile_switch')
        if not args.core_routes_only and sys.platform!='win32':
            report['coverage']['native_tooltip_overlap']='passed'
        report['checks'].append('native_socks5_down_no_direct_fallback_or_profile_change')
        # Prepare independent persistent state before the existing owned restart.
        # No original routes-a/routes-b profile is cleared or reseeded here.
        activate_saved(client, 'routes-historical')
        select(client, 'routes-historical', 2)
        assert_receipt(fixture, 'historical-seed', 'B', 'Historical', 'Historical')
        call(client, 'obychnaya_komanda', id='routes-historical', komanda={
             'deystvie': 'pereyti', 'id': 2, 'adres': fixture.url(historical_case)})
        assert_receipt(fixture, historical_case, 'B', 'Historical', 'Historical')
        select(client, 'routes-historical', 1)
        activate_saved(client, 'routes-neighbor')
        assert_receipt(fixture, 'neighbor-seed', 'A', 'Neighbor', 'Neighbor')
        call(client, 'obychnaya_komanda', id='routes-neighbor', komanda={
             'deystvie': 'pereyti', 'id': 1, 'adres': fixture.url(neighbor_case)})
        assert_receipt(fixture, neighbor_case, 'A', 'Neighbor', 'Neighbor')

        def worker_witness(case, existing):
            requests = fixture.requests(case)
            require(all(r['lane'] == 'B' for r in requests), 'Worker owner used A/Direct instead of B')
            status = [r for r in requests if r['path'] == '/worker/page-status']
            proof = [r for r in requests if r['path'] == '/worker/receipt' and r['worker_proof'] and r['worker_event'] == 'message']
            require(not any(r['path'] == '/worker/page-error' for r in requests), 'Actual worker registration/message failed')
            return proof if proof and any(r['worker_existing'] == str(existing).lower() and r['secure'] == 'true' for r in status) else None

        activate_saved(client, 'routes-worker')
        wait_for(lambda: worker_witness('worker-first', False), 'No actual B worker fetch/secure-context positive receipt', 20)
        # Change only the saved client URL. After restart, getRegistration must
        # find the persisted active worker; a fresh registration cannot pass.
        call(client, 'obychnaya_komanda', id='routes-worker', komanda={
             'deystvie': 'pereyti', 'id': 1, 'adres': fixture.worker_url('worker-restored')})
        wait_for(lambda: worker_witness('worker-restored', True), 'Installed B worker could not handle a second native client', 20)

        def gap_state_persisted():
            try:
                h = json.loads(historical_path.read_text(encoding='utf-8'))
                n = json.loads(neighbor_path.read_text(encoding='utf-8'))
                w = json.loads((device.data / 'profili' / 'routes-worker' / 'obychnye.json').read_text(encoding='utf-8'))
                return (h['aktivnaya'] == 1 and h['vkladki'][1]['adres'] == fixture.url(historical_case)
                        and h['vkladki'][1]['route'] == connection('B')
                        and n['vkladki'][0]['adres'] == fixture.url(neighbor_case)
                        and w['vkladki'][0]['adres'] == fixture.worker_url('worker-restored'))
            except (OSError, ValueError, KeyError, IndexError):
                return False
        wait_for(gap_state_persisted, 'Historical/worker non-seed URLs were not persisted before restart', 30)
        def cookie_metadata_snapshot(phase):
            # Diagnostics only. Native post-restart cookieStats and the neighbor
            # server's actual persistent auth are the functional requirements.
            # Never inspect or serialize encrypted/plain cookie token values.
            import contextlib
            import itertools
            import sqlite3
            snapshot = {}
            for owner in ('routes-historical', 'routes-neighbor'):
                profile = device.data / 'profili' / owner
                data = {'profile_exists': profile.is_dir(), 'cookie_files': []}
                try:
                    candidates = [p for p in itertools.islice(profile.rglob('*'), 2000)
                                  if p.is_file() and 'cookie' in p.name.lower()][:16]
                    for database in candidates:
                        entry = {'path': str(database.relative_to(profile)), 'bytes': database.stat().st_size}
                        data['cookie_files'].append(entry)
                        if database.name != 'Cookies':
                            continue
                        with database.open('rb') as stream:
                            entry['sqlite_header'] = stream.read(16) == b'SQLite format 3\0'
                        try:
                            with contextlib.closing(sqlite3.connect(database.resolve().as_uri() + '?mode=ro', uri=True, timeout=.1)) as db:
                                columns = [r[1] for r in db.execute('PRAGMA table_info(cookies)').fetchall()]
                                entry['columns'] = columns
                                if {'host_key', 'name', 'is_persistent'} <= set(columns):
                                    entry['fixture_cookie_count'] = db.execute("SELECT count(*) FROM cookies WHERE host_key IN ('127.0.0.1','.127.0.0.1') AND name='route_auth'").fetchone()[0]
                                    entry['persistent_fixture_cookie_count'] = db.execute("SELECT count(*) FROM cookies WHERE host_key IN ('127.0.0.1','.127.0.0.1') AND name='route_auth' AND is_persistent=1").fetchone()[0]
                        except sqlite3.Error as error:
                            entry['sqlite_error_type'] = type(error).__name__
                            entry['sqlite_error_name'] = getattr(error, 'sqlite_errorname', None)
                except OSError as error:
                    data['filesystem_error_type'] = type(error).__name__
                snapshot[owner] = data
            report.setdefault('cookie_metadata_diagnostics', {})[phase] = snapshot
            return snapshot

        def both_persistent_cookies(snapshot):
            return all([any(entry.get('persistent_fixture_cookie_count', 0) > 0
                            for entry in snapshot[owner]['cookie_files'])
                        for owner in ('routes-historical', 'routes-neighbor')])

        def persistent_cookie_barrier():
            return both_persistent_cookies(cookie_metadata_snapshot('live_before_restart'))

        report['cookie_observation_strategy'] = {'pre_stop': 'strict_live_positive', 'post_stop': 'diagnostic'}
        if sys.platform == 'win32':
            # Chromium can deny READ sharing while a Windows profile is live.
            # Preserve the full commit window, but do not call a locked file a
            # durable cookie. The post-stop disk positive below is mandatory.
            deadline = time.monotonic() + 40
            live_positive = False
            while time.monotonic() < deadline:
                live = cookie_metadata_snapshot('live_before_restart')
                if both_persistent_cookies(live):
                    live_positive = True
                    break
                time.sleep(.2)
            if not live_positive:
                both_locked = [live[owner].get('filesystem_error_type') == 'PermissionError'
                               and any(pathlib.PureWindowsPath(entry['path']).name == 'Cookies' and entry['bytes'] > 0
                                       for entry in live[owner]['cookie_files'])
                               for owner in ('routes-historical', 'routes-neighbor')]
                require(all(both_locked), 'Live Windows cookies were neither readable persistent positives nor two observed locked databases')
                report['cookie_observation_strategy']['pre_stop'] = 'live_unobservable'
            report['cookie_observation_strategy']['post_stop'] = 'strict_both_disk_positives_required'
        else:
            wait_for(persistent_cookie_barrier,
                     'Native persistent cookie writes were not durable before the owned restart', 40)

        def persisted_selected_b(address=None):
            # The native session callback updates RAM before writing disk. A
            # forced process stop must wait for actual persisted route intent.
            try:
                saved = json.loads(session_path.read_text(encoding='utf-8'))
                selected = next(t for t in saved['vkladki'] if t['id'] == 2)
                return saved if (saved['aktivnaya'] == 2 and selected['route'] == connection('B')
                                 and (address is None or selected['adres'] == address)) else None
            except (OSError, ValueError, KeyError, StopIteration, TypeError):
                return None

        # Stop only our owned process tree, reorder IDs, and cold restore selected B.
        select(client, 'routes-a', 2)
        wait_for(persisted_selected_b, 'Selected B route was not persisted before first cold restart', 30)
        cookie_metadata_snapshot('before_restart')
        device.stop_app()
        post_stop_cookies = cookie_metadata_snapshot('after_stop')
        if sys.platform == 'win32':
            require(both_persistent_cookies(post_stop_cookies),
                    'After all owned processes stopped, both actual persistent cookie disk positives were required before restart')
            report['cookie_observation_strategy']['post_stop'] = 'actual_both_disk_positives_observed'
        config['vyhody'].reverse()
        write_json(device.data / 'nastrojki.json', config)
        client = device.start()
        client.route_core_only=args.core_routes_only
        client.route_trace_device=device;client.route_trace_out=out
        restored_at = time.monotonic()
        activate_saved(client, 'routes-a')
        wait_for(lambda: any(r['time'] > restored_at and r['lane'] == 'B' for r in fixture.records('seed-b')), 'Cold restore lost opaque B connection ID')
        require(row(client, 'routes-a', 2)['route'] == connection('B'), 'Restart rebound connection by list index')
        report['checks'].append('restart_opaque_id_survives_connection_reorder')
        # Historical B is on disk but has no Browser in this new engine. Stats
        # and clear must enumerate its closed-gate context without networking.
        historical_restart_at = time.monotonic()
        activate_saved(client, 'routes-historical')
        require(row(client, 'routes-historical', 2)['browser_id'] == 0, 'Historical B opened before clear')
        require(not [r for r in fixture.requests(historical_case) if r['time'] > restored_at],
                'Historical B requested its origin before clear')
        cookie_metadata_snapshot('after_restart_before_stats')
        before_clear = call(client, 'dannye_saytov_statistika', id='routes-historical', timeout=20)
        report['historical_native_cookie_counts'] = {'before_clear': before_clear.get('cookies')}
        require(type(before_clear.get('cookies')) is int and before_clear['cookies'] > 0,
                'No persistent historical cookie existed before the clear operation')
        cleared = call(client, 'ochistit_dannye_saytov', id='routes-historical', cookies=True, cache=False, timeout=20)
        require(cleared.get('cookies') is True and cleared.get('cache') is False, 'Actual cookie clear was not acknowledged')
        after_clear = call(client, 'dannye_saytov_statistika', id='routes-historical', timeout=20)
        report['historical_native_cookie_counts']['after_clear'] = after_clear.get('cookies')
        require(after_clear.get('cookies') == 0, 'Historical native cookies remained after clear')
        require(row(client, 'routes-historical', 2)['browser_id'] == 0, 'Stats/clear opened the historical B Browser')
        require(not [r for r in fixture.requests(historical_case) if r['time'] > restored_at],
                'Closed-gate historical context performed origin traffic during stats/clear')
        select(client, 'routes-historical', 2)
        wait_for(lambda: [r for r in fixture.records(historical_case) if r['time'] > historical_restart_at],
                 'Historical B never loaded after actual cookie clear', 20)
        historical_after = [r for r in fixture.requests(historical_case) if r['time'] > historical_restart_at]
        require(all(r['lane'] == 'B' and r['auth'] is None for r in historical_after),
                'Historical B authentication survived cookie clear or used a wrong lane')
        require(any(r['storage'] == 'Historical' for r in historical_after),
                'Cookies-only API unexpectedly erased localStorage (the UI explicitly excludes it)')
        neighbor_after_at = time.monotonic()
        activate_saved(client, 'routes-neighbor')
        wait_for(lambda: [r for r in fixture.records(neighbor_case) if r['time'] > neighbor_after_at],
                 'Neighbor did not produce a new persistent state witness after historical clear', 20)
        require(all(r['lane'] == 'A' and r['auth'] == 'Neighbor' and r['storage'] == 'Neighbor'
                    for r in fixture.records(neighbor_case) if r['time'] > neighbor_after_at),
                'Clearing one Super affected the neighboring Super cookie/storage profile')
        report['coverage']['historical_cookies'] = 'passed'
        report['checks'].append('historical_unopened_route_cookies_clear_no_network_ls_preserved_neighbor_untouched')
        worker_restored_at = time.monotonic()
        activate_saved(client, 'routes-worker')
        def restored_worker():
            proof = worker_witness('worker-restored', True)
            return [r for r in proof if r['time'] > worker_restored_at] if proof else None
        wait_for(restored_worker, 'Persisted worker produced no actual B fetch after restart', 20)
        with fixture.lock:
            worker_requests = [r for r in fixture.receipts if r['path'].startswith('/worker/')]
            require(worker_requests and all(r['lane'] == 'B' for r in worker_requests),
                    'Worker traffic escaped B before or after restart')
        report['coverage']['worker_ready_context'] = 'passed'
        report['checks'].append('actual_service_worker_fetch_b_only_persisted_registration_after_restart')
        # This positive is genuinely worker-originated, but requires a ready B
        # client. It does NOT test autonomous worker startup before admission.
        report['coverage']['worker_startup_pre_admission'] = 'not_executed'

        # Existing configured B remains selected when its actual transport dies.
        # A fresh URL makes every origin hit (even before page JS) a failure.
        select(client, 'routes-a', 1)
        healthy_at = time.monotonic()
        wait_for(lambda: any(r['time'] > healthy_at and r['lane'] == 'A' for r in fixture.records('seed-a')),
                 'A route was not live before B transport failure', 20)
        select(client, 'routes-a', 2)
        before_transport = row(client, 'routes-a', 2)
        fixture.stop_proxy('B')
        transport_down_at = time.monotonic()
        down_case = 'transport-down-' + str(time.time_ns())
        down_url = fixture.url(down_case)
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': 2, 'adres': down_url})

        def failed_load_settled():
            current = row(client, 'routes-a', 2)
            return current if current['adres'] == down_url and not current['zagruzka'] else None

        after_transport = wait_for(failed_load_settled, 'B transport failure did not settle its real navigation', 40)
        require(after_transport['route'] == connection('B')
                and after_transport['browser_id'] == before_transport['browser_id']
                and after_transport['route_generation'] == before_transport['route_generation'],
                'Transport failure changed B route intent/profile/browser')
        # Keep observing while A makes multiple real requests, catching delayed
        # Direct fallback as well as proving unrelated route continuity.
        wait_for(lambda: len([r for r in fixture.records('seed-a')
                              if r['time'] > transport_down_at and r['lane'] == 'A']) >= 2,
                 'Healthy A page stopped when B transport failed', 20)
        time.sleep(2)
        require(not fixture.requests(down_case), 'Dead configured B transport leaked a request through Direct/another route')
        require(row(client, 'routes-a', 2)['route'] == connection('B'), 'Dead transport silently changed saved B intent')
        report['coverage']['transport_down'] = 'pending_final_no_fallback_check'
        # Preserve down_case's permanent zero-origin invariant. Recovery uses
        # another URL whose first load genuinely fails on the dead B transport.
        recovery_case = 'recover-network-error-' + str(time.time_ns())
        recovery_url = fixture.url(recovery_case)
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': 2, 'adres': recovery_url})
        wait_for(lambda: row(client, 'routes-a', 2)['adres'] == recovery_url
                 and not row(client, 'routes-a', 2)['zagruzka'], 'Native B network-error page did not settle', 40)
        require(not fixture.requests(recovery_case), 'Dead B reached recovery URL before any route change')
        apply_route_ack(client, 'routes-a', 2, 'Fixture A', connection('A'))
        assert_receipt(fixture, recovery_case, 'A', 'A', 'A')
        require(row(client, 'routes-a', 2)['adres'] == recovery_url, 'Network-error recovery silently changed requested URL')
        require(not fixture.requests(down_case), 'Network-error route recovery revisited the permanent no-fallback URL')
        report['checks'].append('native_network_error_actual_route_selector_a_recovers_same_url_auth_storage')
        # Selecting the configured but dead B is still an explicit route
        # transaction. It must ACK intent without sending this URL via A/Direct.
        recovery_main_loads = sum(r['path'] == '/page' for r in fixture.requests(recovery_case))
        apply_route_ack(client, 'routes-a', 2, 'Fixture B', connection('B'))
        wait_for(lambda: row(client, 'routes-a', 2)['adres'] == recovery_url
                 and not row(client, 'routes-a', 2)['zagruzka'], 'Explicit switch back to dead B did not settle', 40)
        # Retired A-page heartbeat fetches may finish after replacement ACK.
        # A fallback navigation would add another origin /page GET, which must
        # never happen even if it reuses the old A lane/auth/profile.
        time.sleep(2)
        require(sum(r['path'] == '/page' for r in fixture.requests(recovery_case)) == recovery_main_loads
                and all(r['lane'] == 'A' for r in fixture.requests(recovery_case)),
                'Switch to unavailable B leaked the recovered URL through another route')
        require(not fixture.requests(down_case), 'Switch back to B broke permanent transport-down no-fallback invariant')
        report['checks'].append('network_error_recovery_can_reselect_unavailable_b_without_direct')
        # Restore the original URL while B is still down, preserving the
        # deleted-ID cold-restore test's unique prior URL and B selection.
        seed_b_url = tabs[1]['adres']
        call(client, 'obychnaya_komanda', id='routes-a',
             komanda={'deystvie': 'pereyti', 'id': 2, 'adres': seed_b_url})
        wait_for(lambda: row(client, 'routes-a', 2)['adres'] == seed_b_url
                 and not row(client, 'routes-a', 2)['zagruzka'], 'B URL restore did not settle while transport stayed down', 40)
        wait_for(lambda: persisted_selected_b(seed_b_url), 'Selected B URL/intent was not persisted while its transport was down', 30)
        wait_for(persisted_selected_b, 'Selected B route was not persisted before deleted-ID restart', 30)
        device.stop_app()
        config['vyhody'] = [c for c in config['vyhody'] if c['id'] != 'lane-B']
        write_json(device.data / 'nastrojki.json', config)
        before_restart = len(fixture.requests('seed-b'))
        client = device.start()
        client.route_core_only=args.core_routes_only
        client.route_trace_device=device;client.route_trace_out=out
        activate_saved(client, 'routes-a')
        wait_for(lambda: row(client, 'routes-a', 2).get('route_error'), 'Deleted ID was not blocked on cold restore', 40)
        time.sleep(2)
        require(len(fixture.requests('seed-b')) == before_restart and not fixture.requests('blocked-unknown'), 'Removed/unknown ID silently used Direct or stale proxy')
        require(not fixture.requests(down_case), 'Delayed dead-transport fallback reached the origin')
        require(sum(r['path'] == '/page' for r in fixture.requests(recovery_case)) == recovery_main_loads,
                'Delayed switch-back fallback navigated the recovered URL through another route')
        report['coverage']['transport_down'] = 'passed'
        report['checks'].append('configured_transport_down_no_direct_fallback_unaffected_a_live')
        report['checks'].append('deleted_connection_cold_restore_fail_closed')
        with fixture.lock:
            worker_requests = [r for r in fixture.receipts if r['path'].startswith('/worker/')]
            require(worker_requests and all(r['lane'] == 'B' for r in worker_requests),
                    'Delayed worker traffic escaped the selected B lane')
            require(not any(r['time'] > transport_down_at for r in worker_requests),
                    'Worker origin traffic survived the stopped configured B transport')
        report['coverage']['cold_restore'] = 'passed'
        report['completed'] = True
    except Exception as error:
        # No credential/raw server receipts are written to artifacts.
        report['error'] = type(error).__name__ + ': ' + str(error)
        if device and sys.platform == 'win32':
            try:
                report['inspector_on_failure'] = device.startup_diagnostics()
            except Exception as diagnostic_error:
                report['inspector_diagnostic_error'] = type(diagnostic_error).__name__
        if device and device.client:
            persist_route_open_report(report,device.client)
            try:
                report['shell_text_on_failure'] = device.client.js('document.body.innerText').strip()[:12000]
                report['session_on_failure'] = rows(device.client, 'routes-a')
            except Exception as diagnostic_error:
                report['diagnostic_error'] = type(diagnostic_error).__name__
        if device and getattr(device.client,'route_forbid_failure_screenshot',False):
            reason=getattr(device.client,'route_screenshot_block_reason','incomplete_observation')
            report['failure_screenshot_blocked_native_ownership']=True
            report['route_screenshot_block_reason']=reason if reason in ('foreign_cover','incomplete_observation') else 'incomplete_observation'
            if reason=='foreign_cover':report['failure_screenshot_blocked_nonowned_cover']=True
        if device and not getattr(device.client,'route_forbid_failure_screenshot',False):
            # Startup/inspector failures need a native screen too, even before
            # a CDP client exists. Capture before terminating the owned app.
            try:
                from PIL import ImageGrab
                (ImageGrab.grab() if sys.platform == 'win32' else ImageGrab.grab(xdisplay=device.display)).save(out / 'failure.png')
            except Exception as screenshot_error:
                report['screenshot_error'] = type(screenshot_error).__name__
        raise
    finally:
        if device:
            persist_route_open_report(report,device.client,device)
            device.close()
            if hasattr(device,'tooltip_trace_library'):report['gtk_tooltip_trace']=tooltip_trace_records(device)
        if socks:
            socks.close()
        if fixture:
            fixture.close()
        if stun:
            stun.close()
        (out / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')


if __name__ == '__main__':
    if sys.argv[1:]==['--self-test-route-open-trace']:
        print(json.dumps(route_open_trace_self_test()));raise SystemExit(0)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', required=True)
    parser.add_argument('--cef', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--tls-cert')
    parser.add_argument('--tls-key')
    parser.add_argument('--tooltip-native-trace',help='Diagnostic only: exact private CI pass-through GTK observer; never release acceptance')
    parser.add_argument('--core-routes-only',action='store_true',help='Diagnostic only: route checks without the separate GTK tooltip-overlap scenario; never release acceptance')
    run(parser.parse_args())
