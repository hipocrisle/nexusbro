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
from youtube_paired_native import (super_record, switch_super, rows,
    focus_normal_cef, click_control)
from youtube_player_native import (OwnedWindowManager, foreground,
    close_for_privacy, native_app_identity, native_app_identity_alive)
from youtube_download_native import dialogs, choose, keys
from download_fixture_target import fixture_target

COLORS = ((44, 190, 97), (41, 93, 219))


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
                    '/style.css': ('text/css', b'body{margin:0;background:#fff}#tile{width:100px;height:100px;background:rgb(44,190,97)}img{display:block}'),
                    '/image.png': ('image/png', outer.png),
                    '/frame.html': ('text/html', b'<!doctype html><title>Embedded fixture</title><p>OFFLINE_FRAME_MARKER</p>'),
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
    require(any(t == 'text/css' and b'rgb(44,190,97)' in b for t, b in parts),
            'archive_stylesheet_missing')
    require(any(t == 'image/png' and b == png for t, b in parts),
            'archive_image_missing')
    return {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
            'embedded_image': True, 'embedded_stylesheet': True, 'embedded_frame': True}


def paint_matches(client, device):
    from PIL import ImageGrab
    pole = client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height}})()')
    require(pole['w'] > 150 and pole['h'] > 250, 'owned_viewport_missing')
    label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    origin = call(client, 'plugin:window|inner_position', label=label)
    scale = client.js('devicePixelRatio||1')
    for y, color in zip((50, 150), COLORS):
        x = round(origin['x'] + (pole['x'] + 50) * scale)
        sy = round(origin['y'] + (pole['y'] + y) * scale)
        with ImageGrab.grab(bbox=(x, sy, x + 1, sy + 1), xdisplay=device.display) as image:
            pixel = image.convert('RGB').getpixel((0, 0))
        if any(abs(a-b) > 12 for a, b in zip(pixel, color)):
            return False
    return True


def run(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {'completed': False, 'stage': 'preflight', 'checks': {}, 'native_execution': False}
    fixture = device = wm = client = None
    started = time.monotonic()
    create_device = lambda root: Device('page-save', root, Path(args.exe).resolve(), Path(args.cef).resolve())
    create_wm = OwnedWindowManager
    normal_exit = close_for_privacy
    try:
        require(sys.platform == 'linux', 'linux_fixture_only')
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
            nonlocal device,wm,client
            dev = create_device(out / name)
            device = dev
            item = super_record(identifier, 'Save fixture', url, proxy='')
            folder = dev.data / 'profili' / identifier
            folder.mkdir(parents=True)
            (folder/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':url,'zagolovok':'Save fixture','otlozhena':True}], 'aktivnaya':1,'sleduyushchaya':2}))
            for file in ('vkladki.json', 'sohranennye.json'):
                (dev.data/file).write_text(json.dumps([item]))
            (dev.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False,'memory_saver':False}))
            cli = dev.start()
            client = cli
            manager = create_wm(dev, out/(name+'-wm'))
            wm = manager
            dev.player_wm = manager
            switch_super(cli, identifier)
            tab = rows(cli, identifier)['aktivnaya']
            call(cli,'obychnaya_komanda',id=identifier,komanda={'deystvie':'vybrat','id':tab})
            wait_for(lambda:any(t.get('browser_id',0)>0 and not t.get('zagruzka') for t in rows(cli,identifier)['vkladki']), 'page_not_loaded', 45)
            foreground(dev)
            wait_for(lambda:paint_matches(cli,dev), 'page_not_painted',20)
            return dev, manager, cli

        report['stage'] = 'online-page'
        device, wm, client = launch('online',fixture.origin+'/page.html')
        identity = native_app_identity(device)
        require(all(fixture.counts.get(p,0)>0 for p in ('/page.html','/style.css','/image.png','/frame.html')), 'resources_not_loaded')
        before = dialogs(device)
        report['stage'] = 'cancel-native-ctrl-s'
        focus_normal_cef(client,device)
        keys(device,'ctrl+s')
        dialog = wait_for(lambda:next(iter(dialogs(device)-before),None),'save_dialog_missing',15)
        require(native_app_identity_alive(device,identity),'owned_app_changed')
        choose(device,dialog)
        wait_for(lambda:dialog not in dialogs(device),'cancel_did_not_close',10)
        report['checks']['native_ctrl_s_dialog_cancel'] = True
        report['stage'] = 'save-native-ctrl-s'
        focus_normal_cef(client,device)
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
        click_control(client,'button[aria-label="Меню NexusBro"]',10)
        def own_menu_focus():
            env = {**os.environ,'DISPLAY':device.display}
            focus = subprocess.check_output(['xdotool','getwindowfocus'],env=env,text=True,timeout=3).strip()
            title = subprocess.check_output(['xdotool','getwindowname',focus],env=env,text=True,timeout=3).strip()
            pid = subprocess.check_output(['xdotool','getwindowpid',focus],env=env,text=True,timeout=3).strip()
            return (title=='NexusBro — меню' and pid==str(identity['pid'])
                    and native_app_identity_alive(device,identity))
        wait_for(own_menu_focus,'owned_menu_focus_missing',10)
        # PopupMenu's Home selects the first enabled item; Save is second
        # on this ordinary HTTP fixture (there is no YouTube-only item).
        keys(device,'Home','Down','Return')
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
        device.close(); wm.close(); device = wm = client = None
        fixture.close()
        report['checks']['server_stopped_before_reopen'] = True
        report['stage'] = 'offline-fresh-profile'
        device, wm, client = launch('offline',target.as_uri())
        report['checks']['offline_fresh_profile_css_and_image_painted'] = True
        receipt = {}
        normal_exit(client,device,receipt)
        report['offline_exit'] = receipt
        report['completed'] = True
        report['stage'] = 'complete'
    except Exception as error:
        # The fixtures themselves contain no owner data, but don't export raw
        # exceptions: UI/file libraries may include paths in error messages.
        report['failure_type'] = type(error).__name__
        report['blocked_reason'] = report['stage'] + '_failed'
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
    parser.add_argument('--private-root')
    parser.add_argument('--expect-uid',type=int)
    parser.add_argument('--exe-sha256')
    raise SystemExit(run(parser.parse_args()))
