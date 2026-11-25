#!/usr/bin/env python3
"""Local Home probe for the shipped lightweight YouTube player (light mode).

Diagnostic only, never playback acceptance. Complements the official-embed
Home entry: exercises the PRODUCT's own lightweight youtube_light pipeline
(components: pinned yt-dlp + Deno under the exe-adjacent binaries/youtube-light
directory, fixed app arg --nb-youtube-components) in a fresh local profile
with a normal watch?v= URL. No GHA, no sandbox disable, no owner DISPLAY/GPU,
no session/cookie import. ROOT owns the shared local entry module; this probe
reuses its validated helpers through import and changes nothing there.

Proofs, bounded: one ordinary shipped youtube_light snapshot command per
observation; ONE physical play through the existing physical() helper
(controls painted proof + single click); success requires state position
delta >=8s AND playing AND audio policy presence AND decoded frames counter
advancing (not an image-only render). No audio capture, no ads/efficiency
claims. full_playback_acceptance=false and paired=false in every outcome;
OS-normal exit via the shared local_close witness, GHA receipt never claimed.
"""
import argparse
import hashlib
import json
import math
import os
import re
import shutil
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from security_native import require, wait_for
from youtube_embed_reference_native import PREFLIGHT_COMMANDS
from youtube_embed_home_probe import (
    validate_private_root, validate_inside_root, SandboxedDevice, local_close,
    initial_local_report, WORK_BUDGET_SECONDS, CLEANUP_BUDGET_SECONDS,
)
try:
    from youtube_embed_home_probe import LocalWindowManager
except ImportError:  # older committed local entry without the WM subclass
    from youtube_paired_native import OwnedWindowManager as LocalWindowManager
from youtube_paired_native import (
    rows, switch_super, click_control, Blocked, super_record, call, audio_policy, probe,
)
from youtube_player_native import physical

VIDEO = 'M7lc1UVf-VE'
COMPONENT_FILES = ('yt-dlp', 'deno')
SAFE_CODE = re.compile(r'^[a-z0-9_]{1,48}$')
MIN_POSITION_DELTA = 8.0


def validate_components(raw_components, exe, root):
    """The app fixes the components directory itself (cef_dvizhok.rs:
    resources/binaries/youtube-light). Preflight must only accept exactly that
    path inside the private root, with real non-empty component files."""
    expected = (exe.parent / 'binaries' / 'youtube-light').resolve(strict=True)
    try:
        value = Path(raw_components).resolve(strict=True)
    except OSError:
        raise Blocked('local_components_unresolvable')
    if value != expected:
        raise Blocked('local_components_not_exe_adjacent')
    if root not in value.parents:
        raise Blocked('local_components_outside_root')
    for name in COMPONENT_FILES:
        path = value / name
        try:
            st = os.lstat(path)
        except OSError:
            raise Blocked('local_components_missing_' + name)
        if not os.path.isfile(path) or os.path.islink(path):
            raise Blocked('local_components_invalid_' + name)
        if st.st_size <= 0:
            raise Blocked('local_components_empty_' + name)
    return value


def light_state(state):
    """Typed light snapshot validation; only scalars leave, never strings."""
    if not isinstance(state, dict):
        raise Blocked('native_light_unavailable')
    numbers = {}
    for key in ('position', 'duration', 'decoded', 'dropped', 'video_width', 'video_height',
                'ready_state', 'volume', 'playback_rate'):
        value = state.get(key)
        if type(value) not in (int, float) or isinstance(value, bool) or not math.isfinite(value) or value < 0:
            raise Blocked('native_light_state_invalid')
        numbers[key] = value
    if type(state.get('playing')) is not bool:
        raise Blocked('native_light_state_invalid')
    numbers['playing'] = state['playing']
    audio_policy(state)  # volume==1, muted False, default_sink True; raises Blocked otherwise
    if numbers['duration'] <= 0 or numbers['video_width'] <= 0 or numbers['video_height'] <= 0:
        raise Blocked('native_light_state_invalid')
    return numbers


def failure_code(receipt):
    """Safe failure code from the shipped youtube-state error shape only."""
    if not isinstance(receipt, dict):
        return None
    code = receipt.get('code')
    if type(code) is str and SAFE_CODE.fullmatch(code):
        return code
    return None


def initial_light_report():
    report = initial_local_report()
    report.update({'mode': 'local_home_light', 'video': VIDEO,
                   'position_delta_seconds': 0.0, 'decoded_frames_advanced': False,
                   'audio_policy_observed': False, 'playing_observed': False,
                   'native_failure_code': None, 'full_playback_acceptance': False, 'paired': False,
                   'stage': 'setup'})
    return report


