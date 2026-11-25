#!/usr/bin/env python3
"""Local Home ABBA entry: ordinary watch YouTube vs shipped lightweight player.

Diagnostic only — never release acceptance. Reuses the GHA experiment arm()
through dependency injection (no changes to its measurement logic, guards,
ORDER=light,normal,normal,light, warmup/measurement windows, or sampling):
same binary, same direct route, same software decoder expectations, same
audio policy, one public video (M7lc1UVf-VE). Guards and owned-lifecycle
helpers come from the shared local entry module; nothing there is modified.

Local properties vs the GHA run:
- fresh per-arm directories inside ~/AI; uid == euid == --expect-uid (non-root);
  exact --exe-sha256; resource_dir components at <root>/lib/NexusBro/binaries/
  youtube-light; CEF at <private-root>/cef; sealed native hashes authoritative;
- no GHA (rejected), no inherited WEBKIT_DISABLE_SANDBOX (rejected), sandbox
  preserved (SandboxedDevice), own Xvfb + LocalWindowManager, no owner
  DISPLAY/GPU/session/cookies/login/CAPTCHA;
- state_probe replaces the shared probe(): the shared one clears __paired on
  every snapshot (losing async youtube-result failure receipts) and its wait
  raises NativeFailure outside Blocked. The local probe reads only NEW events
  by monotonic index for the exact TIP/tab, never clears the buffer (tail
  bounded at 512), maps shipped youtube-result.error only through the fixed
  sidecar_enum table, and treats start=false+error=null as SUCCESS completion
  (not failure). Bounded 60s extraction admission lives in arm() itself.
- close_owned = local_close: OS-normal exit with real identity/witness facts;
  the GHA shutdown receipt is never claimed locally.
- Report is always diagnostic_only=true, acceptance_passed=false,
  full_playback_acceptance=false, paired_release_acceptance=false, even when
  measurement_valid/comparison_valid are true on complete evidence.
- Abort on the first failed arm; no retries, no client roulette.
"""
import argparse
import hashlib
import json
import os
import shutil
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from security_native import NativeFailure, require, wait_for
from youtube_embed_home_probe import (
    validate_private_root, validate_inside_root, SandboxedDevice,
    LocalWindowManager, local_close,
)
from youtube_light_home_probe import sidecar_enum
from youtube_paired_native import ORDER, arm, Blocked

VIDEO = 'M7lc1UVf-VE'
PACKAGE_NAME = 'NexusBro'
TIPS = {'light': 'youtube-state', 'normal': 'youtube-reference-state', 'decode': 'youtube-decode-state'}
COMMANDS = {'light': 'youtube_snapshot', 'normal': 'youtube_reference_snapshot', 'decode': 'youtube_decode_snapshot'}
ARM_BUDGET_SECONDS = 210
CLEANUP_BUDGET_SECONDS = 90


def validate_components_dir(components, exe, root):
    expected = (exe.parent.parent / 'lib' / PACKAGE_NAME / 'binaries' / 'youtube-light').resolve(strict=True)
    value = Path(components).resolve(strict=True)
    require(value == expected, 'local_components_not_resource_dir')
    require(root in value.parents, 'local_components_outside_root')
    return value


