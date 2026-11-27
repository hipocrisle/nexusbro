#!/usr/bin/env python3
"""Home-native acceptance for the unified tab Network dialog (Direct vs
unreachable inherit route through an OWN counting CONNECT-refusing proxy).

Physical UI is the primary evidence; a SEPARATE readonly
tab_route_check_ip invoke (same owner/tab/generation) independently confirms
the backend answer. Values live solely inside the JS closure and python RAM
while verifying that the dialog shows the SAME ip/country label; only
booleans are recorded, nothing private is logged, persisted or committed.
Nothing is intercepted or wrapped: pinned tauri makes invoke non-writable. No page JS actions, no owner desktop/profile/cookies.
Local owned exit is recorded as local facts, never as a GHA receipt."""
import argparse
import json
import os
import re
import signal
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
CLEANUP_BUDGET_SECONDS = 30
SUPER_ID = 'network-test'
EXIT_UI_KEYS = ('parent_normal_exit', 'all_known_native_executions_exited',
                'native_members_complete', 'launcher_exit_zero')


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


class RefusingProxy(BaseHTTPRequestHandler):
    """Own loopback proxy that counts CONNECT attempts and refuses them: the
    inherit tab has real proxy attempts but no internet through the proxy."""
    connects = 0

    def do_CONNECT(self):
        RefusingProxy.connects += 1
        self.send_response(403)
        self.end_headers()

    def do_GET(self):
        RefusingProxy.connects += 1
        self.send_response(403)
        self.end_headers()

    def log_message(self, *args):
        pass


# Readonly UI/confirm probes. The app's own physical Check-IP click produces
# the dialog text; a SEPARATE readonly tab_route_check_ip invoke (same
# owner/tab/generation, exactly what the UI itself calls) independently
# confirms the backend answer. Values live only inside the JS closure and
# python RAM; only booleans are ever recorded. Nothing is intercepted,
# wrapped or replaced (pinned tauri core.js makes invoke non-writable).
DOM_PROBE_JS = r"""
(()=>{const s=document.querySelector('.tab-route-dialog .tab-route-exit span');
if(!s)return null;
const t=s.textContent||'';
const m=t.match(/(\d{1,3}(?:\.\d{1,3}){3}|[0-9a-fA-F:]{2,45})/);
const validIp=x=>{if(!x)return false;const v4=x.split('.');
 if(v4.length===4&&v4.every(p=>/^\d{1,3}$/.test(p)&&+p<=255))return true;
 try{new URL('http://['+x+']/');return x.includes(':');}catch(e){return false;}};
return {open:true,hasText:t.length>0,
 checking:t.includes('проверяется'),error:t.includes('ошибка проверки'),
 none:t.includes('не проверен'),domIpValid:validIp(m&&m[0])};})()
"""

CONFIRM_JS = r"""
window.__netConfirm=null;
(async()=>{
 const out={done:false};
 try{
  const sess=await window.__TAURI_INTERNALS__.invoke('obychnye_spisok',{id:'network-test'});
  const tab=(sess&&sess.vkladki||[]).find(t=>t.id===__TAB_ID__);
  if(!tab||typeof tab.route_generation!=='number'){out.genOk=false;out.done=true;window.__netConfirm=out;return;}
  out.genOk=true;
  try{
   const v=await window.__TAURI_INTERNALS__.invoke('tab_route_check_ip',
    {id:'network-test',tabId:__TAB_ID__,expectedGeneration:tab.route_generation});
   const s=document.querySelector('.tab-route-dialog .tab-route-exit span');
   const text=s?s.textContent:'';
   const validIp=x=>{if(typeof x!=='string'||!x.length||x.length>45)return false;
    const v4=x.split('.');
    if(v4.length===4&&v4.every(p=>/^\d{1,3}$/.test(p)&&+p<=255))return true;
    try{new URL('http://['+x+']/');return x.includes(':');}catch(e){return false;}};
   out.ok=true;
   out.ipValid=validIp(v&&v.ip);
   out.uiSameIp=out.ipValid&&text.includes(v.ip);
   out.countryNull=v&&v.country===null;
   out.countryOk=typeof (v&&v.country)==='string'&&/^[A-Z]{2}$/.test(v.country);
   if(out.countryOk){
    let label=null;
    try{label=new Intl.DisplayNames(['ru'],{type:'region'}).of(v.country)||v.country;}catch(e){label=v.country;}
    out.uiSameCountry=text.includes(label)||text.includes(v.country);
   } else { out.uiSameCountry=true; }
   out.msNum=typeof (v&&v.ms)==='number';
  }catch(e){out.ok=false;}
 }catch(e){out.outerError=true;}
 out.done=true;window.__netConfirm=out;
})();
true
"""


