"""Actual updater.ts in one Chromium, real streamed HTTP, explicit Tauri/OS adapters.

This checks the module with startup callbacks. App startup/manual handler wiring and
native installation/signatures must separately be verified by the native CI gate.
"""
import http.server
import json
import os
import pathlib
import subprocess
import threading
import time
from playwright.sync_api import expect, sync_playwright

ROOT = pathlib.Path(__file__).resolve().parents[1]
compiled = subprocess.check_output(['node', '-e', '''
const fs=require('fs'),ts=require(process.argv[1]);
process.stdout.write(ts.transpileModule(fs.readFileSync(process.argv[2],'utf8'),{
 compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}
}).outputText);
''', str(ROOT/'desktop/node_modules/typescript'), str(ROOT/'desktop/src/updater.ts')], text=True)

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        if self.path != '/payload':
            body = b'<button id="start">Startup</button><button id="manual">Manual</button><button id="yes" hidden>Yes</button><button id="no" hidden>No</button><p id="status"></p><progress id="progress" max="100" value="0"></progress>'
            self.send_response(200); self.end_headers(); self.wfile.write(body)
            return
        chunk = b'\0'*8192
        self.send_response(200)
        self.send_header('Content-Length', str(len(chunk)*24))
        self.end_headers()
        for _ in range(24):
            try:
                self.wfile.write(chunk); self.wfile.flush(); time.sleep(.05)
            except (BrokenPipeError, ConnectionResetError):
                break

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
server.daemon_threads = True
threading.Thread(target=server.serve_forever, daemon=True).start()
fixture = r'''
window.f={calls:[],visible:true,prepared:false,accepted:false,failure:'',available:true,done:0};
window.exports={};
const f=window.f;
const update={version:'99.0.0',async download(callback, options){
 f.calls.push('download');if(!f.visible)throw Error('download hid sites');
 if(f.failure==='download')throw Error('download failed');
 const r=await fetch('/payload'),total=Number(r.headers.get('Content-Length'));
 callback({event:'Started',data:{contentLength:total}});
 const reader=r.body.getReader();while(true){const row=await reader.read();if(row.done)break;
 if(!f.visible)throw Error('stream hid sites');callback({event:'Progress',data:{chunkLength:row.value.length}});}
 callback({event:'Finished',data:{}});
},async install(){f.calls.push('install');if(f.visible||!f.prepared)throw Error('install requires completed preparation');if(['install','install-and-restart'].includes(f.failure))throw Error('install failed');},async close(){f.calls.push('close');}};
window.require=name=>({
 '@tauri-apps/plugin-updater':{check:async()=>{f.calls.push('check');if(f.failure==='check')throw Error('check failed');return f.available?update:null;}},
 '@tauri-apps/plugin-dialog':{ask:async()=>{f.calls.push('ask');if(f.visible)throw Error('dialog covered by CEF');
  document.querySelector('#yes').hidden=false;document.querySelector('#no').hidden=false;
  return new Promise(resolve=>{for(const [id,result] of [['yes',true],['no',false]])document.querySelector('#'+id).onclick=()=>{
   document.querySelector('#yes').hidden=true;document.querySelector('#no').hidden=true;resolve(result);
  };});}},
 '@tauri-apps/api/core':{invoke:async(command,args)=>{
  if(command==='pokazat_dvizhki'){
   if(args.vidno&&f.prepared)throw Error('drained writers must never resume');
   f.visible=args.vidno;f.calls.push(f.visible?'show':'hide');return;
  }
  if(command==='app_prepare_update'){
   f.calls.push('prepare');if(f.visible)throw Error('prepare must hide engines');
   if(f.failure==='prepare')throw Error('prepare failed');f.prepared=true;return;
  }
  if(command==='app_restart'){
   f.calls.push('restart');if(!f.prepared)throw Error('restart before preparation');
   if(['restart','install-and-restart'].includes(f.failure))throw Error('restart failed');return;
  }
  throw Error('unexpected command');
 }}
})[name];
'''
handlers = r'''
const run=force=>window.exports.proverit_obnovlenie(force,!force,
 text=>document.querySelector('#status').textContent=text,
 (n,total)=>{const p=document.querySelector('#progress');p.max=total||1;p.value=n;}
).finally(()=>f.done++);
document.querySelector('#start').onclick=()=>void run(false);
document.querySelector('#manual').onclick=()=>void run(true);
'''
try:
    with sync_playwright() as p:
        channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome')
        browser=p.chromium.launch(headless=True, **({'channel':channel} if channel else {}))
        page=browser.new_page()
        page.set_default_timeout(10_000)
        def reset():
            page.goto('http://127.0.0.1:'+str(server.server_port))
            page.evaluate('localStorage.clear()')
            page.add_script_tag(content=fixture+'\n'+compiled+'\n'+handlers)
        reset()
        # Real physical mouse clicks through the startup handler, duplicate calls
        # while awaiting confirmation must not produce two competing installers.
        page.locator('#start').click(); expect(page.locator('#yes')).to_be_visible()
        page.locator('#manual').click();page.wait_for_timeout(100)
        assert page.evaluate('f.calls.filter(x=>x==="check").length')==1
        page.locator('#yes').click()
        page.wait_for_function('document.querySelector("#progress").value>0&&document.querySelector("#progress").value<document.querySelector("#progress").max')
        assert page.evaluate('f.visible')
        expect(page.locator('#status')).to_contain_text('Скачиваю')
        page.wait_for_function('f.done===2')
        assert page.evaluate('f.calls.filter(x=>x==="download").length')==1
        assert page.evaluate('f.calls.filter(x=>x==="install").length')==1
        assert page.evaluate('f.calls.includes("restart")&&f.calls.includes("close")&&f.prepared&&!f.visible')
        assert page.evaluate('f.calls.filter(x=>x==="prepare").length===1&&f.calls.filter(x=>x==="restart").length===1')
        assert page.evaluate('f.calls.indexOf("prepare")<f.calls.indexOf("install")&&f.calls.indexOf("install")<f.calls.indexOf("restart")')
        # Accepted startup failures cannot be swallowed under quiet=true.
        reset();page.evaluate('f.failure="download"')
        page.locator('#start').click();page.locator('#yes').click()
        page.wait_for_function('f.done===1')
        expect(page.locator('#status')).to_contain_text('download failed')
        assert page.evaluate('f.visible&&f.calls.includes("close")&&!f.calls.includes("prepare")&&!f.calls.includes("install")&&!f.calls.includes("restart")')
        # Explicit toolbar retry is allowed immediately, no timer/automatic loop.
        page.evaluate('f.failure=""');page.locator('#manual').click();page.locator('#yes').click()
        page.wait_for_function('f.done===2')
        assert page.evaluate('f.calls.filter(x=>x==="install").length')==1
        reset();page.locator('#start').click();page.locator('#no').click()
        page.wait_for_function('f.done===1')
        assert page.evaluate('f.visible&&f.calls.includes("close")&&!f.calls.includes("download")')
        # A failed native preparation blocks BOTH installation and restart.
        reset();page.evaluate('f.failure="prepare"');page.locator('#start').click();page.locator('#yes').click()
        page.wait_for_function('f.done===1')
        expect(page.locator('#status')).to_contain_text('prepare failed')
        assert page.evaluate('!f.prepared&&f.visible&&!f.calls.includes("install")&&!f.calls.includes("restart")&&f.calls.includes("close")')
        # Once preparation succeeded, the old process never resumes writers.
        reset();page.evaluate('f.failure="install"');page.locator('#start').click();page.locator('#yes').click()
        page.wait_for_function('f.done===1')
        expect(page.locator('#status')).to_contain_text('install failed')
        assert page.evaluate('f.prepared&&!f.visible&&f.calls.filter(x=>x==="restart").length===1&&f.calls.includes("close")')
        assert page.evaluate('f.calls.indexOf("prepare")<f.calls.indexOf("install")&&f.calls.indexOf("install")<f.calls.indexOf("restart")&&!f.calls.slice(f.calls.indexOf("prepare")+1).includes("show")')
        # A failed restart is not retried or converted into a resume of old pages.
        reset();page.evaluate('f.failure="restart"');page.locator('#start').click();page.locator('#yes').click()
        page.wait_for_function('f.done===1')
        expect(page.locator('#status')).to_contain_text('restart failed')
        assert page.evaluate('f.prepared&&!f.visible&&f.calls.filter(x=>x==="install").length===1&&f.calls.filter(x=>x==="restart").length===1&&f.calls.includes("close")')
        reset();page.evaluate('f.failure="install-and-restart"');page.locator('#start').click();page.locator('#yes').click()
        page.wait_for_function('f.done===1')
        expect(page.locator('#status')).to_contain_text('install failed')
        expect(page.locator('#status')).to_contain_text('restart failed')
        assert page.evaluate('f.prepared&&!f.visible&&f.calls.filter(x=>x==="restart").length===1&&f.calls.includes("close")')
        reset();page.evaluate('f.available=false');page.locator('#start').click();page.wait_for_function('f.done===1')
        assert page.evaluate('f.calls.join(",")==="check"&&f.visible')
        reset();page.evaluate('f.failure="check"');page.locator('#start').click();page.wait_for_function('f.done===1')
        assert page.evaluate('!localStorage.getItem("nexusbro.lastUpdateCheck")&&f.visible')
        expect(page.locator('#status')).to_have_text('')
        print(json.dumps({'real_chromium':True,'module':'actual updater.ts','http_download':'real streamed localhost','tauri_and_os_install':'explicit test adapters','startup_confirmation_download_progress':True,'startup_accepted_failure_visible':True,'decline_closes_update_resource':True,'singleflight_duplicate_start_and_manual':True,'manual_retry_after_failure':True,'no_hide_during_check_or_stream':True,'prepare_before_install_before_restart':True,'prepare_failure_blocks_install_and_restart':True,'install_failure_restarts_drained_old_version':True,'restart_failure_no_resume_no_retry':True,'check_error_does_not_throttle_retry':True}))
        browser.close()
finally:
    server.shutdown();server.server_close()
