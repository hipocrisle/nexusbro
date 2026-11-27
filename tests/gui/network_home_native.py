#!/usr/bin/env python3
"""Home-native acceptance for the unified tab Network dialog (Direct vs
unreachable inherit route). Real physical UI only: toolbar button, dialog,
"Проверить IP" click, dialog close, tab switch. The invoke wrapper is a
PASSIVE observer of tab_route_check_ip (never rewrites args/results). No page
JS actions, no fake invoke, no owner desktop/profile/cookies. Private IP,
runtime paths and raw DOM/API bodies are never printed or persisted: the
receipt carries booleans, fixed keys, counts and typed codes only. This
fixture never claims a GHA shutdown receipt; the local owned exit is recorded
as local facts. Root reviews and runs it on Home with the new binary."""
import argparse
import json
import os
import re
import socket
import stat
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from security_native import require, wait_for
from youtube_paired_native import super_record, switch_super
from tab_routes import click_control
from youtube_player_native import native_app_identity, native_app_identity_alive
from youtube_embed_home_probe import (SandboxedDevice, LocalWindowManager,
                                      local_close, validate_private_root,
                                      validate_inside_root)

WALL_BUDGET_SECONDS = 120
SUPER_ID = 'network-test'


class Page(BaseHTTPRequestHandler):
    hits = 0

    def do_GET(self):
        Page.hits += 1
        body = b'<!doctype html><title>Network fixture</title><p>own loopback</p>'
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def free_closed_port():
    sock = socket.socket()
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


OBSERVER_JS = r"""
window.__netEvents=[];window.__netReady=false;
const orig=window.__TAURI_INTERNALS__.invoke.bind(window.__TAURI_INTERNALS__);
window.__TAURI_INTERNALS__.invoke=function(cmd,args){
 const p=orig(cmd,args);
 if(cmd==='tab_route_check_ip'){
  const rec={tabId:(args&&typeof args.tabId==='number')?args.tabId:null,
   hasId:typeof (args&&args.id)==='string'&&(args.id||'').length>0,
   hasGen:typeof (args&&args.expectedGeneration)==='number'};
  const push=()=>{window.__netEvents.push(rec);if(window.__netEvents.length>16)window.__netEvents.shift();};
  p.then(v=>{rec.ok=true;
   rec.ipValid=typeof v?.ip==='string'&&v.ip.length>0&&v.ip.length<=45;
   rec.countryOk=typeof v?.country==='string'&&/^[A-Z]{2}$/.test(v.country);
   rec.countryNull=v?.country===null;
   rec.msNum=typeof v?.ms==='number';push();},()=>{rec.ok=false;push();});
 }
 return p;
};
window.__netReady=true;true
"""


