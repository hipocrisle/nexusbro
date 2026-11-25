#!/usr/bin/env python3
"""Local Home consumer entry for the official YouTube embed feasibility probe.

Diagnostic only, never playback acceptance. Runs the SAME fixture as the CI
probe (tests/gui/youtube_embed_reference_native.py) on a local Linux desktop
of the owner WITHOUT GHA: no GHA=true spoofing, no shared run()/guards
changes. The GHA shutdown receipt does not exist locally, so this entry
reports receipt_unavailable_in_local_mode=true and never claims
clean_shutdown_receipt_verified; the normal owned exit is proven with the
real ExitWitness/native identity checks instead. full_playback_acceptance and
paired stay false in every outcome.

Sandbox: the shared GHA Device forces WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
(security_native.Device.start). Local mode must NOT inherit that GHA
accommodation: SandboxedDevice below is a faithful copy of that start() minus
the sandbox-disable variable. If the app cannot start under its normal
sandbox, the run is blocked with 'sandbox_startup_blocked' — it never retries
with the sandbox disabled and never weakens CEF/WebKit sandboxes.

Hard guards before anything else:
- real non-root uid == euid == --expect-uid;
- --private-root strict inside ~/AI, owned by the expected uid, mode 0700, no symlinks;
- --exe/--cef resolved strictly inside the private root; exact --exe-sha256;
- PATH preflight for the same owned-fixture programs as CI (fixed codes);
- own Xvfb via displayfd + own openbox only (--allow-local-virtual-display),
  never the owner DISPLAY/GPU; hardware acceleration stays off.
"""
import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from security_native import require, wait_for
from youtube_embed_reference_native import (
    Fixture, summarize, initial_report as ci_initial_report,
    PREFLIGHT_COMMANDS, VIDEO,
    BUTTON_LEFT, BUTTON_TOP, BUTTON_WIDTH, BUTTON_HEIGHT, CLICK_OFFSET, YELLOW,
)
from youtube_paired_native import (
    Device, OwnedWindowManager, rows, switch_super, click_control, Blocked,
    foreground, super_record,
)
from youtube_player_native import (
    native_app_identity, native_app_identity_alive, native_app_window,
    native_app_execution_alive, ExitWitness,
)

LOCAL_STAGES = ('preflight', 'private-root', 'startup', 'owned-window-manager',
                'tab-live', 'official-api-ready', 'os-paint', 'trusted-play-click',
                'observation', 'local-normal-owned-exit')
WORK_BUDGET_SECONDS = 180
CLEANUP_BUDGET_SECONDS = 90
MIN_WALL_SECONDS = 8
HOME_SUBDIR = 'AI'


def validate_private_root(raw_root, expected_uid):
    """~/AI-only, owner uid, 0700, no symlinks; strict containment."""
    home = Path.home().resolve(strict=True)
    ai = (home / HOME_SUBDIR).resolve(strict=True)
    try:
        root = Path(raw_root).resolve(strict=True)  # strict: full lstat chain
    except OSError:
        raise Blocked('local_root_unresolvable')
    if root != ai and ai not in root.parents:
        raise Blocked('local_root_outside_ai')
    st = os.lstat(root)
    if not stat.S_ISDIR(st.st_mode):
        raise Blocked('local_root_not_a_real_directory')
    if st.st_uid != expected_uid:
        raise Blocked('local_root_owner_mismatch')
    if stat.S_IMODE(st.st_mode) & 0o077:
        raise Blocked('local_root_permissions_not_0700')
    return root


def validate_inside_root(path, root, name):
    try:
        value = Path(path).resolve(strict=True)
    except OSError:
        raise Blocked('local_' + name + '_unresolvable')
    if value != root and root not in value.parents:
        raise Blocked('local_' + name + '_outside_root')
    return value


