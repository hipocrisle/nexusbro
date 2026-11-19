"""Actual bundled App updater UI over real CEF, with explicit plugin adapters.

Transport streams bytes from a real local HTTP server. Updater IPC, confirmation
and OS installation are test adapters: signatures and OS installation are not
claimed here. All browser visibility/layout commands still reach real Tauri/CEF.
"""
import http.server
import json
import threading
import time


def verify_updater(client, device, visible_cef, checks):
    class Download(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_OPTIONS(self):
            self.send_response(204)
            self.send_header('Access-Control-Allow-Origin', '*')
            self.send_header('Access-Control-Allow-Methods', 'GET, OPTIONS')
            self.send_header('Access-Control-Allow-Private-Network', 'true')
            self.end_headers()

        def do_GET(self):
            if self.path != '/update-fixture':
                self.send_error(404)
                return
            chunk = b'\0' * 16384
            self.send_response(200)
            self.send_header('Access-Control-Allow-Origin', '*')
            self.send_header('Access-Control-Allow-Private-Network', 'true')
            self.send_header('Content-Type', 'application/octet-stream')
            self.send_header('Content-Length', str(len(chunk) * 160))
            self.end_headers()
            try:
                for _ in range(160):
                    self.wfile.write(chunk)
                    self.wfile.flush()
                    time.sleep(.06)
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Download)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = 'http://127.0.0.1:' + str(server.server_port) + '/update-fixture'
    adapter = r'''
    const original=window.__TAURI_INTERNALS__.invoke;
    const state=window.__updaterNative={received:0,total:0,install:false,restore:()=>window.__TAURI_INTERNALS__.invoke=original};
    window.__TAURI_INTERNALS__.invoke=async(command,args)=>{
      if(command==='plugin:updater|check')return {rid:2147483001,currentVersion:'0.2.18',version:'native-fixture',rawJson:{}};
      if(command==='plugin:dialog|message'&&args.title==='Обновление NexusBro')return 'Yes';
      if(command==='plugin:updater|download'){
        const response=await fetch(URL);if(!response.ok)throw Error('Fixture HTTP failed');
        const total=state.total=Number(response.headers.get('Content-Length'));
        const send=event=>args.onEvent.onmessage(event);
        send({event:'Started',data:{contentLength:total}});
        const reader=response.body.getReader();
        while(true){const next=await reader.read();if(next.done)break;
          state.received+=next.value.length;
          send({event:'Progress',data:{chunkLength:next.value.length}});
        }
        send({event:'Finished',data:{}});return 2147483002;
      }
      if(command==='plugin:updater|install'){state.install=true;throw Error('fixture installation failure')}
      return original(command,args);
    };return true;
    '''.replace('fetch(URL)', 'fetch(' + json.dumps(url) + ')')
    try:
        client.js('(()=>{' + adapter + '})()')
        # Physical input runs the shipped App handler, updater module, Channel
        # and React progress state. No replacement frontend module is injected.
        client.click('button[title="Проверить обновления"]')
        client.wait('window.__updaterNative.received>0&&!window.__updaterNative.install&&!!document.querySelector(".polosa-progress")', timeout=10)
        visible_cef(client, device, 'updater-streaming-real-cef-visible')
        status = client.js('({received:window.__updaterNative.received,install:window.__updaterNative.install})')
        assert status['received'] > 0 and not status['install'], 'Download finished before native progress visibility proof'
        client.wait('window.__updaterNative.install&&!document.querySelector(".polosa-progress")&&document.body.innerText.includes("fixture installation failure")', timeout=25)
        status = client.js('({received:window.__updaterNative.received,total:window.__updaterNative.total})')
        assert status['received'] == status['total'] > 0, 'HTTP stream did not complete'
        visible_cef(client, device, 'updater-install-failure-real-cef-restored')
        checks.append({'native_updater_bundled_app_physical_button_and_real_progress': True,
                       'transport': 'real_local_http_stream',
                       'updater_ipc_confirmation_and_os_installer': 'test_adapters',
                       'download_bytes': status['received']})
    finally:
        client.js('window.__updaterNative?.restore();delete window.__updaterNative;true')
        server.shutdown()
        server.server_close()
