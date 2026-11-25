#!/usr/bin/env python3
"""GHA native RAM and post-restart resume + exact Super/context isolation.
Positions originate only from the admitted native snapshot. No resume book is
read/written by this fixture and no position/path is supplied to native IPC.
"""
import argparse
import json
import math
import os
from pathlib import Path
import secrets
import shutil
import sys
import tempfile
import time

from security_native import Device, call, js_wait, require, super_record, wait_for
from tab_routes import WindowsDevice, switch_super, rows, click_control
from youtube_player_native import Fixture, Trust, certificates, observe, physical, foreground, HOST, close_for_privacy, kernel_birth, trusted_fill, OwnedWindowManager
from hibernation_cookie_probe import tls_proxy_preflight

OWNERS = ('resume-a', 'resume-b')


def clocks(snapshot):
    values = [snapshot.get('position'), snapshot.get('audio_position')]
    skew = snapshot.get('skew')
    if not all(type(v) in (int, float) and math.isfinite(v) and 0 <= v <= 120 for v in values):
        return None
    if type(skew) not in (int, float) or not math.isfinite(skew) or not 0 <= skew <= .5:
        return None
    return values if abs(values[0] - values[1]) <= .5 else None


def position(snapshot):
    values = clocks(snapshot)
    return min(values) if values is not None else None


def clocks_in_interval(snapshot, low, high):
    values = clocks(snapshot)
    return values is not None and all(low <= value <= high for value in values)


def paused(snapshot):
    return (snapshot.get('playing') is False and snapshot.get('video_paused') is True
            and snapshot.get('audio_paused') is True)


def configured(snapshot):
    return snapshot.get('configured') is True and snapshot.get('disposed') is False and snapshot.get('separate') is True


def install_listeners(client):
    client.js('window.__playerObserved=[];window.__resumeResults=[];window.__resumeListenerReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-state")window.__playerObserved.push(e.payload);if(e.payload?.data?.tip==="youtube-result")window.__resumeResults.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__resumeListenerReady=true);true')
    js_wait(client, 'window.__resumeListenerReady', 'Native resume listener missing')


def activate(client, device, owner):
    foreground(device); switch_super(client, owner)
    if client.js('!!document.querySelector(".pusto button")'): click_control(client, '.pusto button', 20)
    def ready():
        session = rows(client, owner)
        tab = next((x for x in session['vkladki'] if x['id'] == session['aktivnaya']), {})
        return (session['aktivnaya'], tab['browser_id']) if type(tab.get('browser_id')) is int and tab['browser_id'] > 0 and not tab.get('zagruzka') else None
    return wait_for(ready, 'Native resume browser unavailable', 40)


PROBE_CODES = {
    'probe_wrong_thread', 'probe_browser_invalid', 'probe_frame_absent',
    'probe_document_not_admitted', 'probe_message_capacity', 'probe_observer_unavailable',
    'probe_dispatch_failed', 'probe_timeout', 'probe_document_changed',
    'cdp_method_failed', 'cdp_result_absent', 'snapshot_value_unavailable', 'cdp_agent_detached',
    'snapshot_undefined', 'snapshot_exception', 'snapshot_result_absent', 'snapshot_not_object',
    'snapshot_value_absent', 'snapshot_schema_mismatch', 'snapshot_field_invalid',
    'snapshot_invalid_json', 'snapshot_oversized',
}


RESUME_FAILURE_CODES={
    'resume_prepare_busy','resume_prepare_full','resume_prepare_unavailable','resume_prepare_storage',
    'resume_prepare_invalid_position','resume_prepare_timeout','resume_prepare_panic',
    'resume_admit_wrong_thread','resume_admit_stale','resume_admit_disabled','resume_admit_cancelled',
    'resume_admit_browser_changed','resume_admit_route_changed','resume_admit_private_changed',
    'resume_admit_document_unavailable','resume_admit_document_changed','resume_admit_scope_unavailable',
    'resume_admit_scope_changed','resume_open_refused'}


