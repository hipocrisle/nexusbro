#!/usr/bin/env python3
"""Real Ctrl+S/native SaveAs and offline MHTML render in a fresh browser profile.
Only own loopback fixtures and owned virtual displays. No synthetic save IPC,
HTML injection, owner cookies, system installs, public network or JS clicks.
Preparation alone is never a native PASS.
"""
import argparse
import hashlib
import io
import json
import os
import re
import sys
import subprocess
import threading
import time
from email import policy
from email.parser import BytesParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from security_native import Device, call, require, wait_for
from tab_routes import WindowsDevice
from youtube_paired_native import (super_record, switch_super, rows,
    focus_normal_cef, click_control)
from youtube_player_native import (OwnedWindowManager, foreground,
    close_for_privacy, native_app_identity, native_app_identity_alive)
from youtube_download_native import dialogs, choose, keys
from download_fixture_target import fixture_target

COLORS = ((44, 190, 97), (41, 93, 219), (201,71,107))


class Fixture:
    def __init__(self):
        from PIL import Image
        encoded = io.BytesIO()
        Image.new('RGB', (40, 40), COLORS[1]).save(encoded, format='PNG')
        self.png = encoded.getvalue()
        self.counts = {}
        outer = self
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                path, _ = fixture_target(self.path, outer.origin)
                payloads = {
                    '/page.html': ('text/html', b'<!doctype html><meta charset=utf-8><title>Save fixture</title><link rel=stylesheet href=/style.css><div id=tile></div><img src=/image.png width=100 height=100><iframe src=/frame.html></iframe>'),
                    '/style.css': ('text/css', b'body{margin:0;background:#fff}#tile{width:100px;height:100px;background:rgb(44,190,97)}img{display:block}iframe{display:block;width:100px;height:100px;border:0}'),
                    '/image.png': ('image/png', outer.png),
                    '/frame.html': ('text/html', b'<!doctype html><title>Embedded fixture</title><style>body{margin:0;background:rgb(201,71,107)}</style><p>OFFLINE_FRAME_MARKER</p>'),
                }
                kind, body = payloads.get(path, ('text/plain', b'not found'))
                outer.counts[path] = outer.counts.get(path, 0) + 1
                self.send_response(200 if path in payloads else 404)
                self.send_header('Content-Type', kind)
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            def log_message(self, *args):
                pass
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.origin = 'http://127.0.0.1:' + str(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.closed = False

    def close(self):
        if not self.closed:
            self.server.shutdown()
            self.server.server_close()
            self.thread.join(timeout=3)
            require(not self.thread.is_alive(), 'fixture_stop_failed')
            self.closed = True


def windows_app_window_handles(pid, *, enum_top=None, enum_children=None,
                               pid_of=None, visible=None):
    """All VISIBLE windows of the exact app PID: top-level AND child windows.

    Enumerate native HWNDs independently of UIA's desktop window projection.
    Tauri parent() sets an OWNER on Windows, not WS_CHILD (tauri window.rs).
    Include descendants too; the exact menu item and app identity are still
    checked before one physical click. Foreign PIDs are never included.
    """
    if enum_top is None:
        import win32gui
        enum_top = lambda collect: win32gui.EnumWindows(lambda h, _: collect(h), None)
    if enum_children is None:
        import win32gui
        enum_children = lambda parent, collect: win32gui.EnumChildWindows(
            parent, lambda h, _: collect(h), None)
    if pid_of is None:
        def pid_of(hwnd):
            import win32process
            return win32process.GetWindowThreadProcessId(hwnd)[1]
    if visible is None:
        import win32gui
        visible = win32gui.IsWindowVisible
    owned = set()
    def consider(hwnd):
        try:
            if pid_of(hwnd) == pid and visible(hwnd):
                owned.add(int(hwnd))
        except Exception:
            pass
    tops = []
    enum_top(tops.append)
    for hwnd in tops:
        consider(hwnd)
        try:
            enum_children(hwnd, consider)
        except Exception:
            pass
    return owned



def archive_facts(path, png):
    require(path.is_file() and not path.is_symlink(), 'archive_not_regular')
    require(0 < path.stat().st_size < 1024 * 1024, 'archive_size_invalid')
    raw = path.read_bytes()
    message = BytesParser(policy=policy.default).parsebytes(raw)
    require(message.get_content_type() == 'multipart/related' and message.is_multipart(),
            'archive_not_mhtml')
    parts = [(p.get_content_type(), p.get_payload(decode=True) or b'')
             for p in message.walk() if not p.is_multipart()]
    require(any(t == 'text/html' and b'OFFLINE_FRAME_MARKER' in b for t, b in parts),
            'archive_frame_missing')
    require(any(t == 'text/css' and re.search(rb'rgb\(\s*44\s*,\s*190\s*,\s*97\s*\)', b) for t, b in parts),
            'archive_stylesheet_missing')
    require(any(t == 'image/png' and b == png for t, b in parts),
            'archive_image_missing')
    return {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
            'embedded_image': True, 'embedded_stylesheet': True, 'embedded_frame': True}


def paint_matches(client, device):
    from PIL import ImageGrab
    pole = client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height}})()')
    require(pole['w'] > 150 and pole['h'] > 350, 'owned_viewport_missing')
    label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    origin = call(client, 'plugin:window|inner_position', label=label)
    scale = client.js('devicePixelRatio||1')
    for y, color in zip((50, 150, 250), COLORS):
        x = round(origin['x'] + (pole['x'] + 50) * scale)
        sy = round(origin['y'] + (pole['y'] + y) * scale)
        with ImageGrab.grab(bbox=(x, sy, x + 1, sy + 1), **({'xdisplay':device.display} if sys.platform=='linux' else {})) as image:
            pixel = image.convert('RGB').getpixel((0, 0))
        if any(abs(a-b) > 12 for a, b in zip(pixel, color)):
            return False
    return True