def run(args):
    report = initial_light_report(); device = wm = None; root = None; client = None
    close_attempted = False; started = time.monotonic()
    try:
        if os.environ.get('GITHUB_ACTIONS') == 'true':
            raise Blocked('local_mode_forbids_gha')
        if sys.platform != 'linux':
            raise Blocked('local_linux_only')
        expected_uid = int(args.expect_uid)
        if expected_uid == 0 or os.getuid() != os.geteuid() or os.geteuid() != expected_uid:
            raise Blocked('local_uid_mismatch')
        if not args.allow_local_virtual_display:
            raise Blocked('local_virtual_display_not_allowed')
        report['stage'] = 'preflight'
        for command in PREFLIGHT_COMMANDS:
            if shutil.which(command) is None:
                raise Blocked('embed_preflight_missing_' + command)
        report['stage'] = 'private-root'
        root = validate_private_root(args.private_root, expected_uid)
        exe = validate_inside_root(args.exe, root, 'exe')
        cef = validate_inside_root(args.cef, root, 'cef')
        digest = hashlib.sha256()
        with exe.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
        if digest.hexdigest() != args.exe_sha256:
            raise Blocked('local_exe_sha256_mismatch')
        components = validate_components(args.components, exe, root)
        report['components_path_shape'] = 'exe-adjacent-binaries-youtube-light'
        out = root / 'out' / ('light-home-' + str(int(time.time())))
        out.mkdir(parents=True, exist_ok=False)
        work_deadline = started + WORK_BUDGET_SECONDS
        def remaining(cap):
            left = work_deadline - time.monotonic()
            if left <= 0:
                raise Blocked('light_work_deadline')
            return min(cap, left)
        url = 'https://www.youtube.com/watch?v=' + VIDEO
        report['stage'] = 'startup'
        device = SandboxedDevice('light-home', root / 'device', exe, cef)
        identifier = 'light-home'
        record = super_record(identifier, 'Lightweight player local', url, proxy='')
        folder = device.data / 'profili' / identifier; folder.mkdir(parents=True)
        (folder / 'obychnye.json').write_text(json.dumps({'vkladki': [{'id': 1, 'adres': url, 'zagolovok': 'Light local', 'otlozhena': True}], 'aktivnaya': 1, 'sleduyushchaya': 2}))
        for name in ('vkladki.json', 'sohranennye.json'):
            (device.data / name).write_text(json.dumps([record]))
        (device.data / 'nastrojki.json').write_text(json.dumps({'avtoobnovleniya': False, 'apparatnoe_uskorenie': False, 'memory_saver': False}))
        client = device.start()
        if time.monotonic() - started > 45:
            raise Blocked('light_startup_deadline')
        report['stage'] = 'owned-window-manager'
        wm = LocalWindowManager(device, root / 'wm'); device.player_wm = wm
        switch_super(client, identifier, timeout=remaining(10))
        if client.js('!!document.querySelector(".pusto button")'):
            click_control(client, '.pusto button', remaining(10))
        click_control(client, '.obychnaya[data-id="1"]', remaining(10))
        report['stage'] = 'tab-live'
        deadline = time.monotonic() + remaining(60); tab = None
        while time.monotonic() < deadline:
            session = rows(client, identifier, timeout=remaining(3)); tab = session.get('aktivnaya')
            if any(v.get('id') == tab and v.get('browser_id', 0) > 0 for v in session.get('vkladki', [])):
                break
            time.sleep(.5)
        else:
            raise Blocked('light_tab_not_live')
        client.js('window.__paired=[];window.__pairedReady=false;const h=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-state")window.__paired.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:h}).then(()=>window.__pairedReady=true);true')
        wait_for(lambda: client.js('window.__pairedReady'), 'Light state listener unavailable', remaining(5))
        report['stage'] = 'light-snapshot'
        first = None; last = None; began = None
        deadline = time.monotonic() + remaining(90)
        while time.monotonic() < deadline:
            try:
                state = light_state(probe(client, identifier, tab, 'light', VIDEO))
            except Blocked as error:
                if str(error) != 'native_light_unavailable':
                    raise
                # Distinguish shipped safe failure categories from plain absence.
                receipt = client.js('window.__paired.find(e=>e.data?.id==='+str(tab)+'&&e.data?.tip==="youtube-state")')
                code = failure_code(receipt.get('data') if isinstance(receipt, dict) else None)
                report['native_failure_code'] = code or 'unavailable'
                raise Blocked('native_light_' + (code or 'unavailable'))
            if first is None:
                first = state; began = time.monotonic()
                if not first['playing']:
                    break  # proceed to physical play below
            else:
                last = state
                if last['position'] - first['position'] >= MIN_POSITION_DELTA and last['decoded'] > first['decoded'] and last['playing']:
                    break
            time.sleep(1.0)
        if first is None:
            raise Blocked('native_light_unavailable')
        report['stage'] = 'physical-play'
        if not first['playing']:
            diagnostics = {}
            physical(client, device, 'play', diagnostics)  # one physical click, painted proof inside
            report['physical_play'] = {'control': diagnostics.get('control', {}), 'clicked': diagnostics.get('control', {}).get('clicked', False)}
        report['stage'] = 'observation'
        deadline = time.monotonic() + remaining(120)
        while time.monotonic() < deadline:
            state = light_state(probe(client, identifier, tab, 'light', VIDEO))
            last = state
            if state['playing'] and state['position'] - first['position'] >= MIN_POSITION_DELTA:
                break
            time.sleep(1.0)
        delta = round(last['position'] - first['position'], 3) if last else 0.0
        report['position_delta_seconds'] = delta
        report['decoded_frames_advanced'] = bool(last and last['decoded'] > first['decoded'])
        report['audio_policy_observed'] = True
        report['playing_observed'] = bool(last and last['playing'] and delta >= MIN_POSITION_DELTA
                                          and report['decoded_frames_advanced'])
        if not report['playing_observed']:
            raise Blocked('light_playback_not_observed')
        report['stage'] = 'local-normal-owned-exit'; close_attempted = True
        local_close(client, device, report)
        report['completed'] = True; report['stage'] = 'complete'
    except Blocked as error:
        report['blocked_reason'] = str(error)
    except Exception:
        report['blocked_reason'] = 'light_home_probe_unavailable'
    finally:
        if client is not None and not close_attempted and device is not None and device.app is not None:
            prior = report['stage']
            try:
                close_attempted = True; local_close(client, device, report)
            except Exception:
                report['normal_exit_incomplete'] = True
            report['stage'] = prior
        cleanup = True
        for obj in (device, wm):
            if obj:
                try:
                    obj.close()
                except Exception:
                    cleanup = False
        report['cleanup_completed'] = cleanup
        if not cleanup:
            report['completed'] = False; report['blocked_reason'] = 'cleanup_failed'
        if not report.get('owned_exit', {}).get('parent_normal_exit'):
            report['completed'] = False
        report['wall_seconds'] = round(time.monotonic() - started, 3)
        if root:
            out_dir = root / 'out'
            out_dir.mkdir(parents=True, exist_ok=True)
            stamp = time.strftime('%Y%m%d-%H%M%S')
            (out_dir / ('light-result-' + stamp + '.json')).write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    return 0 if report['completed'] else 1