def fixture_ack_class(error,code=None):
    if code in RESUME_FAILURE_CODES:return code
    if error is None: return 'accepted'
    return {'Подготовка позиции видео недоступна': 'resume_preparation_refused',
            'Позиция видео сейчас недоступна': 'resume_setup_refused',
            'Вкладка недоступна': 'tab_unavailable',
            'Путь вкладки недоступен': 'route_unavailable',
            'Путь вкладки изменился': 'route_changed',
            'Диагностика доступна только в CI': 'ci_only',
            'Не удалось подготовить внутренний плеер': 'scheme_install_failed',
            'Не удалось открыть внутренний плеер': 'grant_failed',
            'Не удалось подготовить видео': 'config_failed'}.get(error, 'unknown_native_error') if isinstance(error, str) else 'invalid_native_error'


def admission_probe(client, owner, tab, diagnostics, receipt, *, admitting):
    snapshot = observe(client, owner, tab, admitting=admitting, diagnostics=diagnostics)
    expression = '(window.__playerObserved.find(x=>x.super_id===' + json.dumps(owner) + '&&x.data?.id===' + str(tab) + ')?.data?.code)??null'
    code = client.js(expression)
    receipt['probe_attempts'] += 1
    if code is not None:
        safe = code if isinstance(code, str) and code in PROBE_CODES else 'unknown_probe_code'
        diagnostics['native_probe_code'] = safe
        receipt['probe_codes'][safe] = receipt['probe_codes'].get(safe, 0) + 1
    if configured(snapshot):
        receipt['configured_snapshots'] += 1
        receipt['last_configured_clocks'] = {
            key: value for key, value in snapshot.items()
            if key in ('position', 'audio_position', 'skew') and type(value) in (int, float) and math.isfinite(value)}
    return snapshot


def admit(client, owner, tab, low, high, diagnostics):
    # Per-document fields must not retain a previous Super's successful sample.
    for key in ('last_snapshot', 'resources', 'resources_valid', 'native_probe_code',
                'native_probe_unavailable', 'extended_state_available'):
        diagnostics.pop(key, None)
    receipt = dict(scope_a=owner == OWNERS[0], scope_b=owner == OWNERS[1], initial_ack_seen=False,
                   terminal_ack_seen=False, terminal_code='not_received', terminal_codes={},
                   probe_attempts=0, configured_snapshots=0, probe_codes={})
    history = diagnostics.setdefault('resume_admissions', [])
    require(len(history) < 8, 'Resume admission diagnostic bound exceeded')
    history.append(receipt)
    session = rows(client, owner)
    selected = next((item for item in session.get('vkladki', []) if item.get('id') == tab), {})
    receipt['tab_ready_before'] = dict(selected=session.get('aktivnaya') == tab,
        native_browser=type(selected.get('browser_id')) is int and selected['browser_id'] > 0,
        loading=selected.get('zagruzka') is True, deferred=selected.get('otlozhena') is True,
        ordinary_blank=selected.get('adres') == 'about:blank')
    client.js('window.__resumeResults=[];true')
    call(client, 'obychnaya_komanda', id=owner, komanda={'deystvie': 'youtube_resume_fixture', 'id': tab})
    expression = 'window.__resumeResults.filter(x=>x.super_id===' + json.dumps(owner) + '&&x.data?.id===' + str(tab) + ').slice(-8).map(x=>({start:x.data?.start,error:x.data?.error??null,code:x.data?.code??null}))'
    def terminal():
        events = client.js(expression)
        if not isinstance(events, list): return None
        receipt['initial_ack_seen'] |= any(event.get('start') is True for event in events if isinstance(event, dict))
        result = next((event for event in events if isinstance(event, dict) and event.get('start') is False), None)
        if result is not None:
            receipt['terminal_ack_seen'] = True
            receipt['terminal_code'] = fixture_ack_class(result.get('error'),result.get('code'))
            receipt['terminal_codes'] = {receipt['terminal_code']: 1}
        return result
    acknowledgement = wait_for(terminal, 'Resume fixture terminal acknowledgement missing', 30)
    require(acknowledgement.get('error') is None and acknowledgement.get('code') is None, 'Resume fixture admission refused')
    def admitted():
        snapshot = admission_probe(client, owner, tab, diagnostics, receipt, admitting=True)
        if not configured(snapshot): return None
        # Never hide an early autoplay by waiting for it to become paused later.
        require(paused(snapshot), 'Resume fixture unexpectedly played during admission')
        return snapshot if clocks_in_interval(snapshot, low, high) else None
    snapshot = wait_for(admitted, 'Resume position outside expected native interval', 30)
    time.sleep(1.2)
    stable = admission_probe(client, owner, tab, diagnostics, receipt, admitting=False)
    require(configured(stable) and paused(stable), 'Resume fixture did not remain paused')
    require(clocks_in_interval(stable, low, high) and abs(position(stable) - position(snapshot)) <= .4,
            'Paused resume position advanced')
    return stable


