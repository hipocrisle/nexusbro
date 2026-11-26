#!/usr/bin/env python3
"""GHA-Windows-only native downloads open/folder test (PREPARATION — not run here).

Real CEF download of a safe .txt attachment from an own loopback origin, the
library opened through the real Ctrl+J, a physical click on the exact note.txt
record's Open-file button, and proof of the editor process by the EXACT private
file path in its command line + PID birth after the click + known executable
name. Foreign Notepad/Explorer processes are never touched; only the exact
self-identified own-born process may be closed during cleanup. The folder
action is proven through UI Automation of the Explorer address if available,
otherwise honestly reported unclosed — never faked. No association, registry
or environment-file changes to force a PASS.
"""
import argparse
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from security_native import require, wait_for
from download_fixture_target import fixture_target
from tab_routes import WindowsDevice
from youtube_paired_native import (
    rows, switch_super, click_control, Blocked, super_record, call, close_for_privacy,
)
from youtube_player_native import foreground

TXT_BODY = b'nexus downloads windows fixture\n'


class Fixture:
    def __init__(self):
        self._server = None
        self._thread = None
        self.origin = None

    def start(self):
        outer=self
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                path,_form = fixture_target(self.path,outer.origin)
                if path == '/page.html':
                    body = b'<!doctype html><meta charset="utf-8"><title>dl win fixture</title>page'
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/html; charset=utf-8')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers(); self.wfile.write(body); return
                if path == '/note.txt':
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/plain')
                    self.send_header('Content-Disposition', 'attachment; filename="note.txt"')
                    self.send_header('Content-Length', str(len(TXT_BODY)))
                    self.end_headers(); self.wfile.write(TXT_BODY); return
                self.send_response(404); self.end_headers()

            def log_message(self, *args):
                return

        self._server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.origin = f'http://127.0.0.1:{self._server.server_port}'
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self.origin

    def close(self):
        if self._server is not None:
            self._server.shutdown(); self._server.server_close(); self._server = None


def win_click(client, x, y):
    """One physical click at shell-window CSS coordinates (pyautogui, no JS)."""
    import pyautogui
    label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    from security_native import call as _call
    origin = _call(client, 'plugin:window|inner_position', label=label)
    scale = client.js('devicePixelRatio||1') or 1
    pyautogui.click(round(origin['x'] + x * scale), round(origin['y'] + y * scale))


def processes_snapshot():
    import psutil
    result = {}
    for p in psutil.process_iter(['pid', 'name', 'exe', 'cmdline', 'create_time']):
        try:
            result[p.pid] = {'name': (p.info['name'] or '').lower(), 'exe': p.info['exe'] or '',
                             'cmdline': p.info['cmdline'] or [], 'birth': p.info['create_time']}
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return result


def normalize_win_path(value):
    """Pure normalization for exact comparisons: strip quotes and the \\?\
    verbatim prefix, unify separators, lowercase. No filesystem access."""
    text = str(value).strip().strip('"')
    if text.startswith('\\\\?\\'):
        text = text[4:]
    return text.replace('/', '\\').rstrip('\\').lower()


def argv_has_exact_path(cmdline, target):
    """True only when ONE separate argv element equals the exact target after
    normalization. Substring matches (note.txt.extra) never count."""
    wanted = normalize_win_path(target)
    return any(normalize_win_path(arg) == wanted for arg in cmdline or [])


KNOWN_NOTEPAD_DIRS = ('c:\\windows\\system32\\', 'c:\\windows\\', 'c:\\program files\\windowsapps\\microsoft.windowsnotepad_')


def is_known_notepad(exe_path):
    """Exact known Windows Notepad locations, not any program named notepad."""
    normalized = normalize_win_path(exe_path)
    if not normalized.endswith('\\notepad.exe'):
        return False
    return normalized in ('c:\\windows\\system32\\notepad.exe','c:\\windows\\notepad.exe') or bool(re.fullmatch(r'c:\\program files\\windowsapps\\microsoft[.]windowsnotepad_[^\\]+\\(?:notepad\\)?notepad[.]exe', normalized))