class SandboxedDevice(Device):
    """Local Device: same owned Xvfb/inspector startup, WITHOUT the GHA-only
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS accommodation of the shared
    Device.start (security_native.py). If normal sandboxed startup fails, the
    caller reports 'sandbox_startup_blocked'; there is deliberately no
    fallback that weakens the CEF/WebKit sandbox."""

    def start(self):  # noqa: C901 - mirrors Device.start structure on purpose
        import websocket
        displayfd_dir = self.root
        displayfd_dir.mkdir(parents=True, exist_ok=True)
        read_fd, write_fd = os.pipe()
        os.set_blocking(read_fd, False)
        try:
            self.x = subprocess.Popen(
                ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1280x900x24', '-nolisten', 'tcp', '-noreset'],
                pass_fds=(write_fd,), stdout=self.log('xvfb.log'),
                stderr=subprocess.STDOUT, start_new_session=True)
            os.close(write_fd)
            write_fd = None
            ready = False
            deadline = time.monotonic() + 25
            while time.monotonic() < deadline:
                try:
                    data = os.read(read_fd, 16)
                except BlockingIOError:
                    data = b''
                if data.strip().isdigit():
                    ready = True
                    break
                if self.x.poll() is not None:
                    break
                time.sleep(.05)
            require(ready, 'Local sandboxed Xvfb did not allocate a display')
            self.display = ':' + data.strip().decode()
            try:
                ready = False
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    require(self.x.poll() is None, 'Local sandboxed Xvfb exited before GTK launch')
                    probe = subprocess.run(['xdotool', 'getdisplaygeometry'],
                                           env={**os.environ, 'DISPLAY': self.display},
                                           capture_output=True, text=True, timeout=2)
                    if probe.returncode == 0 and probe.stdout.split() == ['1280', '900']:
                        ready = True
                        break
                    time.sleep(.05)
                require(ready, 'Local sandboxed Xvfb did not accept a real X11 client')
            finally:
                os.close(read_fd)
                if write_fd is not None:
                    os.close(write_fd)
        except BaseException:
            try:
                os.close(read_fd)
            except OSError:
                pass
            if write_fd is not None:
                try:
                    os.close(write_fd)
                except OSError:
                    pass
            raise
        self.launches += 1
        # Same environment as the shared Device.start EXCEPT the GHA-only
        # WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 override, which must not
        # leak into a local owner run.
        env = {**os.environ, 'DISPLAY': self.display, 'CEF_PATH': str(self.cef),
               'LD_LIBRARY_PATH': os.pathsep.join((str(self.cef), str(self.cef.parent / 'tools/usr/lib/x86_64-linux-gnu'), str(self.cef.parent / 'tools/usr/lib/x86_64-linux-gnu/openbox'))), 'NEXUSBRO_TEST_DATA': str(self.data),
               'NEXUSBRO_PROBA': '0',
               'WEBKIT_INSPECTOR_HTTP_SERVER': '127.0.0.1:' + str(self.inspector),
               'XDG_CONFIG_HOME': str(self.root / 'config'),
               'XDG_DATA_HOME': str(self.root / 'xdg-data'),
               'XDG_CACHE_HOME': str(self.root / 'cache'),
               'NO_PROXY': '127.0.0.1,localhost,::1', 'no_proxy': '127.0.0.1,localhost,::1'}
        self.app = subprocess.Popen(['dbus-run-session', '--', str(self.exe)], env=env,
                                    stdout=self.log('app-' + str(self.launches) + '.log'),
                                    stderr=subprocess.STDOUT, start_new_session=True)
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            require(self.app.poll() is None, 'Sandboxed native application exited during startup')
            try:
                from client import Client
                self.client = Client(display=self.display, port=self.inspector)
                break
            except (ConnectionError, OSError, websocket.WebSocketException):
                time.sleep(.2)
        if self.client is None:
            raise Blocked('sandbox_startup_blocked')
        wait_for(lambda: self.client.js('!!window.__TAURI_INTERNALS__?.invoke'),
                 'Tauri IPC did not initialize under the preserved sandbox', 20)
        return self.client


