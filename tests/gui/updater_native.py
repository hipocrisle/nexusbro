"""Actual bundled App updater UI over real CEF, with explicit plugin adapters.

Transport streams bytes from a real local HTTP server. Updater IPC, confirmation
and OS installation are test adapters: signatures and OS installation are not
claimed here. All browser visibility/layout commands still reach real Tauri/CEF.
"""
import http.server
import json
import re
import os
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
    const original=window.fetch;
    const state=window.__updaterNative={received:0,total:0,install:false,checks:0,downloads:0,restore:()=>window.fetch=original};
    const commands=new Map(['plugin:updater|check','plugin:updater|download','plugin:updater|install','plugin:dialog|message'].map(c=>[window.__TAURI_INTERNALS__.convertFileSrc(c,'ipc'),c]));
    const response=(value,ok=true)=>new Response(JSON.stringify(value),{status:200,headers:{'Content-Type':'application/json','Tauri-Response':ok?'ok':'error'}});
    window.fetch=async(input,options)=>{
      const command=commands.get(typeof input==='string'?input:input.url);
      if(!command)return original(input,options);
      try{
        const args=JSON.parse(options.body);
        if(command==='plugin:updater|check'){state.checks++;return response({rid:2147483001,currentVersion:'0.2.18',version:'99.0.0',rawJson:{}})}
        if(command==='plugin:dialog|message')return args.title==='Обновление NexusBro'?response('Yes'):original(input,options);
        if(command==='plugin:updater|download'){
          state.downloads++;
          if(!/^__CHANNEL__:\d+$/.test(args.onEvent))return response('Fixture channel contract mismatch',false);
          const channel=Number(args.onEvent.split(':')[1]);let index=0;
          const send=message=>window.__TAURI_INTERNALS__.runCallback(channel,{index:index++,message});
          const stream=await original(URL);if(!stream.ok)return response('Fixture HTTP failed',false);
          const total=state.total=Number(stream.headers.get('Content-Length'));
          send({event:'Started',data:{contentLength:total}});
          const reader=stream.body.getReader();
          while(true){const next=await reader.read();if(next.done)break;
            state.received+=next.value.length;send({event:'Progress',data:{chunkLength:next.value.length}});
          }
          send({event:'Finished',data:{}});
          window.__TAURI_INTERNALS__.runCallback(channel,{index,end:true});
          return response(2147483002);
        }
        if(command==='plugin:updater|install'){state.install=true;return response('fixture installation failure',false)}
      }catch{return response('Fixture transport contract failed',false)}
    };return true;
    '''.replace('original(URL)', 'original(' + json.dumps(url) + ')')
    try:
        client.js('(()=>{' + adapter + '})()')
        # Physical input runs the shipped App handler, updater module, Channel
        # and React progress state. No replacement frontend module is injected.
        # Bare Xvfb has no WM to focus the shell before a physical click.
        label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
        title=client.command('plugin:window|title',label=label)
        env={**os.environ,'DISPLAY':device.display}
        xid=subprocess.check_output(['xdotool','search','--onlyvisible','--name','^'+re.escape(title)+'$'],env=env,text=True).splitlines()[0]
        client.command('fokus_obolochke')
        subprocess.run(['xdotool','windowfocus','--sync',xid],env=env,check=True,timeout=5)
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
    except Exception:
        checks.append({'native_updater_diagnostic':client.js('(()=>{const s=window.__updaterNative;return {checks:s?.checks,downloads:s?.downloads,received:s?.received,install:s?.install,progress:!!document.querySelector(".polosa-progress"),focused:document.hasFocus()}})()')})
        raise
    finally:
        client.js('window.__updaterNative?.restore();delete window.__updaterNative;true')
        server.shutdown()
        server.server_close()