def main():
    parser = argparse.ArgumentParser()
    for key in ('exe', 'cef', 'out'):
        parser.add_argument('--' + key, required=True)
    parser.add_argument('--private-root', required=True)
    parser.add_argument('--expect-uid', type=int, required=True)
    parser.add_argument('--exe-sha256', required=True)
    args = parser.parse_args()

    report = {'completed': False, 'checks': {}, 'stage': 'guards'}
    started = time.monotonic()
    device = wm = client = identity = None
    server = None
    out = Path(args.out).resolve()
    try:
        require(os.environ.get('GITHUB_ACTIONS') != 'true', 'gha_environment_refused')
        require(os.geteuid() == args.expect_uid, 'unexpected_uid')
        exe = Path(args.exe)
        require(exe.is_absolute() and exe.is_file() and not exe.is_symlink(), 'exe_invalid')
        import hashlib
        require(hashlib.sha256(exe.read_bytes()).hexdigest() == args.exe_sha256,
                'exe_sha256_mismatch')
        root = validate_private_root(args.private_root, args.expect_uid)
        cef = validate_inside_root(Path(args.cef).resolve(), root, 'cef')
        require(stat.S_IMODE(root.stat().st_mode) == 0o700, 'private_root_mode')
        out.mkdir(parents=True, exist_ok=True)
        validate_inside_root(out, root, 'out')

        server = ThreadingHTTPServer(('127.0.0.1', 0), Page)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        page_url = f'http://127.0.0.1:{server.server_address[1]}/page'
        dead_port = free_closed_port()

        report['stage'] = 'seed'
        device = SandboxedDevice('network', root / 'device', exe, cef)
        data = device.data
        (data / 'profili' / SUPER_ID).mkdir(parents=True)
        (data / 'profili' / SUPER_ID / 'obychnye.json').write_text(json.dumps({
            'vkladki': [
                {'id': 1, 'adres': page_url, 'zagolovok': 'direct', 'otlozhena': True,
                 'route': {'kind': 'direct'}},
                {'id': 2, 'adres': page_url, 'zagolovok': 'inherit', 'otlozhena': True,
                 'route': {'kind': 'inherit'}},
            ], 'aktivnaya': 1, 'sleduyushchaya': 3}))
        item = super_record(SUPER_ID, 'Network fixture', page_url,
                            proxy=f'127.0.0.1:{dead_port}')
        for name in ('vkladki.json', 'sohranennye.json'):
            (data / name).write_text(json.dumps([item]))
        (data / 'nastrojki.json').write_text(json.dumps(
            {'avtoobnovleniya': False, 'apparatnoe_uskorenie': False,
             'memory_saver': False}))

        report['stage'] = 'launch'
        client = device.start()
        wm = LocalWindowManager(device, root / 'wm') if sys.platform == 'linux' else None
        if wm is not None:
            wm.__enter__()
            device.player_wm = wm
        identity = native_app_identity(device)
        switch_super(client, SUPER_ID)
        wait_for(lambda: Page.hits > 0, 'direct_tab_not_loaded', 30)
        report['checks']['tab1_direct_loaded_in_cef'] = True
        client.js(OBSERVER_JS)
        wait_for(lambda: client.js('window.__netReady') is True, 'observer_missing', 5)

        def dialog_state():
            return client.js(
                "(()=>{const d=document.querySelector('.tab-route-dialog');"
                "if(!d)return null;const s=d.querySelector('.tab-route-exit span');"
                "const t=s?s.textContent:'';"
                "return {open:true,checking:t.includes('проверяется'),"
                "error:t.includes('ошибка проверки'),none:t.includes('не проверен'),"
                "shown:!t.includes('проверяется')&&!t.includes('ошибка проверки')"
                "&&!t.includes('не проверен')&&t.length>0};})()")

        def events():
            value = client.js('window.__netEvents||[]')
            return value if isinstance(value, list) else []

        def open_dialog_and_check(tab_id):
            click_control(client, 'button[aria-label="Сеть вкладки"]', 10)
            wait_for(lambda: (dialog_state() or {}).get('open') is True,
                     'network_dialog_missing', 10)
            click_control(client, '.tab-route-exit button', 10)
            record = wait_for(
                lambda: next((e for e in reversed(events())
                              if e.get('tabId') == tab_id), None),
                'check_ip_not_observed', 30)
            require(record.get('hasId') is True and record.get('hasGen') is True,
                    'check_ip_args_unproven')
            wait_for(lambda: (dialog_state() or {}).get('checking') is not True,
                     'check_ip_ui_busy', 30)
            return record, dialog_state()

        report['stage'] = 'tab1-direct-check'
        record, ui = open_dialog_and_check(1)
        require(record.get('ok') is True, 'direct_check_failed')
        require(record.get('ipValid') is True, 'direct_ip_unproven')
        require(record.get('msNum') is True, 'direct_ms_unproven')
        require(ui.get('shown') is True, 'direct_ui_not_shown')
        report['checks']['tab1_direct_real_ip'] = True
        report['checks']['tab1_ui_shows_exit'] = True
        if record.get('countryOk') is True:
            report['checks']['country_from_cf_trace'] = True
            report['country_proof'] = 'cf_trace_loc'
        else:
            require(record.get('countryNull') is True, 'country_shape_unknown')
            report['country_proof'] = 'unknown_ipify_fallback'
        click_control(client, 'button[aria-label="Закрыть выбор пути вкладки"]', 10)
        wait_for(lambda: dialog_state() is None, 'dialog_not_closed', 10)
        report['checks']['dialog_closed_physically'] = True

        report['stage'] = 'tab2-inherit-check'
        click_control(client, '.obychnaya[data-id="2"]', 10)
        wait_for(lambda: client.js(
            "document.querySelector('.obychnaya.aktivnaya')?.dataset.id||null") == '2',
            'tab2_not_active', 10)
        before = len(events())
        record, ui = open_dialog_and_check(2)
        require(record.get('ok') is False, 'inherit_check_unexpectedly_succeeded')
        require(ui.get('error') is True and ui.get('shown') is not True,
                'inherit_ui_not_error')
        require(all(e.get('tabId') != 2 or e.get('ok') is False
                    for e in events()[before:]),
                'inherit_produced_success_record')
        report['checks']['tab2_inherit_real_error'] = True
        report['checks']['no_direct_fallback_for_inherit'] = True
        report['stage'] = 'complete'
        report['completed'] = True
    except Exception as error:
        report['blocked_reason'] = report['stage'] + '_failed'
        report['exception_type'] = type(error).__name__
        code = str(error)
        if re.fullmatch(r'[a-z0-9_]{3,64}', code):
            report['typed_code'] = code
    finally:
        report['wall_seconds'] = round(time.monotonic() - started, 3)
        if time.monotonic() - started > WALL_BUDGET_SECONDS:
            report['wall_budget_exceeded'] = True
        try:
            if server is not None:
                server.shutdown()
        except Exception:
            pass
        clean = True
        if client is not None and device is not None:
            try:
                local_close(client, device, report)
            except Exception as error:
                report['local_exit_error'] = type(error).__name__
                clean = False
        if wm is not None:
            try:
                wm.__exit__(None, None, None)
            except Exception:
                clean = False
        if device is not None:
            try:
                device.close()
            except Exception:
                clean = False
        report['cleanup_completed'] = clean
        out_path = Path(args.out).resolve() / 'result.json'
        out_path.write_text(json.dumps(report, ensure_ascii=False, indent=1) + '\n')
        print(json.dumps({'completed': report['completed'],
                          'stage': report['stage'],
                          'cleanup_completed': report['cleanup_completed']}))
    return 0 if report['completed'] else 1


if __name__ == '__main__':
    sys.exit(main())
