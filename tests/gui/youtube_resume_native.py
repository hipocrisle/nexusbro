#!/usr/bin/env python3
"""GHA native RAM resume + exact Super/context isolation, not disk durability.
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
from youtube_player_native import Fixture, Trust, certificates, observe, physical, foreground, HOST
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


def admit(client, owner, tab, low, high, diagnostics):
    client.js('window.__resumeResults=[];true')
    call(client, 'obychnaya_komanda', id=owner, komanda={'deystvie': 'youtube_resume_fixture', 'id': tab})
    expression = 'window.__resumeResults.find(x=>x.super_id===' + json.dumps(owner) + '&&x.data?.id===' + str(tab) + ')'
    acknowledgement = wait_for(lambda: client.js(expression), 'Resume fixture acknowledgement missing', 30)
    require(acknowledgement.get('data', {}).get('error') is None, 'Resume fixture admission refused')
    def admitted():
        snapshot = observe(client, owner, tab, admitting=True, diagnostics=diagnostics)
        if not configured(snapshot): return None
        # Never hide an early autoplay by waiting for it to become paused later.
        require(paused(snapshot), 'Resume fixture unexpectedly played during admission')
        return snapshot if clocks_in_interval(snapshot, low, high) else None
    snapshot = wait_for(admitted, 'Resume position outside expected native interval', 30)
    time.sleep(1.2)
    stable = observe(client, owner, tab, diagnostics=diagnostics)
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


def run(args):
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Disposable GHA runner required')
    out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    report = dict(schema=1, completed=False, extractor_acceptance=False, durable_persistence_proven=False,
                  resume_book_injected=False, checks=[], stage='setup', player_diagnostics={})
    root = Path(tempfile.mkdtemp(prefix='nb-resume-'))
    device = fixture = trust = None
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
        client = device.start(); foreground(device)
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
        tab_b, _ = activate(client, device, OWNERS[1])
        isolated = admit(client, OWNERS[1], tab_b, 0, 1, report['player_diagnostics'])
        report['checks'].append(dict(name='other-super-context-b', native_position=position(isolated), initially_paused=True))
        report['stage'] = 'return-a'
        returned_tab, returned_browser = activate(client, device, OWNERS[0])
        require(returned_tab == tab_a and returned_browser == browser_a, 'Returning changed original context browser')
        ordinary_navigation(client, OWNERS[0], tab_a, browser_a)
        returned = admit(client, OWNERS[0], tab_a, 5, high, report['player_diagnostics'])
        report['checks'].append(dict(name='return-a-retains-position', native_position=position(returned), initially_paused=True))
        media = fixture.receipts
        require(media and all(row['lane'] != 'Direct' for row in media), 'Unproxied controlled media request')
        require({row['track'] for row in media if row['lane'] == 'A'} == {'video', 'audio'}, 'Route A media proof missing')
        report['controlled_route_receipts'] = dict(direct=0, a=sum(row['lane'] == 'A' for row in media), b=sum(row['lane'] == 'B' for row in media))
        report.update(completed=True, stage='complete'); code = 0
    except Exception as error:
        reasons = {
            'Native resume listener missing': 'native_listener_missing',
            'Native resume browser unavailable': 'native_browser_unavailable',
            'Resume fixture acknowledgement missing': 'resume_ack_missing',
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
            'Unproxied controlled media request': 'unproxied_controlled_media',
            'Route A media proof missing': 'routed_media_missing',
        }
        report['failure_class'] = reasons.get(str(error), 'native_resume_check_failed')
        # Only exact source literals are mapped; raw errors/paths never leave.

    finally:
        cleanup = True
        for owned in (device, fixture, trust):
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