def own_born_editor(before, target_path, started_after):
    """New process born after the click whose SEPARATE argv element equals the
    EXACT private file path and whose executable is a known Windows Notepad.
    Foreign Notepad instances are excluded by the before-snapshot; a reused PID
    fails the birth comparison."""
    import psutil
    now = processes_snapshot()
    for pid, info in now.items():
        if pid in before:
            continue
        if info['birth'] < started_after - 1:
            continue
        if not argv_has_exact_path(info['cmdline'], target_path):
            continue
        if not is_known_notepad(info['exe']):
            continue
        try:
            proc = psutil.Process(pid)
            return {'pid': pid, 'exe': info['exe'], 'birth': info['birth'],
                    'exe_known': True, 'process': proc}
        except psutil.NoSuchProcess:
            continue
    return None


def initial_report():
    return {'schema': 1, 'diagnostic_only': True, 'preparation': True, 'completed': False,
            'open_completed': False, 'folder_completed': False,
            'checks': [], 'stage': 'setup', 'editor_proof': None, 'folder_proof': 'unclosed',
            'full_acceptance': False, 'real_os_associations': 'gha_default_txt_only',
            'cleanup_completed': False}


def run(args):
    report = initial_report(); device = None; root = None; client = None
    fixture = Fixture(); close_attempted = False; editor = None; before_windows = None; download_dir = None
    out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    started = time.monotonic(); work_deadline = started + 240
    def remaining(cap):
        left = work_deadline - time.monotonic()
        if left <= 0: raise Blocked('downloads_win_deadline')
        return min(cap, left)
    try:
        if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'win32':
            raise Blocked('gha_windows_only')
        report['preparation'] = False  # real native run passed preflight
        root = Path(tempfile.mkdtemp(prefix='nb-dlwin-')).resolve(strict=True)
        exe = Path(args.exe).resolve(strict=True); cef = Path(args.cef).resolve(strict=True)
        origin = fixture.start()
        download_dir = root / 'downloads'; download_dir.mkdir()
        os.environ['NEXUSBRO_TEST_DOWNLOAD_DIR'] = str(download_dir)
        device = WindowsDevice(root / 'device', exe, cef)
        identifier = 'dlwin'
        url = origin + '/page.html'
        record = super_record(identifier, 'Downloads win fixture', url, proxy='')
        folder = device.data / 'profili' / identifier; folder.mkdir(parents=True)
        (folder / 'obychnye.json').write_text(json.dumps({'vkladki': [{'id': 1, 'adres': url, 'zagolovok': 'DL win', 'otlozhena': True}], 'aktivnaya': 1, 'sleduyushchaya': 2}))
        for name in ('vkladki.json', 'sohranennye.json'):
            (device.data / name).write_text(json.dumps([record]))
        (device.data / 'nastrojki.json').write_text(json.dumps({'avtoobnovleniya': False, 'apparatnoe_uskorenie': False, 'memory_saver': False}))
        report['stage'] = 'startup'; client = device.start()
        switch_super(client, identifier, timeout=remaining(10))
        if client.js('!!document.querySelector(".pusto button")'):
            click_control(client, '.pusto button', remaining(10))
        report['stage'] = 'tab-live'
        deadline = time.monotonic() + remaining(60); tab = None
        while time.monotonic() < deadline:
            session = rows(client, identifier, timeout=remaining(3)); tab = session.get('aktivnaya')
            if any(v.get('id') == tab and v.get('browser_id', 0) > 0 for v in session.get('vkladki', [])):
                break
            time.sleep(.5)
        else:
            raise Blocked('downloads_tab_not_live')
        report['stage'] = 'download'
        call(client, 'obychnaya_komanda', id=identifier,
             komanda={'deystvie': 'pereyti', 'id': tab, 'adres': origin + '/note.txt'})
        target = download_dir / 'note.txt'
        done = wait_for(lambda: next((d for d in call(client, 'zagruzki_spisok')
                                      if d.get('name') == 'note.txt' and d.get('status') == 'Готово'
                                      and target.is_file()), None),
                        'note.txt did not complete with a real file', remaining(60))
        require(target.read_bytes() == TXT_BODY, 'downloaded bytes mismatch')
        report['checks'].append('real_download_completed')
        report['stage'] = 'library-ui'
        foreground(device)
        import pyautogui
        pyautogui.hotkey('ctrl', 'j')
        wait_for(lambda: client.js('!!document.querySelector(".library-panel .download-row")'),
                 'Downloads library did not open via Ctrl+J', remaining(15))
        require(client.js('!!(document.querySelector(".library-panel")&&document.querySelector(".library-panel").matches(":focus-within"))') is True,
                'Library panel did not take real focus')
        report['checks'].append('library_opened_real_ctrl_j')
        report['stage'] = 'open-file'
        point = client.js('(()=>{const a=Array.from(document.querySelectorAll(".library-panel article")).find(x=>x.textContent.includes("note.txt"));const b=a?.querySelector(".download-actions .download-primary");if(!b)return null;const r=b.getBoundingClientRect();return [Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)]})()')
        require(isinstance(point, list) and len(point) == 2, 'note.txt open button not found')
        before = processes_snapshot(); click_time = time.time()
        client.js('(()=>{const a=Array.from(document.querySelectorAll(".library-panel article")).find(x=>x.textContent.includes("note.txt"));a.querySelector(".download-primary").setAttribute("data-nb-open-target","1");return true})()')
        click_control(client,'[data-nb-open-target="1"]',remaining(10))
        editor = wait_for(lambda: own_born_editor(before, target, click_time),
                          'Own-born editor with the exact private path not observed', remaining(30))
        require(editor['exe_known'], 'Editor executable is not the known notepad.exe')
        report['editor_proof'] = {'pid': editor['pid'], 'exe': Path(editor['exe']).name,
                                  'birth_after_click': editor['birth'] >= click_time - 1,
                                  'exact_private_path_in_cmdline': True}
        report['checks'].append('open_file_own_born_editor_exact_path')
        report['open_completed'] = True
        # Вернуть фокус нашему окну до клика по папке: редактор мог стать
        # foreground, и клик не должен попасть в него. Редактор закрывается
        # здесь же (только own-born, с ожиданием выхода), чтобы не мешать.
        try:
            editor['process'].terminate()
            editor['process'].wait(timeout=10)
        except Exception:
            report['editor_cleanup'] = 'incomplete'
        foreground(device)
        require(client.js('!!document.querySelector(".library-panel")') is True,
                'Library panel lost before folder action')
        report['stage'] = 'folder'
        folder_point = client.js('(()=>{const a=Array.from(document.querySelectorAll(".library-panel article")).find(x=>x.textContent.includes("note.txt"));const b=Array.from(a?.querySelectorAll(".download-actions button")||[]).find(x=>x.textContent.includes("Показать в папке"));if(!b)return null;const r=b.getBoundingClientRect();return [Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)]})()')
        if isinstance(folder_point, list):
            before_windows = window_handles_snapshot()
            client.js('(()=>{const a=Array.from(document.querySelectorAll(".library-panel article")).find(x=>x.textContent.includes("note.txt"));const b=Array.from(a.querySelectorAll("button")).find(b=>b.textContent.includes("Показать в папке"));b.setAttribute("data-nb-folder-target","1");return true})()')
            click_control(client,'[data-nb-folder-target="1"]',remaining(10))
            try:
                def observe_folder():
                    try:return explorer_folder_proof(before_windows, download_dir)
                    except Blocked:return None
                proof = wait_for(observe_folder,
                                 'Explorer folder proof unavailable', remaining(20))
                report['folder_proof'] = proof
                report['folder_completed'] = True
                report['checks'].append('folder_new_window_exact_address')
            except Exception:
                report['folder_proof'] = 'unclosed'
        report['stage'] = 'normal-owned-exit'; close_attempted = True
        close_for_privacy(client, device, report)
        report['completed'] = True
        report['full_acceptance'] = bool(report['open_completed'] and report['folder_completed'])
        report['stage'] = 'complete'
    except Blocked as error:
        report['blocked_reason'] = str(error)
    except Exception:
        report['blocked_reason'] = 'downloads_windows_unavailable'
    finally:
        report['cleanup_components'] = {}
        folder_gone = True
        proof = report.get('folder_proof')
        if isinstance(proof,dict):
            try:
                current = explorer_folder_proof(before_windows,download_dir)
                require(current['hwnd']==proof['hwnd'] and current['pid']==proof['pid'] and current['birth']==proof['birth'], 'Owned Explorer identity changed')
                from pywinauto import Desktop
                window = Desktop(backend='uia').window(handle=proof['hwnd'])
                require(window.process_id()==proof['pid'], 'Owned Explorer PID changed')
                window.close()
                wait_for(lambda:not window.exists(), 'Owned Explorer did not close',10)
            except Exception as error:
                folder_gone = False
                report['cleanup_components']['folder_error_type'] = type(error).__name__
        report['cleanup_components']['folder_closed'] = folder_gone
        editor_gone = True
        if editor and editor.get('process') is not None:
            try:
                # Close ONLY the exact self-identified own-born editor and wait
                # with a birth check; cleanup is not True while it lives.
                editor['process'].terminate()
                editor['process'].wait(timeout=10)
            except Exception as error:
                editor_gone = False
                report['cleanup_components']['editor_error_type'] = type(error).__name__
            try:
                import psutil
                if editor['process'].is_running():
                    editor_gone = False
            except Exception:
                pass
        if client is not None and not close_attempted and device is not None and device.app is not None:
            prior = report['stage']
            try:
                close_attempted = True; close_for_privacy(client, device, report)
            except Exception:
                report['normal_exit_incomplete'] = True
            report['stage'] = prior
        report['cleanup_components']['editor_gone'] = editor_gone
        cleanup = True
        fixture.close()
        if device:
            try:
                device.close()
            except Exception as error:
                cleanup = False
                report['cleanup_components']['device_error_type'] = type(error).__name__
        if root:
            try:
                shutil.rmtree(root)
            except OSError as error:
                cleanup = False
                report['cleanup_components']['root_error'] = {'type':type(error).__name__,'errno':error.errno,'winerror':getattr(error,'winerror',None)}
        os.environ.pop('NEXUSBRO_TEST_DOWNLOAD_DIR', None)
        cleanup = cleanup and editor_gone and folder_gone
        report['cleanup_completed'] = cleanup
        if not cleanup: report['completed'] = False; report['blocked_reason'] = 'cleanup_failed'
        if not report.get('owned_exit', {}).get('clean_shutdown_receipt_verified'): report['completed'] = False
        report['wall_seconds'] = round(time.monotonic() - started, 3)
        (out / 'result.json').write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    return 0 if report['completed'] and report['full_acceptance'] else 1