def ordinary_navigation(client, owner, tab, browser):
    call(client, 'obychnaya_komanda', id=owner, komanda={'deystvie': 'pereyti', 'id': tab, 'adres': 'about:blank'})
    def navigated():
        item = next((x for x in rows(client, owner)['vkladki'] if x['id'] == tab), {})
        return item if item.get('adres') == 'about:blank' and not item.get('zagruzka') else None
    current = wait_for(navigated, 'Ordinary navigation did not finish', 30)
    require(current.get('browser_id') == browser, 'Ordinary navigation replaced resume context browser')
    client.js('window.__playerObserved=[];true')
    call(client, 'obychnaya_komanda', id=owner, komanda={'deystvie': 'youtube_snapshot', 'id': tab})
    expression = 'window.__playerObserved.find(x=>x.super_id===' + json.dumps(owner) + '&&x.data?.id===' + str(tab) + ')'
    event = wait_for(lambda: client.js(expression), 'Old document revocation observation missing', 10)
    require(not isinstance(event.get('data', {}).get('state'), dict)
            and isinstance(event.get('data', {}).get('error'), str), 'Old player document remained admitted')


def different_process_identity(previous, current):
    return previous != current and all(type(pid) is int and pid > 0 and isinstance(birth, str) and birth.isdigit()
                                       for pid, birth in (previous, current))


def native_owned_identities(client):
    import psutil
    clock = 'windows_filetime' if sys.platform == 'win32' else 'linux_ticks'
    state = call(client, 'super_lifecycle_snapshot')
    require(state.get('schema') == 1 and isinstance(state.get('engines'), list), 'Restart native ownership schema invalid')
    identities = set()
    for engine in state['engines']:
        require(engine.get('owned_complete') is True and isinstance(engine.get('owned'), list), 'Restart native ownership incomplete')
        for member in engine['owned']:
            pid, birth = member.get('pid'), member.get('birth')
            require(type(pid) is int and pid > 0 and member.get('clock') == clock
                    and isinstance(birth, str) and birth.isdigit(), 'Restart native identity invalid')
            try: actual = kernel_birth(pid, clock)
            except psutil.NoSuchProcess: continue
            require(actual == birth, 'Restart native identity changed')
            identities.add((pid, birth))
    require(bool(identities), 'Restart actual native processes missing')
    return identities