def focus_page(client,device,identifier):
    if sys.platform=='linux':
        from x11_owned_cef_focus import focus_selected_cef
        receipt=focus_selected_cef(client,device,identifier)
        pole=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return {x:r.x,y:r.y}})()')
        # A normal user click gives WebContents input focus in addition to
        # X11's focus on the owned outer CEF window. Fixture tile has no action.
        client.mouse(pole['x']+50,pole['y']+50)
        return receipt
    import ctypes
    from ctypes import wintypes as w
    from youtube_paired_native import native_members
    from youtube_player_native import native_app_window
    identity=native_app_identity(device)
    owner=native_app_window(device,identity)
    members=native_members(client,identifier)
    foreground(device)
    pole=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height}})()')
    require(pole['w']>100 and pole['h']>100,'page_focus_bounds_invalid')
    client.mouse(pole['x']+50,pole['y']+50)
    class Info(ctypes.Structure):
        _fields_=[('cbSize',w.DWORD),('flags',w.DWORD),('active',w.HWND),('focus',w.HWND),('capture',w.HWND),('menu',w.HWND),('move',w.HWND),('caret',w.HWND),('rect',w.RECT)]
    user=ctypes.WinDLL('user32',use_last_error=True)
    user.GetGUIThreadInfo.argtypes=[w.DWORD,ctypes.POINTER(Info)];user.GetGUIThreadInfo.restype=w.BOOL
    user.GetWindowThreadProcessId.argtypes=[w.HWND,ctypes.POINTER(w.DWORD)];user.GetWindowThreadProcessId.restype=w.DWORD
    user.GetAncestor.argtypes=[w.HWND,w.UINT];user.GetAncestor.restype=w.HWND
    info=Info();info.cbSize=ctypes.sizeof(info)
    require(user.GetGUIThreadInfo(0,ctypes.byref(info)) and info.focus,'page_focus_unavailable')
    pid=w.DWORD();user.GetWindowThreadProcessId(info.focus,ctypes.byref(pid))
    require(pid.value in {m['pid'] for m in members} and int(user.GetAncestor(info.focus,2) or 0)==int(owner.handle)
            and native_app_identity_alive(device,identity),'page_focus_not_owned_cef')