def main():
    parser = argparse.ArgumentParser()
    for key in ('exe', 'cef', 'out'):
        parser.add_argument('--' + key, required=True)
    parser.add_argument('--private-root', required=True)
    parser.add_argument('--expect-uid', type=int, required=True)
    parser.add_argument('--exe-sha256', required=True)
    parser.add_argument('--allow-local-virtual-display', action='store_true')
    args = parser.parse_args()

    report = {'completed': False, 'checks': {}, 'stage': 'guards'}
    guards_ok = False
    out = None
    started = time.monotonic()
    work_complete=False
    device = wm = client = identity = None
    servers = []
    exit_receipt = {}

    def alarm_handler(signum, frame):
        raise TimeoutError('wall_budget')

    try:
        require(args.allow_local_virtual_display is True, 'virtual_display_not_allowed')
        require(os.environ.get('GITHUB_ACTIONS') != 'true', 'gha_environment_refused')
        require('WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS' not in os.environ,
                'inherited_sandbox_disable_refused')
        require(os.geteuid() == args.expect_uid and args.expect_uid != 0,
                'unexpected_uid')
        exe_raw = Path(args.exe)
        require(exe_raw.is_absolute() and not exe_raw.is_symlink() and exe_raw.is_file(),
                'exe_invalid')
        import hashlib
        require(hashlib.sha256(exe_raw.read_bytes()).hexdigest() == args.exe_sha256,
                'exe_sha256_mismatch')
        root = validate_private_root(args.private_root, args.expect_uid)
        cef = validate_inside_root(Path(args.cef), root, 'cef')
        require(cef.is_dir() and not Path(args.cef).is_symlink(), 'cef_invalid')
        require(stat.S_IMODE(root.stat().st_mode) == 0o700, 'private_root_mode')
        exe_raw = validate_inside_root(exe_raw, root, 'exe')
        requested_out=Path(args.out)
        require(requested_out.is_absolute() and not requested_out.exists() and not requested_out.is_symlink(),'output_must_be_new')
        parent=validate_inside_root(requested_out.parent,root,'out_parent')
        out=parent/requested_out.name
        out.mkdir(mode=0o700, exist_ok=False)
        out=validate_inside_root(out,root,'out')
        report['exe_sha256']=args.exe_sha256
        guards_ok = True
        signal.signal(signal.SIGALRM, alarm_handler)
        signal.alarm(WALL_BUDGET_SECONDS)

        page_server = ThreadingHTTPServer(('127.0.0.1', 0), Page)
        proxy_server = ThreadingHTTPServer(('127.0.0.1', 0), RefusingProxy)
        servers = [page_server, proxy_server]
        for server in servers:
            threading.Thread(target=server.serve_forever, daemon=True).start()
        page_url = f'http://127.0.0.1:{page_server.server_address[1]}/page'
        proxy_url = f'http://127.0.0.1:{proxy_server.server_address[1]}'

        report['stage'] = 'seed'
        device = SandboxedDevice('network', out / 'device', exe_raw, cef)
        data = device.data
        (data / 'profili' / SUPER_ID).mkdir(parents=True)
        (data / 'profili' / SUPER_ID / 'obychnye.json').write_text(json.dumps({
            'vkladki': [
                {'id': 1, 'adres': page_url, 'zagolovok': 'direct',
                 'otlozhena': True, 'route': {'kind': 'direct'}},
                {'id': 2, 'adres': page_url, 'zagolovok': 'inherit',
                 'otlozhena': True, 'route': {'kind': 'inherit'}},
            ], 'aktivnaya': 1, 'sleduyushchaya': 3}))
        item = super_record(SUPER_ID, 'Network fixture', page_url, proxy=proxy_url)
        for name in ('vkladki.json', 'sohranennye.json'):
            (data / name).write_text(json.dumps([item]))
        (data / 'nastrojki.json').write_text(json.dumps(
            {'avtoobnovleniya': False, 'apparatnoe_uskorenie': False,
             'memory_saver': False}))

        report['stage'] = 'launch'
        client = device.start()
        if sys.platform == 'linux':
            wm = LocalWindowManager(device, out / 'wm')
            device.player_wm = wm
        identity = native_app_identity(device)

        def own(action):
            require(native_app_identity_alive(device, identity),
                    'owned_app_changed_before_' + action)
            return True

        own('switch_super')
        switch_super(client, SUPER_ID)
        # Select the lazy tab through its real tab button. Clicking the
        # empty page viewport cannot wake a tab with no WebContents yet.
        own('wake_tab1')
        click_control(client, '.obychnaya[data-id="1"][aria-selected="true"]', 10)
        wait_for(lambda: Page.hits > 0, 'direct_tab_not_loaded', 30)
        report['checks']['tab1_direct_loaded_in_cef'] = True
        def dialog_state():
            value = client.js(DOM_PROBE_JS)
            return value if isinstance(value, dict) else None

        def physical(selector, action, timeout=10):
            own(action)
            click_control(client, selector, timeout)

        def open_dialog_and_check(tab_id):
            physical('button[aria-label="Сеть вкладки"]', 'open_network_dialog')
            wait_for(lambda: (dialog_state() or {}).get('open') is True,
                     'network_dialog_missing', 10)
            physical('.tab-route-exit button', 'check_ip_click')
            wait_for(lambda: (lambda state: state.get('checking') is False and (state.get('error') is True or state.get('domIpValid') is True))(dialog_state() or {}),
                     'check_ip_ui_busy', 30)
            return dialog_state() or {}

        def readonly_confirm(tab_id):
            own('readonly_confirm')
            client.js(CONFIRM_JS.replace('__TAB_ID__', str(int(tab_id))))
            record = wait_for(
                lambda: client.js('window.__netConfirm||null'),
                'readonly_confirm_missing', 30)
            require(isinstance(record, dict) and record.get('done') is True,
                    'readonly_confirm_invalid')
            return record

        report['stage'] = 'tab1-direct-check'
        ui = open_dialog_and_check(1)
        require(ui.get('domIpValid') is True, 'direct_ui_ip_invalid')
        require(ui.get('shown', ui.get('hasText')) is True and ui.get('error') is False
                and ui.get('none') is False, 'direct_ui_not_shown')
        record = readonly_confirm(1)
        require(record.get('genOk') is True, 'direct_generation_unproven')
        require(record.get('ok') is True, 'direct_readonly_confirm_failed')
        require(record.get('ipValid') is True, 'direct_ip_unproven')
        require(record.get('msNum') is True, 'direct_ms_unproven')
        require(record.get('uiSameIp') is True, 'direct_ui_ip_differs_from_answer')
        report['checks']['tab1_direct_real_ip'] = True
        report['checks']['tab1_ui_shows_same_ip'] = True
        report['checks']['separate_readonly_confirmation'] = True
        if record.get('countryOk') is True:
            require(record.get('uiSameCountry') is True,
                    'direct_ui_country_differs_from_answer')
            report['checks']['country_from_cf_trace'] = True
            report['checks']['tab1_ui_shows_same_country'] = True
            report['country_proof'] = 'cf_trace_loc'
        else:
            require(record.get('countryNull') is True, 'country_shape_unknown')
            report['country_proof'] = 'unknown_ipify_fallback'
        physical('button[aria-label="Закрыть сеть вкладки"]', 'close_dialog')
        wait_for(lambda: dialog_state() is None, 'dialog_not_closed', 10)
        report['checks']['dialog_closed_physically'] = True

        report['stage'] = 'tab2-inherit-check'
        physical('.obychnaya[data-id="2"]', 'select_tab2')
        wait_for(lambda: client.js(
            "document.querySelector('.obychnaya[aria-selected=\"true\"]')"
            "?.dataset.id||null") == '2', 'tab2_not_active', 10)
        proxy_before = RefusingProxy.connects
        ui = open_dialog_and_check(2)
        require(ui.get('error') is True and ui.get('domIpValid') is not True,
                'inherit_ui_not_error')
        record = readonly_confirm(2)
        require(record.get('genOk') is True, 'inherit_generation_unproven')
        require(record.get('ok') is False, 'inherit_readonly_confirm_unexpectedly_succeeded')
        report['checks']['tab2_inherit_real_error'] = True
        report['checks']['inherit_readonly_confirm_error'] = True
        report['checks']['no_success_record_for_inherit'] = True
        # Scoped claim: the inherit attempt really went through the OWN proxy
        # (counted CONNECT attempts) and produced a real error in the physical
        # UI. No universal claim about the absence of any direct packets.
        report['checks']['inherit_attempted_own_proxy'] = RefusingProxy.connects > proxy_before
        report['stage'] = 'complete'
        work_complete=True
    except Exception as error:
        report['blocked_reason'] = report['stage'] + '_failed'
        report['exception_type'] = type(error).__name__
        if client is not None:
            try:
                # Readonly DOM facts at the failure point; booleans only.
                report['dialog_at_failure']=dialog_state()
            except Exception:
                report['failure_observation_unavailable']=True
        code = str(error)
        if re.fullmatch(r'[a-z0-9_]{3,64}', code):
            report['typed_code'] = code
    finally:
        signal.alarm(0)
        report['wall_seconds'] = round(time.monotonic() - started, 3)
        if report['wall_seconds'] > WALL_BUDGET_SECONDS:
            report['wall_budget_exceeded'] = True
        signal.alarm(CLEANUP_BUDGET_SECONDS)
        clean = True
        for server in servers:
            try:
                server.shutdown()
                server.server_close()
            except Exception:
                clean = False
        if client is not None and device is not None:
            try:
                local_close(client, device, exit_receipt)
            except Exception as error:
                report['local_exit_error'] = type(error).__name__
                clean = False
        owned = exit_receipt.get('owned_exit')
        if not isinstance(owned,dict) or owned.get('parent_normal_exit') is not True or owned.get('all_known_native_executions_exited') is not True:
            clean=False
        if isinstance(owned, dict):
            report['owned_exit'] = {key: owned.get(key) for key in EXIT_UI_KEYS
                                    if isinstance(owned.get(key), (bool, int))}
        if wm is not None:
            try:
                wm.close()
            except Exception:
                clean = False
        if device is not None:
            try:
                device.close()
            except Exception:
                clean = False
        signal.alarm(0)
        report['cleanup_completed'] = clean
        if (not work_complete or report.get('blocked_reason') or not clean
                or report.get('wall_budget_exceeded')
                or report.get('country_proof') != 'cf_trace_loc'):
            report['completed'] = False
        else:
            report['completed'] = True
        if guards_ok and out is not None:
            try:
                (out / 'result.json').write_text(
                    json.dumps(report, ensure_ascii=False, indent=1) + '\n')
            except Exception:
                pass
        print(json.dumps({'completed': report['completed'],
                          'stage': report['stage'],
                          'cleanup_completed': report['cleanup_completed']}))
    return 0 if report['completed'] else 1


if __name__ == '__main__':
    sys.exit(main())