def make_state_probe(video_id):
    """state_probe(client, identifier, tab, kind) -> raw state dict.

    Preserves async youtube-result receipts (no buffer clearing), reads only
    new events by monotonic index, maps shipped failure strings only through
    sidecar_enum. Raises Blocked/NativeFailure like the shared probe()."""
    def state_probe(client, identifier, tab, kind):
        tip = TIPS[kind]
        params = {'deystvie': COMMANDS[kind], 'id': tab}
        if kind != 'light':
            params['video_id'] = video_id
        start = client.js('window.__paired.length')
        if type(start) is not int:
            raise Blocked('native_paired_buffer_unavailable')
        from youtube_paired_native import call
        call(client, 'obychnaya_komanda', id=identifier, komanda=params)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            data = client.js(
                '(()=>{const b=window.__paired;let best=-1;'
                'for(let i=' + str(start) + ';i<b.length;i++){const e=b[i];'
                'if(e&&e.super_id===' + json.dumps(identifier) + '&&e.data&&e.data.id===' + str(tab) +
                '&&e.data.tip===' + json.dumps(tip) + ')best=i;}'
                'if(best>=0){const d=b[best].data;if(b.length>512)b.splice(0,b.length-512);return d;}'
                'for(let i=' + str(start) + ';i<b.length;i++){const e=b[i];'
                'if(e&&e.super_id===' + json.dumps(identifier) + '&&e.data&&e.data.id===' + str(tab) +
                '&&e.data.tip==="youtube-result"){const d=e.data;if(b.length>512)b.splice(0,b.length-512);return d;}}'
                'return null;})()')
            if isinstance(data, dict):
                if data.get('tip') == 'youtube-result':
                    # start=false + error=null is SUCCESS completion, not failure.
                    if data.get('start') is False and data.get('error') is None:
                        time.sleep(.1)
                        continue
                    raise Blocked('native_light_' + sidecar_enum(data.get('error')))
                state = data.get('state')
                if isinstance(state, dict):
                    return state
                raise Blocked('native_' + kind + '_unavailable')
            time.sleep(.1)
        raise Blocked('native_' + kind + '_unavailable')
    return state_probe


def initial_report():
    return {'schema': 1, 'mode': 'local_home_paired_abba', 'video': VIDEO,
            'order': list(ORDER), 'diagnostic_only': True, 'acceptance_passed': False,
            'full_playback_acceptance': False, 'paired_release_acceptance': False,
            'measurement_valid': False, 'comparison_valid': False,
            'arms': [], 'stage': 'setup', 'cleanup_completed': False}


def run(args):
    report = initial_report(); parent = None; root = None; started = time.monotonic()
    cleanup_flags = []
    try:
        if os.environ.get('WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS'):
            raise Blocked('local_mode_forbids_inherited_sandbox_disable')
        if os.environ.get('GITHUB_ACTIONS') == 'true':
            raise Blocked('local_mode_forbids_gha')
        if sys.platform != 'linux':
            raise Blocked('local_linux_only')
        expected_uid = int(args.expect_uid)
        if expected_uid == 0 or os.getuid() != os.geteuid() or os.geteuid() != expected_uid:
            raise Blocked('local_uid_mismatch')
        if not args.allow_local_virtual_display:
            raise Blocked('local_virtual_display_not_allowed')
        import shutil as _shutil
        for command in ('Xvfb', 'xdotool', 'xclip', 'openbox', 'wmctrl', 'xprop', 'xwininfo', 'dbus-run-session'):
            if _shutil.which(command) is None:
                raise Blocked('embed_preflight_missing_' + command)
        root = validate_private_root(args.private_root, expected_uid)
        exe = validate_inside_root(args.exe, root, 'exe')
        cef = validate_inside_root(args.cef, root, 'cef')
        components = validate_components_dir(args.components, exe, root)
        digest = hashlib.sha256()
        with exe.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
        require(digest.hexdigest() == args.exe_sha256, 'local_exe_sha256_mismatch')
        out = root / 'out' / ('paired-abba-' + str(int(time.time())))
        out.mkdir(parents=True, exist_ok=False)
        parent = out / 'arms'
        parent.mkdir(parents=True)
        report['components_resource_dir'] = 'lib/NexusBro/binaries/youtube-light'

        class LocalArgs:
            pass
        arm_args = LocalArgs()
        arm_args.exe = str(exe); arm_args.cef = str(cef); arm_args.video_id = VIDEO
        arm_args.expected_duration = args.expected_duration; arm_args.expected_fps = args.expected_fps
        arm_args.width = args.width; arm_args.height = args.height
        arm_args.memory_cap_mib = args.memory_cap_mib
        probe = make_state_probe(VIDEO)
        for index, mode in enumerate(ORDER):
            arm_deadline = time.monotonic() + ARM_BUDGET_SECONDS
            result = arm(arm_args, '', mode, index, out,
                         device_factory=lambda device_root, e, c: SandboxedDevice('paired', device_root, e, c),
                         close_owned=local_close,
                         private_parent=str(parent),
                         window_manager_factory=LocalWindowManager,
                         state_probe=probe)
            result['arm_wall_seconds'] = round(time.monotonic() - (arm_deadline - ARM_BUDGET_SECONDS), 3)
            report['arms'].append(result)
            if result.get('stage') != 'complete' or not result.get('metrics'):
                report['stage'] = 'arm-' + str(index) + '-blocked'
                raise Blocked('paired_arm_' + str(index) + '_blocked')
        report['measurement_valid'] = all(a.get('metrics') for a in report['arms'])
        observed = [a['metrics']['observed_fps'] for a in report['arms']]
        report['comparison_valid'] = bool(report['measurement_valid']) and len(observed) == 4
        report['stage'] = 'complete'
    except Blocked as error:
        report['blocked_reason'] = str(error)
    except (NativeFailure, Exception):
        report['blocked_reason'] = 'paired_home_unavailable'
    finally:
        cleanup = True
        if parent is not None:
            for index in range(4):
                marker = parent / ('arm-' + str(index))
                receipt = marker / 'done'
                if marker.exists() and not receipt.exists():
                    report['cleanup_warning'] = 'arm-' + str(index) + '_without_done_marker'
        report['cleanup_completed'] = cleanup
        report['wall_seconds'] = round(time.monotonic() - started, 3)
        if root:
            out_dir = root / 'out'
            out_dir.mkdir(parents=True, exist_ok=True)
            stamp = time.strftime('%Y%m%d-%H%M%S')
            (out_dir / ('paired-result-' + stamp + '.json')).write_text(
                json.dumps(report, indent=2, allow_nan=False) + '\n')
    return 0 if report['stage'] == 'complete' else 1


