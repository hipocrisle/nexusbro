#!/usr/bin/env python3
"""GHA-only native downloads library test (PREPARATION — not executed here).

Real CEF downloads against an own loopback HTTP fixture with slow chunked
attachments: known-length (real percent) and unknown-length (indeterminate).
No synthetic download state, no faked SUCCESS. The library opens through the
real Ctrl+J shortcut; the Open-file button is exercised only against a safe
.txt fixture through a test-local `xdg-open` shim placed first in PATH for the
spawned app process (records the argument, launches nothing). Production code
has no test bypass; the shim is purely process-environment of this fixture.

Unclosed runtime criteria handed to ROOT: real OS file associations and the
Windows explorer.exe path cannot be proven on the GHA runner by this test.
"""
import argparse
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from security_native import require, wait_for
from youtube_paired_native import (
    Device, OwnedWindowManager, rows, switch_super, click_control, Blocked,
    foreground, super_record, call, close_for_privacy,
)

CHUNK = 64 * 1024
CHUNKS = 12
CHUNK_DELAY = 1.0
KNOWN_TOTAL = CHUNK * CHUNKS


class Fixture:
    """Own loopback origin: page + two slow attachment downloads."""

    def __init__(self):
        self._server = None
        self._thread = None
        self.origin = None

    def start(self):
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                path = self.path.split('?', 1)[0]
                if path == '/page.html':
                    body = (b'<!doctype html><meta charset="utf-8"><title>dl fixture</title>'
                            b'<a id="known" href="/file-known.bin">known</a>'
                            b'<a id="unknown" href="/file-unknown.bin">unknown</a>'
                            b'<a id="doc" href="/note.txt">doc</a>')
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/html; charset=utf-8')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers(); self.wfile.write(body); return
                if path in ('/file-known.bin', '/file-unknown.bin', '/note.txt'):
                    name = path.rsplit('/', 1)[1]
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/octet-stream' if name.endswith('.bin') else 'text/plain')
                    self.send_header('Content-Disposition', f'attachment; filename="{name}"')
                    if name == 'file-known.bin':
                        self.send_header('Content-Length', str(KNOWN_TOTAL))
                    self.end_headers()
                    total = CHUNKS if name != 'note.txt' else 1
                    try:
                        for _ in range(total):
                            self.wfile.write(b'x' * (CHUNK if name != 'note.txt' else 5))
                            self.wfile.flush()
                            time.sleep(CHUNK_DELAY)
                    except (BrokenPipeError, ConnectionResetError):
                        pass
                    return
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


def make_opener_shim(root):
    """Test-local xdg-open: appends argv to a receipt file, launches nothing."""
    shim_dir = root / 'shim-bin'; shim_dir.mkdir(parents=True)
    receipt = root / 'opener-receipt.jsonl'
    script = shim_dir / 'xdg-open'
    script.write_text('#!/bin/sh\nprintf \'{"arg":"%s"}\\n\' "$1" >> ' + str(receipt) + '\n')
    script.chmod(script.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return shim_dir, receipt


def downloads(client, super_id):
    return call(client, 'zagruzki_spisok')


STATUS_ENUM = ('Загружается', 'Приостановлена', 'Готово', 'Отменена', 'Ошибка', 'Прервана')
FILE_KINDS = {'file-known.bin': 'known', 'file-unknown.bin': 'unknown', 'note.txt': 'doc'}
WAIT_MESSAGES = {
    'Known-length download did not report real progress': 'known_progress_timeout',
    'Unknown-length download did not report unknown total': 'unknown_total_timeout',
    'Received bytes did not advance': 'received_stall_timeout',
    'note.txt did not complete with a real file': 'doc_complete_timeout',
    'file-known.bin did not complete with a real file': 'known_complete_timeout',
    'file-unknown.bin did not complete with a real file': 'unknown_complete_timeout',
    'Downloads library did not open via Ctrl+J': 'library_open_timeout',
}


def bounded_scalar(value):
    if type(value) in (int, float) and not isinstance(value, bool) and value == value and 0 <= value <= 10 ** 12:
        return value
    return None


def download_summary(client):
    """Санитизированный снимок записей: file_kind/status из allowlist и
    ограниченные скаляры. Никаких сырых url/path/id/name."""
    try:
        records = call(client, 'zagruzki_spisok')
    except Exception:
        return {'available': False}
    summary = []
    for record in records if isinstance(records, list) else []:
        if not isinstance(record, dict):
            continue
        summary.append({
            'file_kind': FILE_KINDS.get(record.get('name'), 'other'),
            'status': record.get('status') if record.get('status') in STATUS_ENUM else 'other',
            'received': bounded_scalar(record.get('received')),
            'total': bounded_scalar(record.get('total')),
            'percent': bounded_scalar(record.get('percent')),
            'speed': bounded_scalar(record.get('speed')),
        })
    return {'available': True, 'count': len(summary), 'records': summary[:16]}


def wait_download(client, name, predicate, message, timeout=40):
    def check():
        for d in downloads(client, None):
            if d.get('name') == name and predicate(d):
                return d
        return None
    return wait_for(check, message, timeout)


def ctrl_j(client, device):
    foreground(device)
    env = {**os.environ, 'DISPLAY': device.display}
    subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+j'], env=env,
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)