def run(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {'completed': False, 'stage': 'preflight', 'checks': {}, 'native_execution': False}
    fixture = device = wm = client = identity = None
    started = time.monotonic()
    create_device = lambda root: (WindowsDevice(root,Path(args.exe).resolve(),Path(args.cef).resolve()) if sys.platform=='win32' else Device('page-save', root, Path(args.exe).resolve(), Path(args.cef).resolve()))
    create_wm = OwnedWindowManager
    normal_exit = close_for_privacy
    try:
        require(sys.platform in ('linux','win32'), 'native_fixture_platform_unavailable')
        if args.private_root:
            from youtube_embed_home_probe import validate_private_root, validate_inside_root, LocalWindowManager, local_close
            from youtube_paired_home import BenchmarkDevice
            root = validate_private_root(args.private_root, args.expect_uid)
            validate_inside_root(out, root, 'out')
            exe = validate_inside_root(args.exe, root, 'exe')
            cef = validate_inside_root(args.cef, root, 'cef')
            require(hashlib.sha256(exe.read_bytes()).hexdigest() == args.exe_sha256, 'binary_mismatch')
            create_device = lambda target: BenchmarkDevice(target, exe, cef)
            create_wm, normal_exit = LocalWindowManager, local_close
        else:
            require(os.environ.get('GITHUB_ACTIONS') == 'true', 'gha_or_private_home_required')
        fixture = Fixture()
        report['native_execution'] = True
        identifier = 'page-save-test'

        def launch(name, url):
            nonlocal device,wm,client,identity
            dev = create_device(out / name)
            device = dev
            setup_url='about:blank' if args.diagnostic_native_file_navigation and url.startswith('file:') else url
            item = super_record(identifier, 'Save fixture', setup_url, proxy='')
            folder = dev.data / 'profili' / identifier
            folder.mkdir(parents=True)
            (folder/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':setup_url,'zagolovok':'Save fixture','otlozhena':True}], 'aktivnaya':1,'sleduyushchaya':2}))
            for file in ('vkladki.json', 'sohranennye.json'):
                (dev.data/file).write_text(json.dumps([item]))
            (dev.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False,'memory_saver':False}))
            cli = dev.start()
            client = cli
            cli.js('window.__saveEvents=[];window.__saveListenerReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{const d=e.payload?.data;if(d?.tip==="page-save-state"){const s=d.state||{};window.__saveEvents.push({done:s.done===true,cancelled:s.cancelled===true,error:typeof s.error==="string"?s.error:null});if(window.__saveEvents.length>32)window.__saveEvents.shift()}});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__saveListenerReady=true);true')
            wait_for(lambda:cli.js('window.__saveListenerReady') is True,'save_listener_missing',5)
            manager = create_wm(dev, out/(name+'-wm')) if sys.platform=='linux' else None
            wm = manager
            dev.player_wm = manager
            identity = native_app_identity(dev)
            switch_super(cli, identifier)
            tab = rows(cli, identifier)['aktivnaya']
            call(cli,'obychnaya_komanda',id=identifier,komanda={'deystvie':'vybrat','id':tab})
            wait_for(lambda:any(t.get('browser_id',0)>0 and not t.get('zagruzka') for t in rows(cli,identifier)['vkladki']), 'page_not_loaded', 45)
            if setup_url!=url:
                call(cli,'obychnaya_komanda',id=identifier,komanda={'deystvie':'pereyti','id':tab,'adres':url})
                wait_for(lambda:any(t.get('browser_id',0)>0 and not t.get('zagruzka') and t.get('adres')==url for t in rows(cli,identifier)['vkladki']), 'native_file_navigation_not_loaded',30)
            foreground(dev)
            wait_for(lambda:paint_matches(cli,dev), 'page_not_painted',20)
            return dev, manager, cli

        report['stage'] = 'online-page'
        device, wm, client = launch('online',fixture.origin+'/page.html')
        identity = native_app_identity(device)
        require(all(fixture.counts.get(p,0)>0 for p in ('/page.html','/style.css','/image.png','/frame.html')), 'resources_not_loaded')
        before = dialogs(device)
        report['stage'] = 'cancel-native-ctrl-s'
        focus_page(client,device,identifier)
        keys(device,'ctrl+s')
        dialog = wait_for(lambda:next(iter(dialogs(device)-before),None),'save_dialog_missing',15)
        require(native_app_identity_alive(device,identity),'owned_app_changed')
        choose(device,dialog)
        wait_for(lambda:dialog not in dialogs(device),'cancel_did_not_close',10)
        report['checks']['native_ctrl_s_dialog_cancel'] = True
        report['stage'] = 'save-native-ctrl-s'
        focus_page(client,device,identifier)
        before = dialogs(device)
        keys(device,'ctrl+s')
        dialog = wait_for(lambda:next(iter(dialogs(device)-before),None),'save_dialog_missing',15)
        target = out/'my-page.mhtml'
        choose(device,dialog,target)
        wait_for(lambda:target.is_file(),'archive_not_written',60)
        report['archive'] = archive_facts(target,fixture.png)
        report['checks']['native_ctrl_s_saved_mhtml'] = True
        report['stage'] = 'menu-save-overwrite'
        before = dialogs(device)
        old_stamp = target.stat().st_mtime_ns
        def visible_app_windows():
            if sys.platform=='win32':
                return windows_app_window_handles(identity['pid'])
            result=subprocess.run(['xdotool','search','--onlyvisible','--pid',str(identity['pid'])],env={**os.environ,'DISPLAY':device.display},capture_output=True,text=True,timeout=3)
            return {int(n) for n in result.stdout.split() if n.isdecimal()}
        previous_app_windows=visible_app_windows()
        click_control(client,'button[aria-label="Меню NexusBro"]',10)
        wait_for(lambda:len(visible_app_windows()-previous_app_windows)>0,'owned_menu_window_missing',10)
        if sys.platform=='win32':
            from pywinauto import Desktop
            def save_item():
                if not native_app_identity_alive(device,identity):return None
                found=[]
                for handle in visible_app_windows()-previous_app_windows:
                    window=Desktop(backend='uia').window(handle=handle)
                    for item in window.descendants(control_type='MenuItem'):
                        if item.window_text()=='Сохранить страницу…' and item.is_visible() and item.is_enabled():found.append(item)
                return found[0] if len(found)==1 else None
            item=wait_for(save_item,'owned_save_menu_item_missing',10)
            require(native_app_identity_alive(device,identity),'owned_app_changed')
            item.click_input()
        else:
            # Reuse one inspector connection at a time. The popup is a
            # separate trusted shell WebPage, never the CEF site. Read its
            # actual menuitem bounds, then send ONE physical OS click.
            from client import Client
            client.close()
            popup=None
            for index in range(2):
                candidate=Client(display=device.display,port=device.inspector,page_index=index)
                label=candidate.js('window.__TAURI_INTERNALS__?.metadata.currentWindow.label')
                if isinstance(label,str) and label.startswith('menu-'):
                    popup=candidate;break
                candidate.close()
            require(popup is not None,'owned_popup_page_missing')
            require(native_app_identity_alive(device,identity),'owned_app_changed')
            bounds=wait_for(lambda:popup.js("""(()=>{const e=document.querySelector('button[role="menuitem"][aria-label="Сохранить страницу…"]');if(!e)return null;const r=e.getBoundingClientRect(),x=r.x+r.width/2,y=r.y+r.height/2,h=document.elementFromPoint(x,y);return r.width>0&&r.height>0&&(h===e||e.contains(h))?{x,y,w:innerWidth,h:innerHeight,k:devicePixelRatio||1}:null})()"""),'save_menu_item_not_painted',10)
            from x11_owned_cef_focus import NativeX11
            native=NativeX11(device.display)
            try:
                candidates=[]
                for window in visible_app_windows()-previous_app_windows:
                    geometry=native.geometry(window)
                    if native.pid_of(window)==identity['pid'] and geometry['viewable'] and abs(geometry['w']-bounds['w']*bounds['k'])<=2 and abs(geometry['h']-bounds['h']*bounds['k'])<=2:
                        candidates.append(geometry)
                require(len(candidates)==1,'popup_native_geometry_unproven')
                geometry=candidates[0]
                point=(round(geometry['x']+bounds['x']*bounds['k']),round(geometry['y']+bounds['y']*bounds['k']))
            finally:native.close()
            require(native_app_identity_alive(device,identity),'owned_app_changed')
            subprocess.run(['xdotool','mousemove',str(point[0]),str(point[1]),'click','1'],env={**os.environ,'DISPLAY':device.display},check=True,timeout=5)
            try:popup.close()
            except Exception:pass
            client=Client(display=device.display,port=device.inspector,page_index=0)
            device.client=client
            require(client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')=='main','main_page_reconnect_failed')
        dialog = wait_for(lambda:next(iter(dialogs(device)-before),None),'menu_save_dialog_missing',15)
        choose(device,dialog,target,replace=True)
        wait_for(lambda:dialog not in dialogs(device),'menu_save_dialog_still_open',15)
        # The old file cannot prove the second save: require a new published
        # file timestamp after the explicit native replacement confirmation.
        wait_for(lambda:target.stat().st_mtime_ns!=old_stamp,'menu_save_done_missing',60)
        report['menu_archive'] = archive_facts(target,fixture.png)
        report['checks']['menu_save_overwrite'] = True
        exit_receipt = {}
        normal_exit(client,device,exit_receipt)
        report['online_exit'] = exit_receipt
        device.close()
        if wm:wm.close()
        device = wm = client = None
        fixture.close()
        report['checks']['server_stopped_before_reopen'] = True
        report['stage'] = 'offline-fresh-profile'
        device, wm, client = launch('offline',target.as_uri())
        report['checks']['offline_fresh_profile_css_image_and_frame_painted'] = True
        receipt = {}
        normal_exit(client,device,receipt)
        report['offline_exit'] = receipt
        report['diagnostic_only'] = args.diagnostic_native_file_navigation
        report['diagnostic_execution_complete'] = args.diagnostic_native_file_navigation
        report['completed'] = not args.diagnostic_native_file_navigation
        report['stage'] = 'complete'
    except Exception as error:
        # The fixtures themselves contain no owner data, but don't export raw
        # exceptions: UI/file libraries may include paths in error messages.
        report['failure_type'] = type(error).__name__
        if re.fullmatch(r'[a-z][a-z0-9_]{2,80}',str(error)):
            report['failure_code'] = str(error)
        report['blocked_reason'] = report['stage'] + '_failed'
        if client is not None:
            try:report['save_events']=client.js('window.__saveEvents||[]')
            except Exception:pass
        if device is not None and identity is not None:
            from download_startup_facts import owned_failure_screenshot
            check=lambda:native_app_identity_alive(device,identity)
            check.identity=identity
            report['owned_failure_capture']=owned_failure_screenshot(device,out,check)
    finally:
        clean = True
        for obj in (device,wm,fixture):
            if obj is not None:
                try:obj.close()
                except Exception:clean = False
        report['cleanup_completed'] = clean
        report['completed'] = report['completed'] and clean
        report['wall_seconds'] = round(time.monotonic()-started,3)
        (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({k:report[k] for k in ('completed','stage','cleanup_completed')}))
    return 0 if report['completed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('exe','cef','out'):
        parser.add_argument('--'+key,required=True)
    parser.add_argument('--diagnostic-native-file-navigation',action='store_true')
    parser.add_argument('--private-root')
    parser.add_argument('--expect-uid',type=int)
    parser.add_argument('--exe-sha256')
    raise SystemExit(run(parser.parse_args()))