class LocalWindowManager(OwnedWindowManager):
    def __init__(self, device, root):
        require(device.x.poll() is None, 'Owned Xvfb required')
        self.device = device
        self.env = {**os.environ, 'DISPLAY': device.display,
                    'HOME': str(root), 'XDG_CONFIG_HOME': str(root / 'config'),
                    'XDG_CACHE_HOME': str(root / 'cache'),
                    'XDG_DATA_DIRS': str(device.cef.parent / 'tools/usr/share') + ':/usr/local/share:/usr/share'}
        root.mkdir(parents=True)
        config = root / 'rc.xml'
        config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><focus><focusNew>yes</focusNew></focus></openbox_config>')
        with (root / 'openbox.log').open('wb') as log:
            self.process = subprocess.Popen(['openbox', '--config-file', str(config)], env=self.env,
                                            stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        def ready():
            require(self.process.poll() is None, 'Owned local window manager exited')
            result = subprocess.run(['xprop', '-root', '_NET_SUPPORTING_WM_CHECK'], env=self.env,
                                    capture_output=True, text=True, timeout=3)
            return result.returncode == 0 and 'window id #' in result.stdout
        try:
            wait_for(ready, 'Owned local window manager not ready', 15)
        except BaseException:
            self.close()
            raise


def local_close(client, device, report):
    """OS-normal owned exit with real identity/witness facts; the GHA shutdown
    receipt is unavailable locally and is never claimed."""
    report['stage'] = 'local-normal-owned-exit'
    identity = native_app_identity(device)
    witness = ExitWitness(device, client)
    parent_pid = identity['pid']
    with witness.lock:
        witness.identities[parent_pid] = identity['birth']
    try:
        require(native_app_identity_alive(device, identity), 'Local native app identity invalid')
        foreground(device)
        require(native_app_identity_alive(device, identity), 'Local native app identity invalid')
        owner = native_app_window(device, identity)
        subprocess.run(['wmctrl', '-ic', str(owner)],
                       env=device.player_wm.env, check=True, timeout=5)
        exit_code = device.app.wait(timeout=30)
        require(exit_code == 0, 'Local parent did not exit normally')
        wait_for(witness.all_gone, 'Local owned native processes did not exit', 30)
        witness.close()
        require(witness.all_gone(), 'Local owned native processes did not exit')
        require(not native_app_execution_alive(identity), 'Local app execution remained alive')
        report['owned_exit'] = {'parent_normal_exit': True, 'native_members_observed': witness.native_members,
                                'os_identities_observed': len(witness.identities),
                                'all_known_native_executions_exited': True,
                                'native_members_complete': True,
                                'scope': 'bounded_owned_fixture_not_universal_noescape_proof',
                                'harness_kill_used_for_acceptance': False,
                                'clean_shutdown_receipt_verified': False,
                                'receipt_unavailable_in_local_mode': True,
                                'actual_app_identity_bound': True,
                                'launcher_is_actual_app': identity['pid'] == identity['wrapper_pid'],
                                'actual_app_execution_exited': True, 'launcher_exit_zero': True}
        device.app = None
    finally:
        witness.close()


def initial_local_report():
    report = ci_initial_report()
    report.update({'mode': 'local_home', 'full_playback_acceptance': False, 'paired': False,
                   'receipt_unavailable_in_local_mode': True,
                   'sandbox': 'preserved_no_gha_disable',
                   'budgets': {'work_seconds': WORK_BUDGET_SECONDS, 'cleanup_seconds': CLEANUP_BUDGET_SECONDS},
                   'allowed_stages': list(LOCAL_STAGES), 'stage': 'setup'})
    return report


def run(args):
    report = initial_local_report(); device = wm = None; root = None; client = None
    fixture = Fixture(args.video_id or VIDEO); close_attempted = False
    started = time.monotonic()
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
        out = root / 'out' / ('embed-home-' + str(int(time.time())))
        out.mkdir(parents=True, exist_ok=False)
        report['private_root_inside_ai'] = True
        work_deadline = started + WORK_BUDGET_SECONDS
        def remaining(cap):
            left = work_deadline - time.monotonic()
            if left <= 0:
                raise Blocked('embed_work_deadline')
            return min(cap, left)
        url = fixture.start()
        report['stage'] = 'startup'
        device = SandboxedDevice('embed-home', out / 'device', exe, cef)
        identifier = 'embed-home'
        record = super_record(identifier, 'Official embed local reference', url, proxy='')
        folder = device.data / 'profili' / identifier; folder.mkdir(parents=True)
        (folder / 'obychnye.json').write_text(json.dumps({'vkladki': [{'id': 1, 'adres': url, 'zagolovok': 'Embed local', 'otlozhena': True}], 'aktivnaya': 1, 'sleduyushchaya': 2}))
        for name in ('vkladki.json', 'sohranennye.json'):
            (device.data / name).write_text(json.dumps([record]))
        (device.data / 'nastrojki.json').write_text(json.dumps({'avtoobnovleniya': False, 'apparatnoe_uskorenie': False, 'memory_saver': False}))
        client = device.start()
        if time.monotonic() - started > 45:
            raise Blocked('embed_startup_deadline')
        report['stage'] = 'owned-window-manager'
        wm = LocalWindowManager(device, out / 'wm'); device.player_wm = wm
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
            raise Blocked('embed_tab_not_live')
        report['stage'] = 'official-api-ready'
        deadline = time.monotonic() + remaining(60); first_time = None
        while time.monotonic() < deadline:
            receipt = fixture.summary()
            if receipt is not None:
                summary = summarize(receipt)
                if summary['ready']:
                    first_time = summary['api_clock_advanced_seconds']; break
            time.sleep(.5)
        else:
            raise Blocked('embed_api_not_ready')
        point = None
        deadline = time.monotonic() + remaining(20)
        while time.monotonic() < deadline:
            value = client.js('(()=>{const e=document.querySelector(".pole");if(!e)return null;const r=e.getBoundingClientRect();return r.width>300&&r.height>200?[Math.round(r.x+30),Math.round(r.y+30)]:null})()')
            if value:
                point = tuple(value); break
            time.sleep(.25)
        if point is None:
            raise Blocked('embed_pole_unavailable')
        foreground(device)
        report['stage'] = 'os-paint'
        from youtube_embed_reference_native import yellow_observed
        deadline = time.monotonic() + remaining(30)
        while time.monotonic() < deadline:
            try:
                if yellow_observed(device, client, point):
                    report['os_paint_observed'] = True; break
            except Exception:
                pass
            time.sleep(.4)
        if not report['os_paint_observed']:
            raise Blocked('embed_button_not_painted')
        report['stage'] = 'trusted-play-click'
        client.mouse(*point)  # exactly one physical click on the yellow button
        report['stage'] = 'observation'
        deadline = time.monotonic() + remaining(120); summary = None
        while time.monotonic() < deadline:
            receipt = fixture.summary()
            if receipt is not None:
                summary = summarize(receipt, first_time)
                for key in ('ready', 'played', 'states_seen', 'errors_seen', 'api_clock_advanced_seconds', 'playing_observed'):
                    report[key] = summary[key]
                if summary['errors_seen']:
                    raise Blocked('embed_error_' + '_'.join(str(e) for e in summary['errors_seen']))
                wall = time.monotonic() - started
                if summary['playing_observed'] and wall >= MIN_WALL_SECONDS:
                    break
            time.sleep(.5)
        if summary is None:
            raise Blocked('embed_receipt_unavailable')
        report['trusted_play_click'] = summary['played']
        if not summary['played']:
            raise Blocked('trusted_click_missing')
        if not summary['playing_observed']:
            raise Blocked('embed_playing_not_observed')
        report['stage'] = 'local-normal-owned-exit'; close_attempted = True
        local_close(client, device, report)
        report['completed'] = True; report['stage'] = 'complete'
    except Blocked as error:
        report['blocked_reason'] = str(error)
    except Exception as error:
        report['blocked_reason'] = 'embed_home_probe_unavailable'
        report['exception_type'] = type(error).__name__
        if root is not None:
            import traceback
            (root / 'private-exception.txt').write_text(traceback.format_exc())
    finally:
        if client is not None and not close_attempted and device is not None and device.app is not None:
            prior = report['stage']
            try:
                close_attempted = True; local_close(client, device, report)
            except Exception:
                report['normal_exit_incomplete'] = True
            report['stage'] = prior
        cleanup = True
        fixture.close()
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
        out_dir = root / 'out' if root else None
        if out_dir:
            out_dir.mkdir(parents=True, exist_ok=True)
            stamp = time.strftime('%Y%m%d-%H%M%S')
            (out_dir / ('result-' + stamp + '.json')).write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    return 0 if report['completed'] else 1



def self_test():
    import inspect
    source = inspect.getsource(SandboxedDevice.start)
    assert "'WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS': '1'" not in source, \
        'local mode must not inherit the GHA sandbox disable env assignment'
    assert 'NEXUSBRO_TEST_DATA' in source and 'dbus-run-session' in source
    close_src = inspect.getsource(local_close)
    assert "'clean_shutdown_receipt_verified': False" in close_src
    assert 'receipt_unavailable_in_local_mode' in close_src and 'read_shutdown_receipt' not in close_src
    report = initial_local_report()
    assert report['receipt_unavailable_in_local_mode'] is True and report['full_playback_acceptance'] is False
    assert report['paired'] is False and report['sandbox'] == 'preserved_no_gha_disable'
    import re as _re
    assert _re.fullmatch(r'[0-9a-f]{64}', 'ab' * 32)
    return {'self_test': True, 'source_only': True, 'native_execution': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--private-root')
    parser.add_argument('--exe')
    parser.add_argument('--cef')
    parser.add_argument('--expect-uid', type=int)
    parser.add_argument('--exe-sha256')
    parser.add_argument('--allow-local-virtual-display', action='store_true')
    parser.add_argument('--video-id', default=None)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        print(json.dumps(self_test())); raise SystemExit(0)
    missing = [key for key in ('private_root', 'exe', 'cef', 'expect_uid', 'exe_sha256')
               if getattr(args, key) in (None, '')]
    if missing:
        parser.error('Required arguments missing: ' + ', '.join('--' + key.replace('_', '-') for key in missing))
    raise SystemExit(run(args))