def window_handles_snapshot():
    """Snapshot of existing Explorer window handles. COM Shell.Application is
    preferred; when COM is unavailable the set is None and only the strict UIA
    fallback can prove a NEW window. Availability is never fabricated."""
    com = None
    try:
        import win32com.client
        shell_windows = win32com.client.Dispatch('Shell.Application').Windows()
        com = set()
        for index in range(shell_windows.Count):
            try:
                com.add(int(shell_windows.Item(index).HWND))
            except Exception:
                continue
    except Exception:
        com = None
    uia = None
    try:
        from pywinauto import Desktop
        windows = Desktop(backend='uia').windows()
        uia = set()
        for win in windows:
            try:
                uia.add(int(win.handle))
            except Exception:
                continue
    except Exception:
        pass
    return {'com': com, 'uia': uia}


def file_url_to_path(url):
    """file:///C:/a%20b -> exact normalized Windows path; None otherwise."""
    import urllib.parse
    if not isinstance(url, str) or not url.lower().startswith('file:'):
        return None
    parsed = urllib.parse.urlparse(url)
    if parsed.netloc.lower() not in ('','localhost') or parsed.query or parsed.fragment:return None
    path = urllib.parse.unquote(parsed.path)
    if len(path) > 2 and path[0] == '/' and path[2] == ':':
        path = path[1:]
    return normalize_win_path(path)


