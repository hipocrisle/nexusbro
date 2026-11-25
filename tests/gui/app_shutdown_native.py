#!/usr/bin/env python3
"""GHA-only real prepare-update drain, without an installer or restart.
One native controlled media context, real OS births and strict clean receipts.
This is not extractor-job cancellation or universal process-containment proof.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import threading
import time

import psutil
from security_native import Device, call, js_wait, require, super_record, wait_for
from tab_routes import WindowsDevice, switch_super, rows, click_control
from youtube_player_native import (Fixture, Trust, certificates, observe, physical,
    foreground, HOST, kernel_birth, native_owner, OwnedWindowManager,
    read_shutdown_receipt, native_app_identity, native_app_identity_alive, native_app_execution_alive)
from hibernation_cookie_probe import tls_proxy_preflight

OWNER = 'prepare-update-a'


def parse_native_members(snapshot, clock):
    require(isinstance(snapshot, dict) and type(snapshot.get('schema')) is int and snapshot.get('schema') == 1
            and isinstance(snapshot.get('engines'), list)
            and 1 <= len(snapshot['engines']) <= 16, 'Prepare ownership invalid')
    members = {}
    owners = 0
    for engine in snapshot['engines']:
        require(isinstance(engine, dict) and engine.get('owned_complete') is True
                and isinstance(engine.get('owned'), list), 'Prepare ownership incomplete')
        if engine['owned']: owners += 1
        require(len(engine['owned']) <= 1024, 'Prepare ownership invalid')
        for member in engine['owned']:
            require(isinstance(member, dict) and type(member.get('pid')) is int and 0 < member['pid'] <= 0xffffffff
                    and isinstance(member.get('birth'), str) and 1 <= len(member['birth']) <= 24
                    and member['birth'].isdigit() and member.get('clock') == clock,
                    'Prepare ownership invalid')
            pid, birth = member['pid'], member['birth']
            require(pid not in members or members[pid] == birth, 'Prepare identity changed')
            members[pid] = birth
    require(owners > 0 and 0 < len(members) <= 1024, 'Prepare native members missing')
    return members, owners


class OwnedExecutions:
    """OS-only observer during drain: custom native IPC is then intentionally shut."""
    def __init__(self, device, snapshot):
        self.device = device
        self.clock = 'windows_filetime' if sys.platform == 'win32' else 'linux_ticks'
        self.native, self.owners = parse_native_members(snapshot, self.clock)
        self.app_identity = native_app_identity(device)
        self.parent = self.app_identity['pid']
        self.parent_birth = self.app_identity['birth']
        self.wrapper = self.app_identity['wrapper_pid']
        self.wrapper_birth = self.app_identity['wrapper_birth']
        self.whole = {self.parent:self.parent_birth,self.wrapper:self.wrapper_birth}
        for pid, birth in self.native.items():
            try: actual = kernel_birth(pid, self.clock)
            except psutil.NoSuchProcess: continue
            require(actual == birth, 'Prepare identity changed')
            self.whole[pid] = birth
        self.lock = threading.Lock(); self.done = threading.Event(); self.error = False
        self.sample()
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def remember_tree(self, pid, birth, target):
        try:
            if kernel_birth(pid, self.clock) != birth: return
            root = psutil.Process(pid)
            for process in [root] + root.children(recursive=True):
                try:
                    current = kernel_birth(process.pid, self.clock)
                    require(process.pid not in target or target[process.pid] == current,
                            'Prepare identity changed')
                    target[process.pid] = current
                    require(len(target) <= 2048, 'Prepare process observation bound')
                except psutil.NoSuchProcess:
                    pass
        except psutil.NoSuchProcess:
            pass

    def sample(self):
        with self.lock:
            # Known native members stay roots even if the original launcher exits.
            for pid, birth in list(self.native.items()):
                self.remember_tree(pid, birth, self.native)
            self.whole.update(self.native)
            self.remember_tree(self.wrapper, self.wrapper_birth, self.whole)

    def run(self):
        while not self.done.is_set():
            try: self.sample()
            except Exception: self.error = True; return
            self.done.wait(.02)

    def parent_alive(self):
        require(not self.error, 'Prepare process observation incomplete')
        return native_app_identity_alive(self.device,self.app_identity)

    def gone(self, *, native):
        require(not self.error, 'Prepare process observation incomplete')
        with self.lock: identities = list((self.native if native else self.whole).items())
        for pid, birth in identities:
            try:
                process = psutil.Process(pid)
                if process.status() == psutil.STATUS_ZOMBIE or not process.is_running(): continue
                if kernel_birth(pid, self.clock) == birth: return False
            except psutil.NoSuchProcess:
                pass
        return True

    def counts(self):
        with self.lock: return len(self.native), len(self.whole)

    def close(self):
        self.done.set(); self.thread.join(timeout=3)
        require(not self.thread.is_alive() and not self.error, 'Prepare process observation incomplete')


def install_listeners(client):
    client.js('window.__playerObserved=[];window.__playerFixtureResults=[];window.__prepareListenerReady=false;'
              'window.__prepareHandler=window.__TAURI_INTERNALS__.transformCallback(e=>{'
              'if(e.payload?.data?.tip==="youtube-state")window.__playerObserved.push(e.payload);'
              'if(e.payload?.data?.tip==="youtube-result")window.__playerFixtureResults.push(e.payload)});'
              'window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",'
              'target:{kind:"Window",label:"main"},handler:window.__prepareHandler})'
              '.then(()=>window.__prepareListenerReady=true);true')
    js_wait(client, 'window.__prepareListenerReady', 'Prepare listener unavailable')


def request_prepare(client):
    # Exact trusted native command. The JS stores only its real promise outcome.
    client.js('window.__prepareUpdateResult=null;window.__TAURI_INTERNALS__.invoke("app_prepare_update")'
              '.then(()=>window.__prepareUpdateResult={ok:true},()=>window.__prepareUpdateResult={ok:false});true')
    js_wait(client, 'window.__prepareUpdateResult!==null', 'Prepare command timed out', 25)
    require(client.js('window.__prepareUpdateResult?.ok===true') is True, 'Prepare command refused')


def normal_close(device):
    # Reuse the same exact native owner identity as existing physical fixtures.
    if sys.platform == 'win32': native_owner(device).close()
    else:
        subprocess.run(['wmctrl', '-ic', str(native_owner(device))], env=device.player_wm.env,
                       check=True, timeout=5, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def run(args):
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Disposable GHA runner required')
    out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    root = Path(tempfile.mkdtemp(prefix='nb-prepare-update-'))
    report = dict(schema=1, completed=False, release_acceptance_passed=False,
                  installer_invoked=False, restart_invoked=False, extractor_cancellation_proven=False,
                  test_kill_used_for_acceptance=False, checks=[], stage='setup')
    device = fixture = trust = wm = witness = None
    previous = os.environ.get('NEXUSBRO_TEST_YOUTUBE_DNS_GUARD')
    code = 1
    try:
        thumb = certificates(root/'tls'); trust = Trust(root, root/'tls/ca.crt', thumb); trust.install()
        fixture = Fixture(root/'tls/server.crt', root/'tls/server.key', Path(args.video_webm), Path(args.audio_webm))
        report['tls_preflight'] = tls_proxy_preflight(fixture.proxies['A'], HOST, 443,
                                                     root/'tls/ca.crt', root/'tls/server.crt')
        exe, cef = Path(args.exe).resolve(), Path(args.cef).resolve()
        device = WindowsDevice(root/'device', exe, cef) if sys.platform == 'win32' else Device('prepare', root/'device', exe, cef)
        definitions = [super_record(OWNER, 'Prepare update fixture', 'about:blank',
                                    proxy='http://127.0.0.1:'+str(fixture.proxies['A']))]
        profile = device.data/'profili'/OWNER; profile.mkdir(parents=True)
        (profile/'obychnye.json').write_text(json.dumps(dict(vkladki=[dict(id=1, adres='about:blank',
            zagolovok='Fixture', otlozhena=True)], aktivnaya=1, sleduyushchaya=2)), encoding='utf-8')
        for filename in ('vkladki.json', 'sohranennye.json'):
            (device.data/filename).write_text(json.dumps(definitions), encoding='utf-8')
        (device.data/'nastrojki.json').write_text(json.dumps(dict(avtoobnovleniya=False,
            apparatnoe_uskorenie=False)), encoding='utf-8')
        os.environ['NEXUSBRO_TEST_YOUTUBE_DNS_GUARD'] = '1'
        client = device.start()
        if sys.platform != 'win32':
            wm = OwnedWindowManager(device, root/'wm'); device.player_wm = wm
        foreground(device)
        master = 'nb-prepare-'+secrets.token_hex(16)
        require(call(client, 'security_vault', action='create', password=master).get('unlocked') is True,
                'Prepare vault unavailable')
        install_listeners(client)
        switch_super(client, OWNER)
        if client.js('!!document.querySelector(".pusto button")'): click_control(client, '.pusto button', 20)
        session = wait_for(lambda: rows(client, OWNER) if any(t.get('browser_id', 0)>0
            and not t.get('zagruzka') for t in rows(client, OWNER)['vkladki']) else None,
            'Prepare native browser unavailable', 40)
        tab = session['aktivnaya']; report['stage'] = 'actual-media-before-prepare'
        call(client, 'obychnaya_komanda', id=OWNER, komanda={'deystvie':'youtube_fixture', 'id':tab})
        wait_for(lambda: (lambda s: s if s.get('configured') and not s.get('disposed') else None)(
            observe(client, OWNER, tab, admitting=True)), 'Prepare fixture admission failed', 30)
        physical(client, device, 'play', {})
        state = wait_for(lambda: (lambda s: s if s.get('playing') and s.get('decoded', 0)>=6 else None)(
            observe(client, OWNER, tab)), 'Prepare physical playback unavailable', 25)
        require(state.get('video_width') == 320 and state.get('video_height') == 180,
                'Prepare actual media geometry mismatch')
        with fixture.lock:
            require(fixture.direct == 0 and {r['track'] for r in fixture.receipts if r['lane']=='A'}
                    == {'audio','video'}, 'Prepare actual route proof missing')
        witness = OwnedExecutions(device, call(client, 'super_lifecycle_snapshot'))
        require(witness.parent_alive(), 'Prepare parent identity changed')
        report['checks'].append(dict(name='actual-owned-media', owners=witness.owners,
                                    native_members=witness.counts()[0], decoded=state['decoded']))
        report['stage'] = 'prepare-update'
        request_prepare(client)
        require(witness.parent_alive(), 'Prepare exited parent prematurely')
        wait_for(lambda: witness.gone(native=True), 'Prepare native executions remain', 15)
        receipt = read_shutdown_receipt(device.data, witness.parent)
        require(receipt['prepared_update'] is True, 'Prepare receipt mode invalid')
        report['prepared_receipt'] = receipt
        report['app_identity_binding'] = dict(verified=True,launcher_is_actual_app=witness.parent==witness.wrapper,
                                             actual_app_and_launcher_alive=True)
        for _ in range(5):
            require(witness.parent_alive() and witness.gone(native=True), 'Prepare parent or native state changed')
            time.sleep(.2)
        report['checks'].append(dict(name='prepared-clean-parent-alive', owners=receipt['owners'],
            clean=receipt['clean'], native_members_exited=witness.counts()[0], observed_alive_ms=1000,
            native_scope='receipt_plus_bounded_known_OS_births_not_universal_escape_proof'))
        report['stage'] = 'ordinary-close-after-prepare'
        normal_close(device)
        require(device.app.wait(timeout=30) == 0, 'Prepare final parent exit failed')
        wait_for(lambda: witness.gone(native=False), 'Prepare final owned executions remain', 30)
        witness.close()
        require(not native_app_execution_alive(witness.app_identity),'Native app execution remained alive')
        # The parent is reaped and all known births are gone: emergency cleanup
        # must never signal a recycled PID or reinterpret this as a test kill.
        device.app = None
        report['final_receipt'] = read_shutdown_receipt(device.data, witness.parent)
        report['checks'].append(dict(name='ordinary-close-after-prepare', parent_exit_zero=True,
                                     known_owned_executions_exited=witness.counts()[1],actual_app_execution_exited=True,launcher_exit_zero=True))
        report.update(completed=True, stage='complete'); code = 0
    except Exception as error:
        known = {'Prepare ownership invalid','Prepare ownership incomplete','Prepare identity changed',
            'Prepare native members missing','Prepare process observation bound','Prepare process observation incomplete',
            'Prepare listener unavailable','Prepare command timed out','Prepare command refused',
            'Prepare vault unavailable','Prepare native browser unavailable','Prepare fixture admission failed',
            'Prepare physical playback unavailable','Prepare actual media geometry mismatch','Prepare actual route proof missing',
            'Prepare parent identity changed','Prepare exited parent prematurely','Prepare native executions remain',
            'Prepare receipt mode invalid','Prepare parent or native state changed','Prepare final parent exit failed',
            'Prepare final owned executions remain','Native app identity invalid','Native app execution remained alive','Shutdown receipt missing','Shutdown receipt invalid','Shutdown receipt unclean'}
        report['failure_code'] = str(error).lower().replace(' ', '_') if str(error) in known else 'native_prepare_check_failed'
        report['failure_class'] = type(error).__name__ if type(error).__name__ in {'NativeFailure','AssertionError','TimeoutExpired','OSError'} else 'other'
    finally:
        if witness:
            try: witness.close()
            except Exception: report['completed']=False; code=1
        report['emergency_cleanup_required'] = bool(device and device.app is not None and device.app.poll() is None)
        if report['emergency_cleanup_required']: report['completed']=False; code=1
        cleanup = True
        for owned in (wm, device, fixture, trust):
            if owned:
                try: owned.close()
                except Exception: cleanup=False
        report['cleanup_completed']=cleanup
        if not cleanup: report['completed']=False; code=1
        shutil.rmtree(root, ignore_errors=True)
        if previous is None: os.environ.pop('NEXUSBRO_TEST_YOUTUBE_DNS_GUARD', None)
        else: os.environ['NEXUSBRO_TEST_YOUTUBE_DNS_GUARD']=previous
        (out/'result.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
    return code


def self_test():
    clock='linux_ticks'
    member=dict(pid=123, birth='456', clock=clock)
    snapshot=dict(schema=1, engines=[dict(owned_complete=True, owned=[member])])
    assert parse_native_members(snapshot, clock)==({123:'456'},1)
    for bad in [dict(schema=True, engines=[dict(owned_complete=True,owned=[member])]),
                dict(schema=1, engines=[]), dict(schema=1, engines=[dict(owned_complete=False,owned=[member])]),
                dict(schema=1, engines=[dict(owned_complete=True,owned=[dict(member,birth='private')])]),
                dict(schema=1, engines=[dict(owned_complete=True,owned=[dict(member,pid=True)])]),
                dict(schema=1, engines=[dict(owned_complete=True,owned=[member,dict(member,birth='789')])])]:
        try: parse_native_members(bad, clock)
        except Exception: pass
        else: raise AssertionError('Unproven native ownership accepted')
    return dict(self_test=True, synthetic_only=True, native_execution=False, release_acceptance_passed=False)


if __name__=='__main__':
    if sys.argv[1:]==['--self-test']: print(json.dumps(self_test())); raise SystemExit(0)
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('exe','cef','video-webm','audio-webm','out'): parser.add_argument('--'+name, required=True)
    raise SystemExit(run(parser.parse_args()))