def initial_report():
    return {'schema': 1, 'diagnostic_only': True, 'preparation': True,
            'completed': False, 'checks': [], 'stage': 'setup',
            'open_launch': 'test_shim_only', 'real_os_associations': 'unclosed_criterion',
            'windows_explorer_path': 'unclosed_criterion', 'cleanup_completed': False}


def run(args):
    report = initial_report(); device = wm = None; root = None; client = None
    fixture = Fixture(); close_attempted = False
    out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    started = time.monotonic(); work_deadline = started + 240
    def remaining(cap):
        left = work_deadline - time.monotonic()
        if left <= 0: raise Blocked('downloads_work_deadline')
        return min(cap, left)
    try:
        if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'linux':
            raise Blocked('gha_linux_only')
        report['preparation'] = False  # реальный native-прогон прошёл preflight
        root = Path(tempfile.mkdtemp(prefix='nb-downloads-')).resolve(strict=True)
        exe = Path(args.exe).resolve(strict=True); cef = Path(args.cef).resolve(strict=True)
        origin = fixture.start()
        download_dir = root / 'downloads'; download_dir.mkdir()
        shim_dir, receipt = make_opener_shim(root)
        os.environ['NEXUSBRO_TEST_DOWNLOAD_DIR'] = str(download_dir)
        os.environ['PATH'] = str(shim_dir) + os.pathsep + os.environ.get('PATH', '')
        device = Device('downloads', root / 'device', exe, cef)
        identifier = 'downloads'
        url = origin + '/page.html'
        record = super_record(identifier, 'Downloads fixture', url, proxy='')
        folder = device.data / 'profili' / identifier; folder.mkdir(parents=True)
        (folder / 'obychnye.json').write_text(json.dumps({'vkladki': [{'id': 1, 'adres': url, 'zagolovok': 'DL fixture', 'otlozhena': True}], 'aktivnaya': 1, 'sleduyushchaya': 2}))
        for name in ('vkladki.json', 'sohranennye.json'):
            (device.data / name).write_text(json.dumps([record]))
        (device.data / 'nastrojki.json').write_text(json.dumps({'avtoobnovleniya': False, 'apparatnoe_uskorenie': False, 'memory_saver': False}))
        report['stage'] = 'startup'; client = device.start()
        wm = OwnedWindowManager(device, root / 'wm'); device.player_wm = wm
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
        report['stage'] = 'downloads-start'
        # Реальные скачивания: обычная нативная команда Pereyti на attachment URL
        # в том же CEF-табе (DOM оболочки не содержит этих ссылок — клики по ней
        # были бы ложными). Никакого synthetic download state.
        # Сериализация: три навигации по 0.3с создавали гонку — новая
        # навигация могла отменить предыдущую ДО on_before_download. Каждая
        # загрузка стартует после подтверждения предыдущей; сами вложения и
        # требование активного прогресса не ослаблены.
        call(client, 'obychnaya_komanda', id=identifier,
             komanda={'deystvie': 'pereyti', 'id': tab, 'adres': origin + '/note.txt'})
        wait_download(client, 'note.txt',
            lambda d: d.get('status') == 'Готово' and (download_dir / 'note.txt').is_file(),
            'note.txt did not complete with a real file', remaining(30))
        call(client, 'obychnaya_komanda', id=identifier,
             komanda={'deystvie': 'pereyti', 'id': tab, 'adres': origin + '/file-known.bin'})
        report['stage'] = 'progress-observation'
        report['downloads_before_wait'] = download_summary(client)
        active_known = wait_download(client, 'file-known.bin',
            lambda d: d.get('status') == 'Загружается' and d.get('total') == KNOWN_TOTAL and type(d.get('percent')) is int and 0 <= d['percent'] <= 100,
            'Known-length download did not report real progress', remaining(40))
        report['checks'].append('known_length_progress_real')
        ctrl_j(client,device)
        wait_for(lambda:client.js('Array.from(document.querySelectorAll(".download-row")).some(r=>r.textContent.includes("file-known.bin")&&r.textContent.includes("Загружается")&&!!r.querySelector("progress[value]"))'),'Known progress not visible in the real library',remaining(10))
        report['checks'].append('known_progress_visible_in_library')
        click_control(client,'.library-panel header button[aria-label="Закрыть библиотеку"]',remaining(10))
        wait_for(lambda:not client.js('!!document.querySelector(".library-panel")'),'Library did not close',remaining(10))
        wait_download(client, 'file-known.bin',
            lambda d: d.get('status') == 'Готово' and (download_dir / 'file-known.bin').is_file(),
            'file-known.bin did not complete with a real file', remaining(60))
        call(client, 'obychnaya_komanda', id=identifier,
             komanda={'deystvie': 'pereyti', 'id': tab, 'adres': origin + '/file-unknown.bin'})
        active_unknown = wait_download(client, 'file-unknown.bin',
            lambda d: d.get('status') == 'Загружается' and type(d.get('total')) is int and d['total'] <= 0,
            'Unknown-length download did not report unknown total', remaining(40))
        report['checks'].append('unknown_length_indeterminate_state')
        ctrl_j(client,device)
        wait_for(lambda:client.js('Array.from(document.querySelectorAll(".download-row")).some(r=>r.textContent.includes("file-unknown.bin")&&r.textContent.includes("размер неизвестен")&&!!r.querySelector("progress:not([value])"))'),'Unknown progress not visible in the real library',remaining(10))
        report['checks'].append('unknown_progress_visible_indeterminate')
        click_control(client,'.library-panel header button[aria-label="Закрыть библиотеку"]',remaining(10))
        wait_for(lambda:not client.js('!!document.querySelector(".library-panel")'),'Library did not close',remaining(10))
        first_received = active_unknown['received']
        wait_download(client, 'file-unknown.bin',
            lambda d: (d.get('received') or 0) > first_received,
            'Received bytes did not advance', remaining(30))
        report['checks'].append('received_bytes_advance')
        report['stage'] = 'completion'
        for name in ('note.txt', 'file-known.bin', 'file-unknown.bin'):
            done = wait_download(client, name,
                lambda d: d.get('status') == 'Готово' and (download_dir / name).is_file(),
                f'{name} did not complete with a real file', remaining(60))
            require((download_dir / name).stat().st_size > 0, f'{name} file empty')
        require((download_dir / 'note.txt').stat().st_size == 5, 'note.txt size mismatch')
        require((download_dir / 'file-known.bin').stat().st_size == KNOWN_TOTAL, 'known size mismatch')
        require((download_dir / 'file-unknown.bin').stat().st_size == KNOWN_TOTAL, 'unknown size mismatch')
        report['checks'].append('real_files_completed')
        report['stage'] = 'library-ui'
        ctrl_j(client, device)
        js_ok = wait_for(lambda: client.js('!!document.querySelector(".library-panel .download-row")'),
                         'Downloads library did not open via Ctrl+J', remaining(15))
        require(js_ok, 'library_missing')
        require(client.js('!!document.querySelector(".library-panel")?.matches(":focus-within")') is True,
                'Library panel did not take real focus')
        require(client.js('!!Array.from(document.querySelectorAll(".download-actions button")).find(b=>b.textContent.trim()==="Открыть файл")'),
                'Open file button missing for finished download')
        require(client.js('!!Array.from(document.querySelectorAll(".download-actions button")).find(b=>b.textContent.trim()==="Показать в папке")'),
                'Folder button missing for finished download')
        report['checks'].append('library_rows_and_actions_real')
        report['stage'] = 'open-file-shim'
        # Точно статья note.txt: первая .download-primary может быть у .bin
        # (заблокированного политикой) — клик только по нужной записи.
        targeted = client.js('(()=>{const a=Array.from(document.querySelectorAll(".library-panel article")).find(x=>x.textContent.includes("note.txt"));const b=a?.querySelector(".download-actions .download-primary");if(!b)return false;b.setAttribute("data-nb-open-target","1");return true})()')
        require(targeted is True, 'note.txt open button not found')
        click_control(client, '[data-nb-open-target="1"]', remaining(10))
        deadline = time.monotonic() + remaining(10); opened = None
        while time.monotonic() < deadline:
            if receipt.is_file() and receipt.read_text().strip():
                opened = json.loads(receipt.read_text().strip().splitlines()[-1])['arg']; break
            time.sleep(.25)
        require(opened and Path(opened).name.endswith('.txt') and Path(opened).is_file(),
                'Open-file shim did not receive the safe txt path')
        report['checks'].append('open_file_uses_saved_safe_path')
        report['stage'] = 'record-removal'
        before = (download_dir / 'note.txt').is_file()
        targeted = client.js('(()=>{const a=Array.from(document.querySelectorAll(".library-panel article")).find(x=>x.textContent.includes("note.txt"));const b=Array.from(a?.querySelectorAll(".download-actions button")||[]).find(x=>x.textContent.includes("Убрать из списка"));if(!b)return false;b.setAttribute("data-nb-remove-target","1");return true})()')
        require(targeted is True, 'note.txt remove button not found')
        click_control(client, '[data-nb-remove-target="1"]', remaining(10))
        wait_for(lambda: not any(d['name'] == 'note.txt' for d in downloads(client, None)),
                 'Record removal did not update the list', remaining(15))
        require(before and (download_dir / 'note.txt').is_file(), 'Record removal deleted the file')
        report['checks'].append('record_removal_keeps_file')
        report['stage'] = 'isolation'
        # Реальное переключение разделов библиотеки: закладки/история не должны
        # содержать записей загрузок, а раздел загрузок — вернуться к своим.
        for section, label in (('zakladki', 'Закладки'), ('istoriya', 'История')):
            clicked = client.js('(()=>{const b=Array.from(document.querySelectorAll(".library-panel nav button")).find(x=>x.textContent.trim()===' + json.dumps(label) + ');if(!b)return false;b.click();return true})()')
            require(clicked is True, f'library section {label} unavailable')
            wait_for(lambda: client.js('!document.querySelector(".library-list")?.getAttribute("aria-busy")||document.querySelector(".library-list")?.getAttribute("aria-busy")==="false"'),
                     f'{label} section did not settle', remaining(10))
            leaked = client.js('Array.from(document.querySelectorAll(".library-list article")).some(a=>a.textContent.includes("file-known.bin")||a.textContent.includes("file-unknown.bin"))')
            require(leaked is not True, f'download records leaked into {label}')
        back = client.js('(()=>{const b=Array.from(document.querySelectorAll(".library-panel nav button")).find(x=>x.textContent.trim()==="Загрузки");if(!b)return false;b.click();return true})()')
        require(back is True, 'downloads section unavailable')
        wait_for(lambda: client.js('Array.from(document.querySelectorAll(".library-list article")).some(a=>a.textContent.includes("file-known.bin"))'),
                 'Downloads section did not restore its own records', remaining(10))
        report['checks'].append('sections_isolated_real')
        report['stage'] = 'normal-owned-exit'; close_attempted = True
        close_for_privacy(client, device, report)
        report['completed'] = True; report['stage'] = 'complete'
    except Blocked as error:
        report['blocked_reason'] = str(error)
    except Exception as error:
        # wait_for/require бросают NativeFailure (AssertionError), не Blocked —
        # раньше это терялось в generic. Фиксированные наши сообщения мапятся
        # в allowlist-коды; чужие строки не пишутся.
        message = str(error)
        report['blocked_reason'] = WAIT_MESSAGES.get(message,
            'native_assertion' if type(error).__name__ in ('NativeFailure', 'AssertionError') else 'downloads_native_unavailable')
        report['failure_type'] = type(error).__name__ if type(error).__name__ in (
            'NativeFailure', 'AssertionError', 'RuntimeError', 'OSError', 'KeyError', 'TypeError') else 'Other'
        try:
            if client is not None:
                report['downloads_at_failure'] = download_summary(client)
        except Exception:
            pass
    finally:
        if client is not None and not close_attempted and device is not None and device.app is not None:
            prior = report['stage']
            try:
                close_attempted = True; close_for_privacy(client, device, report)
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
        if root:
            try:
                shutil.rmtree(root)
            except OSError:
                cleanup = False
        os.environ.pop('NEXUSBRO_TEST_DOWNLOAD_DIR', None)
        receipt = report.get('app_shutdown_receipt')
        if isinstance(receipt, dict):
            report['app_shutdown_receipt'] = {key: receipt[key] for key in
                ('schema', 'complete', 'owners', 'clean', 'forced', 'unacknowledged', 'unclean', 'pending_starts')
                if key in receipt}
        report['cleanup_completed'] = cleanup
        if not cleanup: report['completed'] = False; report['blocked_reason'] = 'cleanup_failed'
        if not report.get('owned_exit', {}).get('clean_shutdown_receipt_verified'): report['completed'] = False
        report['wall_seconds'] = round(time.monotonic() - started, 3)
        (out / 'result.json').write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    return 0 if report['completed'] else 1


def self_test():
    fixture = Fixture(); origin = fixture.start(); fixture.close()
    assert origin.startswith('http://127.0.0.1:')
    root = Path(tempfile.mkdtemp(prefix='nb-shim-'))
    shim_dir, receipt = make_opener_shim(root)
    assert (shim_dir / 'xdg-open').is_file() and os.access(shim_dir / 'xdg-open', os.X_OK)
    report = initial_report()
    assert report['preparation'] is True and report['real_os_associations'] == 'unclosed_criterion'
    shutil.rmtree(root)
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
