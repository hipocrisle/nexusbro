"""Actual Linux app: encrypted local Super trash/undo without a sync server.

Fixture definitions are written before startup. Real IPC performs vault creation
and deletion; trusted pointer clicks drive the history preview and confirmation.
No IPC overrides, synthetic DOM events, account server or external URLs.
"""
import argparse
import copy
import http.server
import json
import os
import pathlib
import threading

import psutil
from security_native import Device, call, js_wait, painted, require, settings, super_record


def native_button(client, text):
    expression = ('(()=>{const e=[...document.querySelectorAll("button")].find(e=>e.textContent.trim()==='
                  + json.dumps(text) + ');if(!e||e.disabled)return false;e.scrollIntoView({block:"center"});return true})()')
    js_wait(client, expression, 'Native history button missing')
    painted(client)
    client.text_button(text)


def engines(device):
    require(device.app.poll() is None, 'Native app exited')
    found = []
    for child in psutil.Process(device.app.pid).children(recursive=True):
        try:
            if '--nb-dvizhok' in child.cmdline():
                found.append(child.pid)
        except psutil.NoSuchProcess:
            pass
    return found


def run(exe, cef, out):
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Run native history acceptance on a disposable CI runner')
    out.mkdir(parents=True, exist_ok=False)
    device = Device('history', out / 'device', exe, cef)
    checks = []
    completed = False
    hits = []

    class Fixture(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            hits.append(True)
            body = b'<!doctype html><title>Unexpected history navigation</title>'
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{server.server_port}'
    ordinary = super_record('native-history-super', 'HistoryOriginalNameFixture', base + '/ordinary',
                            proxy='socks5://history-fixture-user:history-fixture-password@127.0.0.1:9')
    ordinary['proxy_uzel'] = 'history-fixture-node'
    private = super_record('legacy-private-history', 'PrivateHistoryNameFixture', base + '/private', private=True)
    master = 'independent history native fixture password'
    (device.data / 'nastrojki.json').write_text(json.dumps(settings('temnaya', 'history-route-id')))
    (device.data / 'sohranennye.json').write_text(json.dumps([ordinary, private]))
    (device.data / 'vkladki.json').write_text('[]')
    saved_path = device.data / 'sohranennye.json'

    def saved():
        return json.loads(saved_path.read_text())

    def history(client):
        return call(client, 'security_sync', action='history')['entries']

    def no_start(client):
        require(call(client, 'spisok') == [], 'History operation opened a Super')
        require(engines(device) == [], 'History operation launched a CEF engine')
        require(hits == [], 'History operation navigated to a fixture site')

    try:
        client = device.start()
        js_wait(client, '!!document.querySelector(' + json.dumps('button[title="Настройки NexusBro"]') + ')', 'Browser shell did not initialize', 40)
        require(call(client, 'security_vault', action='create', password=master)['unlocked'], 'Real vault creation failed')
        call(client, 'security_sync', action='configure', data={'selection': {'settings': False, 'bookmarks': False, 'supers': [ordinary['id']], 'passwords': []}})
        no_start(client)
        call(client, 'udalit_sohranennuyu', id=ordinary['id'])
        require(not any(row['id'] == ordinary['id'] for row in saved()), 'Actual saved deletion did not commit')
        rows = history(client)
        require(len(rows) == 1 and rows[0]['super_id'] == ordinary['id'] and rows[0]['reason'] == 'local_delete', 'Ordinary deletion did not create one encrypted recovery entry')
        entry_id = rows[0]['entry_id']
        call(client, 'udalit_sohranennuyu', id=private['id'])
        require(len(history(client)) == 1 and not any(row['super_id'] == private['id'] for row in history(client)), 'Legacy private definition entered recovery history')
        no_start(client)
        checks.append({'actual_saved_delete_archives_ordinary_only_private_legacy_excluded': True})

        # Open through the real application controls; no custom event dispatch.
        client.command('fokus_obolochke')
        client.click('button[title="Настройки NexusBro"]')
        js_wait(client, '!!document.querySelector(".nastrojki-okno")', 'Settings did not open')
        native_button(client, 'Пароли, аккаунт и синхронизация')
        js_wait(client, '!!document.querySelector(".security-panel") && document.querySelector(".security-panel").getAttribute("aria-busy")==="false"', 'Security panel did not become ready')
        native_button(client, 'Синхронизация')
        selector = '.security-content > details.security-card > summary'
        js_wait(client, '!!document.querySelector(' + json.dumps(selector) + ')', 'History section missing without account login')
        client.js('document.querySelector(' + json.dumps(selector) + ').scrollIntoView({block:"center"});true')
        painted(client)
        client.click(selector)
        native_button(client, 'Показать историю')
        js_wait(client, 'document.querySelector(".security-content > details")?.textContent.includes("HistoryOriginalNameFixture")', 'Encrypted history did not render')
        native_button(client, 'Посмотреть восстановление')
        js_wait(client, '!!document.querySelector(' + json.dumps('[aria-label="Предпросмотр восстановления"]') + ')', 'Native restore preview missing')
        require(not any(row['id'] == ordinary['id'] for row in saved()), 'Preview restored data without confirmation')
        native_button(client, 'Подтвердить восстановление')
        js_wait(client, 'document.querySelector(".security-content > details")?.textContent.includes("Настройки супервкладки восстановлены на этом устройстве.")', 'Confirmed restore did not finish')
        restored = next(row for row in saved() if row['id'] == ordinary['id'])
        for field in ['id', 'nazvanie', 'proxy', 'proxy_uzel', 'adres', 'cvet', 'rezat_reklamu', 'pustaya', 'privatnaya']:
            require(restored[field] == ordinary[field], 'Restore changed original ordinary metadata or route')
        no_start(client)
        status = call(client, 'security_sync', action='status')
        require(status['last_sync'] is None and status['sync_state']['state'] == 'pending'
                and status['super_states'][ordinary['id']]['state'] == 'pending', 'Local undo claimed a server ACK or green sync status')
        checks.append({'physical_history_preview_confirmation_restores_saved_only_exact_prior_route_without_ack_or_network': True})

        preview = call(client, 'security_sync', action='history_preview', data={'entry_id': entry_id})
        require(not preview['restores_saved_only'] and 'proxy' not in preview['previous'], 'Preview exposed raw route or misreported existing data')
        # Simulate a competing metadata writer after the preview. No engines or
        # app-side saved writer is active; this is a real on-disk CAS race fixture,
        # not a replacement of IPC, vault contents or native application events.
        no_start(client)
        changed = copy.deepcopy(restored)
        changed['nazvanie'] = 'ConcurrentHistoryEditFixture'
        temporary = saved_path.with_suffix('.fixture-tmp')
        temporary.write_text(json.dumps([changed]))
        temporary.replace(saved_path)
        rejection = call(client, 'security_sync', action='history_restore',
                         data={'entry_id': entry_id, 'guard': preview['guard'], 'confirm': True}, error=True)
        require('изменилась' in rejection and saved()[0]['nazvanie'] == changed['nazvanie'], 'Stale restore overwrote a competing metadata change')
        no_start(client)
        checks.append({'real_metadata_change_after_preview_rejects_stale_restore_guard': True})

        before_forget = saved_path.read_bytes()
        call(client, 'security_sync', action='history_forget', data={'entry_id': entry_id, 'confirm': False}, error=True)
        require(any(row['entry_id'] == entry_id for row in history(client)), 'Unconfirmed forget deleted recovery data')
        forgotten = call(client, 'security_sync', action='history_forget', data={'entry_id': entry_id, 'confirm': True})
        require(forgotten == {'forgotten': True, 'local_data_unchanged': True}, 'Forget acknowledgement was not explicit')
        require(not any(row['entry_id'] == entry_id for row in history(client)), 'Confirmed forget retained the entry')
        require(saved_path.read_bytes() == before_forget, 'Forgetting history deleted or changed actual saved Super')
        no_start(client)
        checks.append({'history_forget_requires_confirmation_and_never_deletes_actual_saved_super': True})

        markers = [ordinary['nazvanie'], ordinary['proxy'], ordinary['proxy_uzel'], ordinary['adres'], private['nazvanie'], master]
        for path in device.cipher_files():
            raw = path.read_bytes()
            require(all(marker.encode() not in raw for marker in markers), 'Encrypted history vault contains fixture plaintext')
        checks.append({'main_and_backup_vault_contain_no_plaintext_history_name_route_url_or_master': True})
        completed = True
    finally:
        try:
            device.close()
        except Exception:
            completed = False
            raise
        finally:
            server.shutdown()
            (out / 'results.json').write_text(json.dumps({'completed': completed, 'checks': checks,
                                                         'uses_sync_server': False, 'uses_ipc_mocks': False}, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True)
    parser.add_argument('--cef', required=True)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    run(pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve(), pathlib.Path(args.out).resolve())