def explorer_hwnd_pid(hwnd):
    """ctypes GetWindowThreadProcessId — the real owner PID of a handle."""
    import ctypes
    from ctypes import wintypes
    pid = wintypes.DWORD()
    function=ctypes.windll.user32.GetWindowThreadProcessId
    function.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.DWORD)];function.restype=wintypes.DWORD
    function(wintypes.HWND(hwnd),ctypes.byref(pid))
    return pid.value


def _process_info(pid):
    import psutil
    try:
        proc = psutil.Process(pid)
        return {'exe': proc.exe(), 'birth': proc.create_time(), 'name': proc.name().lower()}
    except (psutil.NoSuchProcess, psutil.AccessDenied):
        return None


def _known_explorer(info, recheck=None):
    """Owner must be the canonical %WINDIR%\\explorer.exe; the PID birth is
    re-read and must be stable (defeats PID reuse) and not in the future."""
    if not info:
        return False
    windir = normalize_win_path(os.environ.get('WINDIR', 'C:\\Windows'))
    if normalize_win_path(info.get('exe') or '') != windir + '\\explorer.exe':
        return False
    birth = info.get('birth')
    if type(birth) not in (int,float) or not math.isfinite(birth) or birth<=0 or birth > time.time() + 1:
        return False
    if recheck is None or recheck != birth:
        return False
    return True


