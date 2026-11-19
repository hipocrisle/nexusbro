#!/usr/bin/env python3
"""GitHub-only single-device CEF bootstrap diagnostic, never feature acceptance."""
import argparse
import http.server
import json
import os
import pathlib
import re
import threading
import time


REASONS = {
    'Early safety observation unavailable': 'early_observer_unavailable',
    'Cannot enter document context': 'cannot_enter_context',
    'Document context changed': 'context_changed',
    'Document token changed': 'token_changed',
    'Current main-world context unavailable': 'main_context_unavailable',
    'Invalid safety result': 'invalid_safety_result',
    'Состояние документа ещё неизвестно': 'native_document_unknown',
    'Безопасность выгрузки страницы не подтверждена': 'safety_unconfirmed',
    'Страница не ответила на проверку и осталась в памяти': 'probe_timeout',
    'Form, media, frame, canvas or shadow DOM': 'protected_form_or_dom',
    'Frame, canvas or media element created': 'protected_dom_creation',
    'Document state unavailable or changed': 'js_state_unknown',
    'Early native instrumentation unavailable': 'js_install_unavailable',
    'Native hooks unavailable': 'js_hooks_unavailable',
    'Before-unload hook unavailable': 'js_unload_hook_unavailable',
    'Before-unload state unavailable': 'js_unload_state_unavailable',
    'Dynamic or uninspectable document': 'dynamic_document',
}
FIELDS = {'stage', 'valid', 'entered', 'eval_ok', 'retval', 'is_function', 'exception',
          'line', 'function_present', 'function_valid', 'value_present', 'value_string',
          'known', 'reason_code', 'tab', 'browser', 'automatic', 'requested', 'probe',
          'verified', 'close', 'closed', 'timeout', 'intent'}
VALUES = {'true', 'false', 'manual', 'automatic', 'unknown', 'two-phase', 'graceful',
          'row-retained', 'close', 'retained', 'install', 'probe_context_changed',
          'probe_no_observer', 'probe_cannot_enter', 'probe_invalid_string',
          'probe_invalid_json_schema', 'probe_js', 'none', 'js_install_unavailable',
          'js_dynamic_budget', 'js_probe_unavailable_or_tampered', 'js_trusted_interaction',
          'js_media_or_worker', 'js_dom_protected', 'js_unload_protected',
          'js_state_protected', 'other_internal_reason'}


def metadata_line(raw):
    """Allowlist native metadata; never copy expressions, tokens or page values."""
    prefix = next((p for p in ('NEXUSBRO_MEMORY_RENDERER', 'NEXUSBRO_MEMORY_V8',
                               'NEXUSBRO_MEMORY') if raw.startswith(p + ' ')), None)
    if prefix is None:
        return None
    result = []
    for key, value in re.findall(r'\b([a-z_]+)=([^\s]+)', raw):
        if key in FIELDS and (value in VALUES or re.fullmatch(r'-?\d{1,12}', value)):
            result.append(key + '=' + value)
    for marker in (' refused=', ' reason='):
        if marker in raw:
            reason = raw.split(marker, 1)[1].strip()
            result.append(marker.strip().split('=')[0] + '_code=' +
                          REASONS.get(reason, 'unclassified_internal_reason'))
    return prefix + ' ' + ' '.join(result) if result else None


def run(args):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise RuntimeError('Native diagnostic is restricted to GitHub Actions')
    from security_native import Device, call, super_record, wait_for

    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False, mode=0o700)
    exe, cef = pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve()
    report = {'diagnostic_only': True, 'feature_acceptance': False,
              'observed_native_refusal': False, 'lines': []}
    device = server = worker = None
    try:
        if not exe.is_file() or not cef.is_dir():
            raise RuntimeError('Built executable and CEF resources are required')
        class Page(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                body = (b'<!doctype html><title>SecurityNativeFixture</title>'
                        b'<form><input type="password" autocomplete="off"></form>'
                        b'<script>document.title="SecurityNativeFixture";'
                        b'window.__memoryBootstrapFixture=true;</script>')
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            def log_message(self, *_):
                pass
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Page)
        server.daemon_threads = True
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        device = Device('memory-bootstrap', out / 'device', exe, cef)
        (device.data / 'nastrojki.json').write_text(json.dumps({
            'memory_saver': False, 'avtoobnovleniya': False,
            'apparatnoe_uskorenie': False, 'vyhody': [],
        }), encoding='utf-8')
        client = device.start()
        owner = 'memory-bootstrap'
        address = 'http://127.0.0.1:' + str(server.server_port) + '/form'
        call(client, 'otkryt', nastrojki=super_record(owner, 'Bootstrap diagnostic', address))
        first = device.loaded(owner, address=address)['aktivnaya']
        call(client, 'obychnaya_komanda', id=owner,
             komanda={'deystvie': 'novaya', 'adres': 'about:blank'})
        def inactive_loaded():
            state = call(client, 'obychnye_spisok', id=owner, timeout=5)
            return (state.get('aktivnaya') != first and
                    any(tab['id'] == first and not tab.get('zagruzka')
                        for tab in state.get('vkladki', [])))
        wait_for(inactive_loaded, 'Fixture did not become a loaded inactive tab', 10)
        log = device.data / 'profili' / owner / 'cef.log'
        call(client, 'obychnaya_komanda', id=owner,
             komanda={'deystvie': 'usypit', 'id': first})
        deadline = time.monotonic() + 10
        raw_lines = []
        while time.monotonic() < deadline:
            text = ''
            if log.is_file():
                with log.open('rb') as stream:
                    stream.seek(0, 2)
                    stream.seek(max(0, stream.tell() - 1024 * 1024))
                    text = stream.read(1024 * 1024).decode('utf-8', errors='replace')
            raw_lines = text.splitlines()
            if any(re.match(r'NEXUSBRO_MEMORY tab=' + str(first) + r'\b.*\brefused=', line)
                   for line in raw_lines):
                report['observed_native_refusal'] = True
                break
            time.sleep(.1)
        report['lines'] = [line for raw in raw_lines if (line := metadata_line(raw))][:128]
        if not report['observed_native_refusal']:
            raise RuntimeError('No actual native refusal was observed within ten seconds')
    except Exception as error:
        # No exception text: inspector/IPC errors may include expressions or URLs.
        report['error_type'] = type(error).__name__
        raise RuntimeError('Native diagnostic failed; inspect sanitized results.json') from None
    finally:
        for cleanup in ([device.close] if device else []) + ([server.shutdown, server.server_close] if server else []):
            try:
                cleanup()
            except Exception as error:
                report['cleanup_error_type'] = type(error).__name__
        if worker:
            worker.join(timeout=2)
        (out / 'results.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps(report, ensure_ascii=False))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    for name in ('exe', 'cef', 'out'):
        parser.add_argument('--' + name, required=True)
    run(parser.parse_args())