def run(args):
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Disposable GHA runner required')
    out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    report = dict(schema=1, completed=False, extractor_acceptance=False, durable_persistence_proven=False,
                  resume_book_injected=False, checks=[], stage='setup', player_diagnostics={})
    root = Path(tempfile.mkdtemp(prefix='nb-resume-'))
    device = fixture = trust = wm = None
    names = ('NEXUSBRO_TEST_YOUTUBE_DNS_GUARD', 'NEXUSBRO_TEST_YOUTUBE_RESUME')
    previous = {name: os.environ.get(name) for name in names}
    code = 1
    try:
        thumb = certificates(root / 'tls'); trust = Trust(root, root / 'tls/ca.crt', thumb); trust.install()
        fixture = Fixture(root / 'tls/server.crt', root / 'tls/server.key', Path(args.video_webm), Path(args.audio_webm))
        report['tls_preflight'] = tls_proxy_preflight(fixture.proxies['A'], HOST, 443, root / 'tls/ca.crt', root / 'tls/server.crt')
        exe, cef = Path(args.exe).resolve(), Path(args.cef).resolve()
        device = WindowsDevice(root / 'device', exe, cef) if sys.platform == 'win32' else Device('resume', root / 'device', exe, cef)
        definitions = []
        for lane, owner in zip(('A', 'B'), OWNERS):
            definition = super_record(owner, 'Resume ' + lane, 'about:blank', proxy='http://127.0.0.1:' + str(fixture.proxies[lane]))
            definition['poryadok'] = len(definitions); definitions.append(definition)
            profile = device.data / 'profili' / owner; profile.mkdir(parents=True)
            (profile / 'obychnye.json').write_text(json.dumps(dict(vkladki=[dict(id=1, adres='about:blank', zagolovok='Fixture', otlozhena=True)], aktivnaya=1, sleduyushchaya=2)))
        for filename in ('vkladki.json', 'sohranennye.json'): (device.data / filename).write_text(json.dumps(definitions))
        (device.data / 'nastrojki.json').write_text(json.dumps(dict(avtoobnovleniya=False, apparatnoe_uskorenie=False)))
        for name in names: os.environ[name] = '1'
        client = device.start()
        if sys.platform != 'win32' and wm is None:
            wm = OwnedWindowManager(device, root / 'wm'); device.player_wm = wm
        foreground(device)
        master = 'nb-resume-' + secrets.token_hex(16)
        require(call(client, 'security_vault', action='create', password=master).get('unlocked') is True, 'Fixture vault unavailable')
        install_listeners(client)
        report['stage'] = 'fresh-a'
        tab_a, browser_a = activate(client, device, OWNERS[0])
        initial = admit(client, OWNERS[0], tab_a, 0, 1, report['player_diagnostics'])
        report['checks'].append(dict(name='fresh-a', initially_paused=True, position=position(initial)))
        report['stage'] = 'physical-play-a'
        physical(client, device, 'play', report['player_diagnostics'])
        def progressed():
            snapshot = observe(client, OWNERS[0], tab_a, diagnostics=report['player_diagnostics'])
            value = position(snapshot)
            return snapshot if snapshot.get('playing') is True and snapshot.get('decoded', 0) >= 24 and value is not None and value >= 12 else None
        played = wait_for(progressed, 'Actual media did not advance twelve seconds', 40)
        high = position(played) + 2
        report['checks'].append(dict(name='physical-play-a', native_position=position(played), decoded=played['decoded']))
        report['stage'] = 'ordinary-navigation-a'
        ordinary_navigation(client, OWNERS[0], tab_a, browser_a)
        report['stage'] = 'resume-a'
        resumed = admit(client, OWNERS[0], tab_a, 5, high, report['player_diagnostics'])
        report['checks'].append(dict(name='same-context-resume-a', native_position=position(resumed), initially_paused=True,
                                     browser_identity_preserved=True, old_document_probe_refused=True))
        report['stage'] = 'other-scope-b'
        tab_b, browser_b = activate(client, device, OWNERS[1])
        isolated = admit(client, OWNERS[1], tab_b, 0, 1, report['player_diagnostics'])
        report['checks'].append(dict(name='other-super-context-b', native_position=position(isolated), initially_paused=True))
        ordinary_navigation(client, OWNERS[1], tab_b, browser_b)
        report['stage'] = 'return-a'
        returned_tab, returned_browser = activate(client, device, OWNERS[0])
        require(returned_tab == tab_a and returned_browser == browser_a, 'Returning changed original context browser')
        ordinary_navigation(client, OWNERS[0], tab_a, browser_a)
        returned = admit(client, OWNERS[0], tab_a, 5, high, report['player_diagnostics'])
        report['checks'].append(dict(name='return-a-retains-position', native_position=position(returned), initially_paused=True))
        report['stage'] = 'neutralize-exit-document-a'
        ordinary_navigation(client, OWNERS[0], tab_a, browser_a)
        report['exit_documents_neutral'] = True
        report['exit_during_playback'] = False
        report['stage'] = 'normal-owned-exit-before-restart'
        clock = 'windows_filetime' if sys.platform == 'win32' else 'linux_ticks'
        previous_parent = (device.app.pid, kernel_birth(device.app.pid, clock))
        previous_native = native_owned_identities(client)
        close_for_privacy(client, device, report)
        report['restart_owned_exit'] = report.pop('owned_exit')
        require(device.app is None and report['restart_owned_exit'].get('parent_normal_exit') is True
                and report['restart_owned_exit'].get('all_known_native_executions_exited') is True,
                'Restart old owned executions did not exit normally')
        # app=None: these public lifecycle methods close only stale inspector/
        # Windows policy handles, not a living acceptance process. Linux Xvfb
        # remains owned and available. Profiles/settings are not reseeded.
        device.stop_app()
        report['stage'] = 'restart-same-device'
        client = device.start(); foreground(device)
        current_parent = (device.app.pid, kernel_birth(device.app.pid, clock))
        require(different_process_identity(previous_parent, current_parent), 'Restart parent OS identity not fresh')
        report['stage'] = 'restart-native-master-unlock'
        js_wait(client, '!!document.querySelector(".browser-lock-screen")', 'Restart master lock screen missing', 25)
        trusted_fill(client, device, '[aria-label="Мастер-пароль для разблокировки браузера"]', master)
        client.text_button('Разблокировать браузер')
        js_wait(client, '!document.querySelector(".browser-lock-screen")&&!!document.querySelector(".okno")',
                'Restart actual master UI unlock failed', 25)
        require(call(client, 'security_vault', action='status').get('unlocked') is True, 'Restart native vault remained locked')
        install_listeners(client)
        report['stage'] = 'restart-resume-a'
        restarted_tab_a, _ = activate(client, device, OWNERS[0])
        current_native = native_owned_identities(client)
        require(previous_native.isdisjoint(current_native), 'Restart reused an old native OS execution')
        restarted_a = admit(client, OWNERS[0], restarted_tab_a, 5, high, report['player_diagnostics'])
        report['checks'].append(dict(name='durable-restart-a', native_position=position(restarted_a),
                                     initially_paused=True, old_owned_tree_exited=True, new_parent_identity=True,
                                     fresh_native_executions=True, native_members=len(current_native), actual_master_ui_unlock=True))
        report['stage'] = 'restart-isolated-b'
        restarted_tab_b, _ = activate(client, device, OWNERS[1])
        restarted_b = admit(client, OWNERS[1], restarted_tab_b, 0, 1, report['player_diagnostics'])
        report['checks'].append(dict(name='durable-restart-b-still-isolated', native_position=position(restarted_b), initially_paused=True))
        report['durable_persistence_proven'] = True
        report['stage'] = 'normal-owned-final-exit'
        close_for_privacy(client, device, report)
        report['final_owned_exit'] = report.pop('owned_exit')
        media = fixture.receipts
        report['stage'] = 'controlled-route-proof'
        report['controlled_route_observed'] = {
            lane: {track: sum(row.get('lane') == lane and row.get('track') == track for row in media)
                   for track in ('video', 'audio')}
            for lane in ('A', 'B', 'Direct')}
        report['controlled_route_observed']['other_tracks'] = sum(row.get('track') not in ('video', 'audio') for row in media)
        report['controlled_route_observed']['other_lanes'] = sum(row.get('lane') not in ('A', 'B', 'Direct') for row in media)
        report['controlled_route_observed']['direct_connection_attempts'] = fixture.direct
        require(fixture.direct == 0, 'Direct controlled media connection attempted')
        require(media and all(row['lane'] != 'Direct' for row in media), 'Unproxied controlled media request')
        require({row['track'] for row in media if row['lane'] == 'A'} == {'video', 'audio'}, 'Route A media proof missing')
        report['controlled_route_receipts'] = dict(direct=0, a=sum(row['lane'] == 'A' for row in media), b=sum(row['lane'] == 'B' for row in media))
        report.update(completed=True, stage='complete'); code = 0
    except Exception as error:
        reasons = {
            'Restart old owned executions did not exit normally': 'restart_old_owned_exit_failed',
            'Restart parent OS identity not fresh': 'restart_parent_not_fresh',
            'Restart master lock screen missing': 'restart_master_screen_missing',
            'Restart actual master UI unlock failed': 'restart_master_unlock_failed',
            'Restart native vault remained locked': 'restart_vault_locked',
            'Restart reused an old native OS execution': 'restart_native_identity_reused',
            'Restart native ownership schema invalid': 'restart_ownership_schema',
            'Restart native ownership incomplete': 'restart_ownership_incomplete',
            'Restart native identity invalid': 'restart_identity_invalid',
            'Restart native identity changed': 'restart_identity_changed',
            'Restart actual native processes missing': 'restart_native_missing',
            'Native resume listener missing': 'native_listener_missing',
            'Native resume browser unavailable': 'native_browser_unavailable',
            'Resume fixture acknowledgement missing': 'resume_ack_missing',
            'Resume fixture terminal acknowledgement missing': 'resume_terminal_ack_missing',
            'Resume admission diagnostic bound exceeded': 'resume_diagnostic_bound',
            'Resume fixture admission refused': 'resume_admission_refused',
            'Resume fixture unexpectedly played during admission': 'admission_autoplay',
            'Resume position outside expected native interval': 'resume_position_interval',
            'Resume fixture did not remain paused': 'resume_not_paused',
            'Paused resume position advanced': 'paused_position_advanced',
            'Ordinary navigation did not finish': 'ordinary_navigation_timeout',
            'Ordinary navigation replaced resume context browser': 'native_context_browser_changed',
            'Old document revocation observation missing': 'revocation_receipt_missing',
            'Old player document remained admitted': 'old_document_still_admitted',
            'Actual media did not advance twelve seconds': 'actual_playback_not_advanced',
            'Returning changed original context browser': 'return_context_changed',
            'Direct controlled media connection attempted': 'direct_controlled_connection',
            'Unproxied controlled media request': 'unproxied_controlled_media',
            'Route A media proof missing': 'routed_media_missing',
        }
        report['failure_class'] = reasons.get(str(error), 'native_resume_check_failed')
        classes={'NativeFailure','AssertionError','ValueError','AttributeError','TimeoutExpired','CalledProcessError','RuntimeError'}
        report['failure_exception_class']=type(error).__name__ if type(error).__name__ in classes else 'other'
        trace=error.__traceback__
        while trace:
            source=Path(trace.tb_frame.f_code.co_filename).name
            function=trace.tb_frame.f_code.co_name
            if source in {'youtube_player_native.py','youtube_resume_native.py'} and len(function)<=64 and function.isidentifier():
                report['failure_location']={'source':source,'function':function,'line':trace.tb_lineno}
            trace=trace.tb_next

        # Only exact source literals are mapped; raw errors/paths never leave.

    finally:
        cleanup = True
        for owned in (wm, device, fixture, trust):
            if owned:
                try: owned.close()
                except Exception: cleanup = False
        report['cleanup_completed'] = cleanup
        if not cleanup: report['completed'] = False; code = 1
        shutil.rmtree(root, ignore_errors=True)
        for name, value in previous.items():
            if value is None: os.environ.pop(name, None)
            else: os.environ[name] = value
        (out / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    return code


def self_test():
    assert fixture_ack_class(None) == 'accepted'
    assert fixture_ack_class('Подготовка позиции видео недоступна') == 'resume_preparation_refused'
    assert fixture_ack_class('Подготовка позиции видео недоступна','resume_prepare_storage') == 'resume_prepare_storage'
    private = 'https://private.invalid/path secret'
    assert private not in fixture_ack_class(private)
    assert private not in fixture_ack_class(private,private)
    assert 'probe_document_not_admitted' in PROBE_CODES and 'probe_timeout' in PROBE_CODES
    assert different_process_identity((1, '10'), (2, '11'))
    assert different_process_identity((1, '10'), (1, '11'))  # PID reuse is a new OS identity.
    assert not different_process_identity((1, '10'), (1, '10'))
    assert not different_process_identity((1, '10'), (2, 'invalid'))
    snapshot = dict(configured=True, disposed=False, separate=True, playing=False, video_paused=True, audio_paused=True, position=9., audio_position=8.9, skew=.1)
    assert configured(snapshot) and paused(snapshot) and position(snapshot) == 8.9
    assert not paused(dict(snapshot, playing=True))
    assert position(dict(snapshot, audio_position=float('nan'))) is None
    assert position(dict(snapshot, position=True)) is None
    assert clocks_in_interval(snapshot, 5, 10)
    for video, audio in ((0, 12), (12, 0), (10, 95), (95, 10)):
        wrong = dict(snapshot, position=video, audio_position=audio, skew=0)
        assert not clocks_in_interval(wrong, 0, 1)
        assert not clocks_in_interval(wrong, 5, 14)
        assert position(wrong) is None
    assert not clocks_in_interval(dict(snapshot, position=9.9, audio_position=10.1, skew=.2), 5, 10)
    assert not clocks_in_interval(dict(snapshot, skew=.51), 5, 10)
    assert not clocks_in_interval(dict(snapshot, skew=float('nan')), 5, 10)
    print(json.dumps(dict(self_test=True, native_execution=False, durable_persistence_proven=False)))


if __name__ == '__main__':
    if sys.argv[1:] == ['--self-test']: self_test(); raise SystemExit(0)
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('exe', 'cef', 'video-webm', 'audio-webm', 'out'): parser.add_argument('--' + name, required=True)
    raise SystemExit(run(parser.parse_args()))