def explorer_folder_proof(before, folder, *, shell_windows=None, pid_of=None,
                          process_info=None, uia_windows=None):
    """Prove a NEW Explorer WINDOW (handle absent from the pre-click snapshot)
    whose address is EXACTLY our private folder, owned by the real Windows
    explorer.exe with a stable PID birth. Explorer usually opens a new window
    in an EXISTING process, so a fresh PID is never required. COM
    Shell.Application.Windows() LocationURL is the preferred evidence; the UIA
    fallback demands a NEW hwnd + owning PID + exact address value. Substring
    or title-only matches are never acceptance. Foreign Explorer windows are
    never terminated or controlled. Raises Blocked (honest unclosed)."""
    wanted = normalize_win_path(folder)
    pid_of = pid_of or explorer_hwnd_pid
    info_of = process_info or _process_info
    try:
        if before.get('com') is None:raise RuntimeError('COM snapshot unavailable')
        windows = shell_windows
        if windows is None:
            import win32com.client
            windows = win32com.client.Dispatch('Shell.Application').Windows()
        for index in range(windows.Count):
            item = windows.Item(index)
            try:
                hwnd = int(item.HWND)
            except Exception:
                continue
            if hwnd in (before.get('com') or set()):
                continue
            if file_url_to_path(getattr(item, 'LocationURL', None)) != wanted:
                continue
            pid = pid_of(hwnd)
            info = info_of(pid)
            recheck = (info_of(pid) or {}).get('birth')
            if _known_explorer(info, recheck):
                return {'hwnd': hwnd, 'pid': pid, 'birth':info['birth'], 'proof': 'com_locationurl_exact'}
    except Blocked:
        raise
    except Exception:
        pass
    try:
        if before.get('uia') is None:raise RuntimeError('UIA snapshot unavailable')
        if uia_windows is not None:
            candidates = uia_windows
        else:
            from pywinauto import Desktop
            candidates = []
            for win in Desktop(backend='uia').windows():
                try:
                    addresses = []
                    for control in win.descendants(control_type='Edit') + win.descendants(control_type='ToolBar'):
                        try:
                            addresses.append(control.window_text() or '')
                        except Exception:
                            continue
                    candidates.append({'hwnd': int(win.handle), 'pid': win.process_id(),
                                       'addresses': addresses})
                except Exception:
                    continue
        for candidate in candidates:
            hwnd = candidate.get('hwnd')
            if hwnd is None or hwnd in before.get('uia', set()):
                continue
            pid = candidate.get('pid')
            info = info_of(pid)
            recheck = (info_of(pid) or {}).get('birth')
            if not _known_explorer(info, recheck):
                continue
            for text in candidate.get('addresses') or []:
                cleaned = str(text)
                for prefix in ('Address: ', 'Адрес: '):
                    if cleaned.startswith(prefix):
                        cleaned = cleaned[len(prefix):]
                if normalize_win_path(cleaned) == wanted:
                    return {'hwnd': hwnd, 'pid': pid, 'birth':info['birth'], 'proof': 'uia_new_hwnd_exact'}
    except Blocked:
        raise
    except Exception:
        pass
    raise Blocked('explorer_folder_proof_unavailable')


def self_test():
    fixture = Fixture(); origin = fixture.start(); fixture.close()
    assert origin.startswith('http://127.0.0.1:')
    report = initial_report()
    assert report['preparation'] is True and report['folder_proof'] == 'unclosed'
    assert TXT_BODY.endswith(b'\n')
    return {'self_test': True, 'source_only': True, 'native_execution': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('exe', 'cef', 'out'):
        parser.add_argument('--' + key)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        print(json.dumps(self_test())); raise SystemExit(0)
    if not all(getattr(args, key) for key in ('exe', 'cef', 'out')):
        parser.error('Required arguments missing: --exe --cef --out')
    raise SystemExit(run(args))
