#!/usr/bin/env python3
"""GitHub-only single-device CEF bootstrap diagnostic, never feature acceptance."""
import argparse
import http.server
import json
import os
import pathlib
import re
import statistics
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
          'verified', 'close', 'closed', 'timeout', 'intent', 'registered', 'safety',
          'accepted', 'bootstrap_approved', 'uri_matches', 'initial_blank', 'installed'}
VALUES = {'true', 'false', 'manual', 'automatic', 'unknown', 'two-phase', 'graceful',
          'row-retained', 'close', 'retained', 'install', 'probe_context_changed',
          'probe_no_observer', 'probe_cannot_enter', 'probe_invalid_string',
          'probe_invalid_json_schema', 'probe_js', 'none', 'js_install_unavailable',
          'js_dynamic_budget', 'js_probe_unavailable_or_tampered', 'js_trusted_interaction',
          'js_media_or_worker', 'js_dom_protected', 'js_unload_protected',
          'js_state_protected', 'other_internal_reason', 'early', 'keep',
          'native_binding', 'native_capture_success'}


def metadata_line(raw):
    """Allowlist native metadata; never copy expressions, tokens or page values."""
    prefix = next((p for p in ('NEXUSBRO_MEMORY_BOOTSTRAP', 'NEXUSBRO_MEMORY_DOCUMENT',
                               'NEXUSBRO_MEMORY_RENDERER', 'NEXUSBRO_MEMORY_V8',
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


def log_lines(path):
    if not path.is_file():
        return []
    with path.open('rb') as stream:
        stream.seek(0, 2)
        stream.seek(max(0, stream.tell() - 1024 * 1024))
        return stream.read(1024 * 1024).decode('utf-8', errors='replace').splitlines()


def owned_uss(device):
    import psutil
    if device.app.poll() is not None:
        raise RuntimeError('Owned application exited during measurement')
    parent = psutil.Process(device.app.pid)
    processes = [parent] + parent.children(recursive=True)
    total, count, seen = 0, 0, set()
    for process in processes:
        try:
            identity = (process.pid, process.create_time())
            if identity in seen:
                continue
            seen.add(identity)
            # Linux USS, not Windows private commit and not a forced working-set trim.
            total += process.memory_full_info().uss
            count += 1
        except psutil.NoSuchProcess:
            continue
    if not count:
        raise RuntimeError('No owned process memory samples')
    return total, count


def median_uss(device):
    samples, counts = [], []
    for _ in range(5):
        value, count = owned_uss(device)
        samples.append(value)
        counts.append(count)
        time.sleep(.2)
    return {'metric': 'linux_process_tree_uss_bytes', 'samples': samples,
            'median': int(statistics.median(samples)), 'process_counts': counts}


def run(args):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise RuntimeError('Native diagnostic is restricted to GitHub Actions')
    from security_native import Device, call, super_record, wait_for

    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False, mode=0o700)
    exe, cef = pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve()
    report = {'diagnostic_only': True, 'feature_acceptance': False,
              'scope': 'native-engine-component', 'safe_unload': args.safe_unload,
              'observed_native_refusal': False, 'lines': [], 'phase': 'startup'}
    device = server = worker = log = None
    receipts, receipt_lock = [], threading.Lock()
    try:
        if not exe.is_file() or not cef.is_dir():
            raise RuntimeError('Built executable and CEF resources are required')
        class Page(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path not in ('/form', '/safe'):
                    self.send_response(404); self.end_headers(); return
                if args.safe_unload:
                    body = (b'<!doctype html><title>SecurityNativeFixture</title><p>Static fixture</p><script>'
                            b'const oldCookie=document.cookie.split("; ").includes("nb_memory_diag=1");'
                            b'const oldStorage=localStorage.getItem("nb_memory_diag")==="1";'
                            b'document.cookie="nb_memory_diag=1;path=/;SameSite=Lax";'
                            b'localStorage.setItem("nb_memory_diag","1");'
                            b'window.__memoryDiagAllocation=new Uint8Array(64*1024*1024);'
                            b'for(let i=0;i<window.__memoryDiagAllocation.length;i+=4096)window.__memoryDiagAllocation[i]=37;'
                            b'window.__memoryDiagAllocation[window.__memoryDiagAllocation.length-1]=91;'
                            b'fetch("/receipt",{method:"POST",headers:{"Content-Type":"application/json"},'
                            b'body:JSON.stringify({bytes:window.__memoryDiagAllocation.length,'
                            b'first:window.__memoryDiagAllocation[0],last:window.__memoryDiagAllocation[window.__memoryDiagAllocation.length-1],'
                            b'cookie_before:oldCookie,storage_before:oldStorage})});</script>')
                else:
                    body = (b'<!doctype html><title>SecurityNativeFixture</title>'
                            b'<form><input type="password" autocomplete="off"></form>'
                            b'<script>document.title="SecurityNativeFixture";'
                            b'window.__memoryBootstrapFixture=true;</script>')
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            def do_POST(self):
                size = int(self.headers.get('Content-Length', '0'))
                if self.path != '/receipt' or not 0 < size <= 512:
                    self.send_response(400); self.end_headers(); return
                try:
                    value = json.loads(self.rfile.read(size))
                    valid = (set(value) == {'bytes', 'first', 'last', 'cookie_before', 'storage_before'}
                             and value['bytes'] == 64*1024*1024 and value['first'] == 37 and value['last'] == 91
                             and type(value['cookie_before']) is bool and type(value['storage_before']) is bool)
                except (ValueError, TypeError):
                    valid = False
                if not valid:
                    self.send_response(400); self.end_headers(); return
                with receipt_lock:
                    if len(receipts) < 8:
                        receipts.append(value)
                self.send_response(204); self.end_headers()
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
        address = 'http://127.0.0.1:' + str(server.server_port) + ('/safe' if args.safe_unload else '/form')
        report['phase'] = 'opening-fixture'
        call(client, 'otkryt', nastrojki=super_record(owner, 'Bootstrap diagnostic', address))
        first = device.loaded(owner, address=address)['aktivnaya']
        log = device.data / 'profili' / owner / 'cef.log'
        if args.safe_unload:
            report['phase'] = 'waiting-allocation'
            wait_for(lambda: len(receipts) == 1, 'Touched allocation receipt missing', 10)
            report['initial_allocation_receipt'] = dict(receipts[0])
            if receipts[0]['cookie_before'] or receipts[0]['storage_before']:
                raise RuntimeError('Owned fresh profile was not empty')
        report['phase'] = 'selecting-blank'
        call(client, 'obychnaya_komanda', id=owner,
             komanda={'deystvie': 'novaya', 'adres': 'about:blank'})
        def inactive_loaded():
            state = call(client, 'obychnye_spisok', id=owner, timeout=5)
            return (state.get('aktivnaya') != first and
                    any(tab['id'] == first and not tab.get('zagruzka')
                        for tab in state.get('vkladki', [])))
        wait_for(inactive_loaded, 'Fixture did not become a loaded inactive tab', 10)
        if args.safe_unload:
            report['phase'] = 'before-close-metric'
            report['before_close'] = median_uss(device)
        report['phase'] = 'requesting-unload'
        call(client, 'obychnaya_komanda', id=owner,
             komanda={'deystvie': 'usypit', 'id': first})
        report['phase'] = 'waiting-close' if args.safe_unload else 'waiting-refusal'
        deadline = time.monotonic() + 10
        report['observed_native_close'] = False
        while time.monotonic() < deadline:
            raw = log_lines(log)
            refused = any(re.match(r'NEXUSBRO_MEMORY tab=' + str(first) + r'\b.*\brefused=', line) for line in raw)
            report['observed_native_refusal'] |= refused
            if not args.safe_unload:
                if refused:
                    break
            else:
                if refused:
                    raise RuntimeError('Safe fixture was refused; component diagnostic failed')
                state = call(client, 'obychnye_spisok', id=owner, timeout=5)
                rows = [tab for tab in state.get('vkladki', []) if tab['id'] == first]
                verified = any(re.match(r'NEXUSBRO_MEMORY tab=' + str(first) + r'\b.*verified=two-phase close=graceful\b', line) for line in raw)
                closed = any(re.match(r'NEXUSBRO_MEMORY tab=' + str(first) + r'\b.*closed=row-retained\b', line) for line in raw)
                if (len(rows) == 1 and rows[0].get('otlozhena') and not rows[0].get('memory_pending')
                        and len(state['vkladki']) == 2 and verified and closed):
                    report['observed_native_close'] = True
                    report['retained_row'] = True
                    break
            time.sleep(.1)
        if not args.safe_unload:
            if not report['observed_native_refusal']:
                raise RuntimeError('No actual native refusal within ten seconds')
        else:
            if not report['observed_native_close']:
                raise RuntimeError('Retained native close was not observed within ten seconds')
            report['phase'] = 'measuring-recovery'
            reclaim_deadline = time.monotonic() + 10
            while time.monotonic() < reclaim_deadline:
                report['after_close'] = median_uss(device)
                drop = report['before_close']['median'] - report['after_close']['median']
                report['uss_drop_bytes'] = drop
                if drop >= 16*1024*1024:
                    report['observed_uss_recovery'] = True
                    break
            if not report.get('observed_uss_recovery'):
                raise RuntimeError('Actual Linux USS recovery was below sixteen MiB')
            report['phase'] = 'waking'
            call(client, 'obychnaya_komanda', id=owner, komanda={'deystvie': 'vybrat', 'id': first})
            restored = device.loaded(owner, address=address, tab_id=first)
            wait_for(lambda: len(receipts) >= 2, 'Restored allocation receipt missing', 10)
            report['restored_allocation_receipt'] = dict(receipts[-1])
            report['same_row_restored'] = restored['aktivnaya'] == first
            report['persistent_storage_preserved'] = receipts[-1]['cookie_before'] and receipts[-1]['storage_before']
            if not report['same_row_restored'] or not report['persistent_storage_preserved']:
                raise RuntimeError('Native wake did not preserve row and persistent storage')
            report['component_steps_observed'] = True
        report['phase'] = 'done'
    except Exception as error:
        report['error_type'] = type(error).__name__
        raise RuntimeError('Native diagnostic failed; inspect sanitized results.json') from None
    finally:
        if log is not None:
            report['lines'] = [line for raw in log_lines(log) if (line := metadata_line(raw))][:128]
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
    parser.add_argument('--safe-unload', action='store_true', help='Diagnostic native-engine close/USS/wake component only; never release acceptance')
    run(parser.parse_args())