def self_test():
    import math as _math
    assert SAFE_CODE.fullmatch('challenge_required') and not SAFE_CODE.fullmatch('Raw(Url)')
    good = {'position': 1.0, 'duration': 600, 'decoded': 10, 'dropped': 0, 'video_width': 640,
            'video_height': 360, 'ready_state': 4, 'volume': 1, 'playback_rate': 1,
            'playing': True, 'muted': False, 'default_sink': True}
    assert light_state(good)['playing'] is True
    for bad in (dict(good, playing='yes'), dict(good, position=float('nan')),
                dict(good, muted=True), dict(good, default_sink=False),
                dict(good, video_width=0)):
        try:
            light_state(bad)
        except Blocked:
            pass
        else:
            raise AssertionError('invalid_light_state_accepted')
    assert failure_code({'code': 'challenge_required'}) == 'challenge_required'
    assert failure_code({'code': 'https://secret'}) is None
    assert failure_code({'data': {'code': 'x'}}) is None
    report = initial_light_report()
    assert report['full_playback_acceptance'] is False and report['paired'] is False
    assert report['mode'] == 'local_home_light'
    return {'self_test': True, 'source_only': True, 'native_execution': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--private-root')
    parser.add_argument('--exe')
    parser.add_argument('--cef')
    parser.add_argument('--expect-uid', type=int)
    parser.add_argument('--exe-sha256')
    parser.add_argument('--components')
    parser.add_argument('--allow-local-virtual-display', action='store_true')
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        print(json.dumps(self_test())); raise SystemExit(0)
    missing = [key for key in ('private_root', 'exe', 'cef', 'expect_uid', 'exe_sha256', 'components')
               if getattr(args, key) in (None, '')]
    if missing:
        parser.error('Required arguments missing: ' + ', '.join('--' + key.replace('_', '-') for key in missing))
    raise SystemExit(run(args))