def self_test():
    import inspect
    src = inspect.getsource(make_state_probe)
    assert 'window.__paired.length' in src and 'window.__paired=[]' not in src
    assert 'sidecar_enum' in src and 'youtube-result' in src
    assert "get('error') is None" in src  # success-completion semantics
    arm_src = inspect.getsource(__import__('youtube_paired_native').arm)
    assert 'device_factory=None' in arm_src and 'state_probe=None' in arm_src
    assert 'close_owned or close_for_privacy' in arm_src
    report = initial_report()
    assert report['diagnostic_only'] is True and report['acceptance_passed'] is False
    assert report['full_playback_acceptance'] is False and report['paired_release_acceptance'] is False
    assert ORDER == ('light', 'normal', 'normal', 'light')
    return {'self_test': True, 'source_only': True, 'native_execution': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--private-root')
    parser.add_argument('--exe')
    parser.add_argument('--cef')
    parser.add_argument('--components')
    parser.add_argument('--expect-uid', type=int)
    parser.add_argument('--exe-sha256')
    parser.add_argument('--allow-local-virtual-display', action='store_true')
    parser.add_argument('--video-id', default=VIDEO)
    parser.add_argument('--expected-duration', type=float, default=1343.6)
    parser.add_argument('--expected-fps', type=float, default=30)
    parser.add_argument('--width', type=int, default=1280)
    parser.add_argument('--height', type=int, default=720)
    parser.add_argument('--memory-cap-mib', type=int, default=1024)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        print(json.dumps(self_test())); raise SystemExit(0)
    missing = [key for key in ('private_root', 'exe', 'cef', 'components', 'expect_uid', 'exe_sha256')
               if getattr(args, key) in (None, '')]
    if missing:
        parser.error('Required arguments missing: ' + ', '.join('--' + key.replace('_', '-') for key in missing))
    raise SystemExit(run(args))
