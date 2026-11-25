"""Actual bundled App updater UI over real CEF, with explicit plugin adapters.

Transport streams bytes from a real local HTTP server. Updater IPC, confirmation
and OS installation, prepare-update and restart are test adapters: signatures,
installation and actual drain/restart are not claimed here. Browser visibility
commands still reach real Tauri/CEF. Drain acceptance is app_shutdown_native.py.
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
    const controller=new AbortController();
    const state=window.__updaterNative={received:0,total:0,install:false,checks:0,downloads:0,prepareRequests:0,restartRequests:0,prepared:false,showAfterPrepared:0,order:[],errors:[],closed:false,cleanup:()=>{state.closed=true;controller.abort()}};
    const record=stage=>{if(state.order.length<64)state.order.push(stage);else if(!state.errors.includes('event_bound'))state.errors.push('event_bound')};
    const invalid=code=>{if(state.errors.length<16)state.errors.push(code);return response('Fixture lifecycle contract mismatch',false)};
    const commands=new Map(['plugin:updater|check','plugin:updater|download','plugin:updater|install','plugin:dialog|message','app_prepare_update','app_restart','pokazat_dvizhki'].map(c=>[window.__TAURI_INTERNALS__.convertFileSrc(c,'ipc'),c]));
    const response=(value,ok=true)=>new Response(JSON.stringify(value),{status:200,headers:{'Content-Type':'application/json','Tauri-Response':ok?'ok':'error'}});
    window.fetch=async(input,options)=>{
      const command=commands.get(typeof input==='string'?input:input.url);
      if(!command)return original(input,options);
      // Keep lifecycle/install guards for this disposable Device even after
      // cleanup: an in-flight UI promise must never restart its real parent.
      if(state.closed&&command!=='pokazat_dvizhki'&&command!=='plugin:dialog|message')return response('Fixture updater adapter closed',false);
      try{
        const args=JSON.parse(options.body);
        if(command==='plugin:updater|check'){state.checks++;record('check');return response({rid:2147483001,currentVersion:'0.2.18',version:'99.0.0',rawJson:{}})}
        if(command==='plugin:dialog|message'){if(args.title!=='Обновление NexusBro')return original(input,options);if(!state.closed)record('confirmation');return response(state.closed?'No':'Yes')}
        if(command==='pokazat_dvizhki'){
          if(!state.closed){record(args.vidno?'show':'hide');if(args.vidno&&state.prepared)state.showAfterPrepared++;}
          return original(input,options);
        }
        if(command==='app_prepare_update'){
          state.prepareRequests++;record('prepare');
          if(state.prepareRequests!==1||state.install||state.total<=0||state.received!==state.total||!state.order.includes('download_finished'))return invalid('prepare_order');
          state.prepared=true;return response(null);
        }
        if(command==='app_restart'){
          state.restartRequests++;record('restart');
          if(state.restartRequests!==1||!state.prepared||!state.install||!state.order.includes('install_failed'))return invalid('restart_order');
          return response(null);
        }
        if(command==='plugin:updater|download'){
          state.downloads++;record('download');
          if(!/^__CHANNEL__:\d+$/.test(args.onEvent))return response('Fixture channel contract mismatch',false);
          const channel=Number(args.onEvent.split(':')[1]);let index=0;
          const send=message=>window.__TAURI_INTERNALS__.runCallback(channel,{index:index++,message});
          const stream=await original(URL,{signal:controller.signal});if(!stream.ok)return response('Fixture HTTP failed',false);
          const total=state.total=Number(stream.headers.get('Content-Length'));
          send({event:'Started',data:{contentLength:total}});
          const reader=stream.body.getReader();
          while(true){const next=await reader.read();if(next.done)break;
            state.received+=next.value.length;send({event:'Progress',data:{chunkLength:next.value.length}});
          }
          record('download_finished');send({event:'Finished',data:{}});
          window.__TAURI_INTERNALS__.runCallback(channel,{index,end:true});
          return response(2147483002);
        }
        if(command==='plugin:updater|install'){state.install=true;record('install_failed');if(!state.prepared||state.prepareRequests!==1||state.restartRequests!==0)return invalid('install_order');return response('fixture installation failure',false)}
      }catch{return response('Fixture transport contract failed',false)}
      return response('Fixture command not admitted',false);
    };return true;
    '''.replace('original(URL,', 'original(' + json.dumps(url) + ',')
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
        client.wait('window.__updaterNative.received>window.__updaterNative.total*.25&&!window.__updaterNative.install&&!!document.querySelector(".polosa-progress")', timeout=10)
        client.js('window.__updaterPainted=false;requestAnimationFrame(()=>requestAnimationFrame(()=>window.__updaterPainted=true));true')
        client.wait('window.__updaterPainted')
        from PIL import ImageGrab
        origin=client.command('plugin:window|inner_position',label=label)
        deadline=time.monotonic()+3
        actual=None
        while time.monotonic()<deadline:
            fill=client.js('(()=>{const e=document.querySelector(".progress-zalivka"),r=e.getBoundingClientRect();return {x:r.x+Math.min(10,r.width/2),y:r.y+r.height/2,k:devicePixelRatio||1,color:getComputedStyle(e).backgroundColor,width:r.width}})()')
            expected=tuple(round(float(v)) for v in re.findall(r'[0-9.]+',fill['color'])[:3])
            assert fill['width']>20 and len(expected)==3, 'Progress fill has no measurable painted area'
            point=(round(origin['x']+fill['x']*fill['k']),round(origin['y']+fill['y']*fill['k']))
            actual=ImageGrab.grab(xdisplay=device.display).getpixel(point)[:3]
            if actual==expected:break
            time.sleep(.05)
        assert actual==expected, 'Native progress fill is not visible: '+str(actual)+' versus '+str(expected)
        checks.append({'native_updater_progress_pixels':True,'rgb':list(actual)})
        visible_cef(client, device, 'updater-streaming-real-cef-visible')
        status = client.js('({received:window.__updaterNative.received,install:window.__updaterNative.install})')
        assert status['received'] > 0 and not status['install'], 'Download finished before native progress visibility proof'
        client.wait('window.__updaterNative.install&&window.__updaterNative.restartRequests===1&&!document.querySelector(".polosa-progress")&&document.body.innerText.includes("fixture installation failure")', timeout=25)
        status = client.js('(()=>{const s=window.__updaterNative;return {received:s.received,total:s.total,checks:s.checks,downloads:s.downloads,prepared:s.prepared,prepare_requests:s.prepareRequests,restart_requests:s.restartRequests,show_after_prepared:s.showAfterPrepared,order:s.order,errors:s.errors}})()')
        validate_adapter_status(status)
        checks.append({'native_updater_install_failure_requests_restart_without_reopen':True,
                       'prepare_and_restart':'explicit_test_adapters',
                       'actual_drain_proven':False,'actual_restart_performed':False,
                       'restart_requests':status['restart_requests'],
                       'lifecycle_order':status['order'],
                       'show_after_prepared':status['show_after_prepared']})
        checks.append({'native_updater_bundled_app_physical_button_and_real_progress': True,
                       'transport': 'real_local_http_stream',
                       'updater_ipc_confirmation_os_installer_prepare_restart': 'test_adapters',
                       'download_bytes': status['received']})
    except Exception:
        checks.append({'native_updater_diagnostic':client.js('(()=>{const s=window.__updaterNative;return {checks:s?.checks,downloads:s?.downloads,received:s?.received,install:s?.install,prepare_requests:s?.prepareRequests,restart_requests:s?.restartRequests,show_after_prepared:s?.showAfterPrepared,order:s?.order,errors:s?.errors,progress:!!document.querySelector(".polosa-progress"),focused:document.hasFocus()}})()')})
        raise
    finally:
        try:
            client.js('window.__updaterNative?.cleanup();delete window.__updaterNative;delete window.__updaterPainted;true')
            # Lifecycle/install guards deliberately remain until Device teardown,
            # including on an early progress/assertion failure.
            # TEST CLEANUP ONLY: prepare was an adapter, so the real engines
            # remain alive. This is deliberately not shipped updater behavior.
            client.command('pokazat_dvizhki',vidno=True)
            visible_cef(client,device,'updater-adapter-test-cleanup-cef-restored')
            checks.append({'updater_adapter_test_cleanup_restored_cef':True,
                           'scope':'cleanup_after_adapted_prepare_not_product_reopen_proof'})
        finally:
            server.shutdown()
            server.server_close()


def validate_adapter_status(status):
    assert status['received']==status['total']>0, 'HTTP stream did not complete'
    assert status['checks']==status['downloads']==status['prepare_requests']==status['restart_requests']==1, 'Updater lifecycle adapter count mismatch'
    assert status['prepared'] is True and status['show_after_prepared']==0 and status['errors']==[], 'Updater reopened after prepared or adapter contract failed'
    order=status['order']
    for event in ('check','confirmation','download','download_finished','prepare','install_failed','restart'):
        assert order.count(event)==1, 'Updater lifecycle stage repeated or missing'
    assert [order.index(event) for event in ('check','confirmation','download','download_finished','prepare','install_failed','restart')]==sorted(order.index(event) for event in ('check','confirmation','download','download_finished','prepare','install_failed','restart')), 'Updater lifecycle stage order mismatch'
    assert 'show' not in order[order.index('prepare')+1:], 'Updater showed CEF after prepare'



def adapter_offline_self_test():
    """Execute the actual shim with mock transport; no browser, network or native IPC."""
    import ast
    from pathlib import Path
    tree=ast.parse(Path(__file__).read_text(encoding='utf-8'))
    function=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='verify_updater')
    assignment=next(n for n in function.body if isinstance(n,ast.Assign)
                    and any(isinstance(t,ast.Name) and t.id=='adapter' for t in n.targets))
    adapter=assignment.value.func.value.value.replace('original(URL,','original("fixture://stream",')
    script=r'''
    const assert=(ok)=>{if(!ok)throw new Error('offline_adapter_contract')};
    const raw=ADAPTER;
    function boot(){
      const forwarded=[];
      global.window={__TAURI_INTERNALS__:{convertFileSrc:c=>'ipc:'+c,runCallback:()=>{}},fetch:async(input)=>{
        const url=typeof input==='string'?input:input.url;forwarded.push(url);
        if(['ipc:app_prepare_update','ipc:app_restart','ipc:plugin:updater|install'].includes(url))throw new Error('unsafe_native_fallthrough');
        return url==='fixture://stream'?new Response(new Uint8Array(32),{headers:{'Content-Length':'32'}}):new Response('null');
      }};
      eval('(()=>{'+raw+'})()');
      return {forwarded,call:(command,args={})=>window.fetch('ipc:'+command,{body:JSON.stringify(args)})};
    }
    (async()=>{
      let h=boot();
      assert((await h.call('app_prepare_update')).headers.get('Tauri-Response')==='error');
      assert((await h.call('app_restart')).headers.get('Tauri-Response')==='error');
      assert(h.forwarded.length===0);
      h=boot();
      await h.call('plugin:updater|check');await h.call('pokazat_dvizhki',{vidno:false});
      await h.call('plugin:dialog|message',{title:'Обновление NexusBro'});await h.call('pokazat_dvizhki',{vidno:true});
      await h.call('plugin:updater|download',{onEvent:'__CHANNEL__:1'});await h.call('pokazat_dvizhki',{vidno:false});
      assert((await h.call('app_prepare_update')).headers.get('Tauri-Response')==='ok');
      assert((await h.call('plugin:updater|install')).headers.get('Tauri-Response')==='error');
      assert((await h.call('app_restart')).headers.get('Tauri-Response')==='ok');
      const s=window.__updaterNative;
      const status={received:s.received,total:s.total,checks:s.checks,downloads:s.downloads,prepared:s.prepared,
        prepare_requests:s.prepareRequests,restart_requests:s.restartRequests,show_after_prepared:s.showAfterPrepared,order:[...s.order],errors:[...s.errors]};
      assert(h.forwarded.filter(x=>x==='fixture://stream').length===1);
      const prior=h.forwarded.length;s.cleanup();
      for(const cmd of ['app_prepare_update','app_restart','plugin:updater|install','plugin:updater|download'])
        assert((await h.call(cmd)).headers.get('Tauri-Response')==='error');
      assert(h.forwarded.length===prior);
      await h.call('pokazat_dvizhki',{vidno:true});assert(h.forwarded.length===prior+1);
      console.log(JSON.stringify(status));
    })().catch(()=>{console.error('offline_adapter_failed');process.exitCode=1});
    '''.replace('ADAPTER',json.dumps(adapter))
    result=subprocess.run(['node','-'],input=script,text=True,capture_output=True,timeout=5)
    assert result.returncode==0 and len(result.stdout)<8192,'Offline updater adapter failed'
    validate_adapter_status(json.loads(result.stdout))


def self_test():
    adapter_offline_self_test()
    good=dict(received=100,total=100,checks=1,downloads=1,prepared=True,prepare_requests=1,
              restart_requests=1,show_after_prepared=0,errors=[],
              order=['check','hide','confirmation','show','download','download_finished','hide','prepare','install_failed','restart'])
    validate_adapter_status(good)
    bad=[]
    for field,value in [('restart_requests',0),('restart_requests',2),('prepare_requests',0),('show_after_prepared',1),('errors',['prepare_order'])]:
        bad.append(dict(good,**{field:value}))
    bad.append(dict(good,order=good['order']+['show']))
    bad.append(dict(good,order=['check','confirmation','download','prepare','download_finished','install_failed','restart']))
    for value in bad:
        try:validate_adapter_status(value)
        except AssertionError:pass
        else:raise AssertionError('Unsafe updater lifecycle accepted')
    return dict(self_test=True,synthetic_only=True,native_execution=False,actual_drain_proven=False)


if __name__=='__main__':
    import sys
    assert sys.argv[1:]==['--self-test'], 'Only the offline self-test is available directly'
    print(json.dumps(self_test()))
