"""Production updater control flow in a native webview with visible real CEF.

Transport streams bytes from a real local HTTP server. Confirmation and the OS
installer are explicit test adapters: this does not claim to test OS installation
or updater signatures (those have separate package acceptance).
"""
import http.server
import json
import pathlib
import subprocess
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
            self.send_header('Content-Length', str(len(chunk) * 120))
            self.end_headers()
            try:
                for _ in range(120):
                    self.wfile.write(chunk)
                    self.wfile.flush()
                    time.sleep(.06)
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Download)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    root = pathlib.Path(__file__).resolve().parents[2]
    # Transpile the actual shipped source, without adding any production hook.
    program = "const ts=require('./desktop/node_modules/typescript');let s=require('fs').readFileSync('desktop/src/updater.ts','utf8');s=s.replace(/^import .*;\\n/gm,'').replace('export async function','async function');process.stdout.write(ts.transpileModule(s,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.None}}).outputText)"
    source = subprocess.check_output(['node', '-e', program], cwd=root, text=True)
    url = 'http://127.0.0.1:' + str(server.server_port) + '/update-fixture'
    adapter = r'''
    window.__updaterNative={received:0,total:0,done:false,error:'',install:false};
    const state=window.__updaterNative;
    const invoke=(command,args)=>window.__TAURI_INTERNALS__.invoke(command,args);
    const ask=async()=>true;
    const relaunch=async()=>{throw Error('Unexpected relaunch after failed installer')};
    const check=async()=>({version:'native-fixture',download:async(onEvent)=>{
      const response=await fetch(URL);if(!response.ok)throw Error('Fixture HTTP failed');
      const total=Number(response.headers.get('Content-Length'));
      onEvent({event:'Started',data:{contentLength:total}});
      const reader=response.body.getReader();
      while(true){const next=await reader.read();if(next.done)break;
        onEvent({event:'Progress',data:{chunkLength:next.value.length}});
      }
      onEvent({event:'Finished',data:{}});
    },install:async()=>{state.install=true;throw Error('fixture installation failure')}});
    '''.replace('fetch(URL)', 'fetch(' + json.dumps(url) + ')')
    try:
        client.js('(()=>{' + adapter + source + r'''
        proverit_obnovlenie(true,false,message=>state.error=message,
          (received,total)=>{state.received=received;state.total=total;})
          .then(()=>state.done=true,()=>state.done=true);return true;})()''')
        client.wait('window.__updaterNative.received>0&&!window.__updaterNative.done', timeout=10)
        visible_cef(client, device, 'updater-streaming-real-cef-visible')
        status = client.js('window.__updaterNative')
        assert status['received'] > 0 and not status['done'], 'Download finished before native visibility proof'
        client.wait('window.__updaterNative.done', timeout=25)
        status = client.js('window.__updaterNative')
        assert status['received'] == status['total'] > 0, 'HTTP stream did not complete'
        assert status['install'] and 'fixture installation failure' in status['error'], 'Expected installer failure did not run'
        visible_cef(client, device, 'updater-install-failure-real-cef-restored')
        checks.append({'native_updater_production_control_flow': True,
                       'transport': 'real_local_http_stream',
                       'confirmation_and_os_installer': 'test_adapters',
                       'download_bytes': status['received']})
    finally:
        client.js('delete window.__updaterNative;true')
        server.shutdown()
        server.server_close()
