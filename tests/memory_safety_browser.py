"""Sequential real Chromium checks of the exact renderer safety closure.

The probe handle is exposed ONLY by this fixture, never by production code.
Does not prove CEF early-context timing, native sender authentication or freezing;
those require the native integration suite. No real sites or credentials.
"""
import argparse
import base64
import io
import json
import os
import pathlib
import threading
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from playwright.sync_api import sync_playwright
ROOT=pathlib.Path(__file__).resolve().parents[1]
SCRIPT=(ROOT/'desktop/src-tauri/scripts/memory_safety.js').read_text()
wave_buffer=io.BytesIO()
with wave.open(wave_buffer,'wb') as audio:
    audio.setnchannels(1);audio.setsampwidth(2);audio.setframerate(8000);audio.writeframes(b'\0\0'*24000)
WAV='data:audio/wav;base64,'+base64.b64encode(wave_buffer.getvalue()).decode()
class Fixture(BaseHTTPRequestHandler):
    def do_GET(self):
        body=b'<!doctype html><meta charset="utf-8"><style>html,body{margin:0;min-height:100%;background:#185541}p{margin:20px}</style><p>Static article without interactive controls</p>'
        self.send_response(200);self.send_header('Content-Type','text/html; charset=utf-8');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    def log_message(self,*args):pass

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output');args=parser.parse_args()
    server=ThreadingHTTPServer(('127.0.0.1',0),Fixture)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    reports=[]
    try:
        with sync_playwright() as pw:
            browser=pw.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None,
                headless=True,args=['--no-sandbox','--autoplay-policy=no-user-gesture-required'])
            try:
                def run(name,action=None,blocked=True,unknown=False):
                    context=browser.new_context()
                    # Test-only native-handle stand-in. Production stores the
                    # function as V8Value and never writes a global property.
                    context.add_init_script('const __walker=Document.prototype.createTreeWalker;Document.prototype.createTreeWalker=function(...a){window.__memoryWalksForTest=(window.__memoryWalksForTest||0)+1;return Reflect.apply(__walker,this,a);};Object.defineProperty(window,"__memoryProbeForTest",{value:('+SCRIPT+'),configurable:false});')
                    page=context.new_page()
                    try:
                        page.goto(f'http://127.0.0.1:{server.server_port}/page',wait_until='load')
                        initial=page.evaluate('JSON.parse(__memoryProbeForTest())')
                        assert initial['known'] and initial['reason'] is None,(name,'initial',initial)
                        details=action(page) if action else None
                        result=page.evaluate('JSON.parse(__memoryProbeForTest())')
                        if unknown:assert result['known'] is False,(name,result)
                        elif blocked:assert result['known'] and result['reason'],(name,result)
                        else:assert result['known'] and result['reason'] is None,(name,result)
                        reports.append({'case':name,'result':result,'details':details})
                    finally:context.close()
                def run_opaque(name,owned=False,action=None,url='about:blank',known=False,blocked=False):
                    context=browser.new_context()
                    # Match the native builder's lexical literal substitution;
                    # production never accepts this flag from a page/global.
                    script=SCRIPT.replace('/*NB_NATIVE_OWNED_BLANK*/false','true' if owned else 'false')
                    context.add_init_script('Object.defineProperty(window,"__memoryProbeForTest",{value:('+script+'),configurable:false});')
                    page=context.new_page()
                    try:
                        page.goto(url,wait_until='load')
                        actual=page.evaluate("""()=>{let storageError=null;try{void sessionStorage;}catch(e){storageError=e.name;}return {url:document.URL,origin:origin,storage_error:storageError};}""")
                        assert actual['origin']=='null' and actual['storage_error']=='SecurityError',(name,actual)
                        if action:action(page)
                        result=page.evaluate('JSON.parse(__memoryProbeForTest())')
                        assert result['known'] is known,(name,result)
                        if known:assert bool(result['reason']) is blocked,(name,result)
                        reports.append({'case':name,'result':result,'details':actual})
                    finally:context.close()
                run_opaque('ordinary_opaque_blank_stays_unknown')
                run_opaque('native_owned_opaque_blank',owned=True,known=True)
                run_opaque('opaque_data_document_is_not_owned_blank',owned=True,url='data:text/html,<p>Opaque data document</p>')
                run_opaque('owned_blank_fake_security_error_getter_unknown',owned=True,action=lambda p:p.evaluate("Object.defineProperty(window,'sessionStorage',{get(){throw {name:'SecurityError'};},configurable:true})"))
                run_opaque('owned_blank_genuine_security_error_getter_tamper_unknown',owned=True,action=lambda p:p.evaluate("Object.defineProperty(window,'sessionStorage',{get(){throw new DOMException('synthetic','SecurityError');},configurable:true})"))
                run_opaque('owned_blank_web_audio_protected',owned=True,action=lambda p:web_audio(p),known=True,blocked=True)
                run_opaque('owned_blank_input_protected',owned=True,action=lambda p:(p.evaluate("const e=document.createElement('input');document.body.append(e);e.focus()"),p.keyboard.type('synthetic draft')),known=True,blocked=True)
                run('static_article',blocked=False)
                run('synthetic_click_not_trusted',lambda p:p.evaluate("window.dispatchEvent(new MouseEvent('click',{bubbles:true}))"),blocked=False)
                run('trusted_pointer_click',lambda p:p.mouse.click(8,8))
                run('trusted_edit',lambda p:(p.evaluate("const e=document.createElement('input');document.body.append(e);e.focus()"),p.keyboard.type('synthetic draft')))
                run('media_element_playing',lambda p:media_play(p))
                run('web_audio_playing',lambda p:web_audio(p))
                run('peer_connection',lambda p:p.evaluate("const p=new RTCPeerConnection({iceServers:[]});const s=p.signalingState;p.close();s"))
                run('worker_created_then_terminated',lambda p:p.evaluate("""async()=>{const w=new Worker('data:text/javascript,onmessage=()=>postMessage("ack")');const done=new Promise(r=>w.onmessage=e=>r(e.data));w.postMessage('test');const v=await done;w.terminate();return v;}"""))
                run('transient_iframe',lambda p:p.evaluate("const e=document.createElement('iframe');document.body.append(e);e.remove()"))
                run('parser_transient_iframe',lambda p:p.evaluate("document.body.innerHTML='<section><iframe></iframe></section>';document.body.innerHTML=''"))
                run('closed_shadow_root',lambda p:p.evaluate("const e=document.createElement('div');document.body.append(e);e.attachShadow({mode:'closed'});e.remove()"))
                run('canvas_created_then_removed',lambda p:p.evaluate("const e=document.createElement('canvas');e.getContext('2d');e.remove()"))
                run('beforeunload_listener',lambda p:p.evaluate("window.addEventListener('beforeunload',()=>{})"))
                run('beforeunload_inline_body_attribute',lambda p:p.evaluate("document.body.setAttribute('onbeforeunload','return 1')"))
                run('speech_synthesis_request',lambda p:p.evaluate("speechSynthesis.speak(new SpeechSynthesisUtterance('synthetic test'));speechSynthesis.cancel()"))
                run('beforeunload_property',lambda p:p.evaluate("window.onbeforeunload=()=>{}"))
                run('session_storage',lambda p:p.evaluate("sessionStorage.setItem('synthetic','state')"))
                run('constructor_preserves_new_target',lambda p:p.evaluate("const C=class extends AudioContext{};const a=new C();const valid=a instanceof C&&a instanceof AudioContext&&a.constructor===C&&AudioContext.name==='AudioContext';a.close();if(!valid)throw Error('Constructor transparency');valid"))
                run('constructor_alias',lambda p:p.evaluate("const c=new AudioContext.prototype.constructor();c.close()"))
                run('poisoned_object_toJSON_cannot_forge_safe',lambda p:p.evaluate("Object.prototype.toJSON=function(){return {known:true,reason:null}};const c=new AudioContext();c.close()"))
                run('hook_replacement_unknown',lambda p:p.evaluate("window.Worker=function(){}"),unknown=True)
                run('document_own_shadow_unknown',lambda p:p.evaluate("document.createElement=function(){}"),unknown=True)
                run('large_dom_budget_unknown',lambda p:p.evaluate("const r=document.createElement('section');for(let i=0;i<5000;i++)r.append(document.createElement('span'));document.body.append(r)"),unknown=True)
                run('small_batches_exhaust_lifetime_budget',lambda p:p.evaluate("async()=>{for(let i=0;i<180;i++){const e=document.createElement('span');document.body.append(e);e.remove();await Promise.resolve();}}"),unknown=True)
                run('protected_mutations_no_tree_scans',lambda p:protected_cost(p))

            finally:browser.close()
    finally:server.shutdown();server.server_close()
    result={'checks':reports,'count':len(reports),'scope':'real Chromium renderer JS only; CEF process-message, early-hook and freeze integration require native tests'}
    if args.output:pathlib.Path(args.output).write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps(result,ensure_ascii=False))

def protected_cost(page):
    page.evaluate("const a=new AudioContext();a.close()")
    before=page.evaluate('window.__memoryWalksForTest')
    page.evaluate("for(let i=0;i<300;i++){const e=document.createElement('span');document.body.append(e);e.remove()}")
    page.wait_for_timeout(30)
    after=page.evaluate('window.__memoryWalksForTest')
    assert after==before, (before,after)
    return {'tree_walks_before':before,'tree_walks_after':after}

def actual_clock_advance(page, kind, baseline=None):
    # Observe real renderer clocks; never replay, seek, or synthesize time.
    if kind == 'media':
        state = "(()=>{const a=__actualMedia;return {paused:a.paused,ended:a.ended,ready:a.readyState,network:a.networkState,error:a.error?a.error.code:null,time:a.currentTime,duration:a.duration};})()"
        predicate = """base=>{const a=__actualMedia,d=a.duration,t=a.currentTime;
            if(a.paused||a.ended||a.error||a.readyState<2||!Number.isFinite(d)||d<=0||!Number.isFinite(t))return false;
            return base===null?t>0.01:((t-base+d)%d)>0.01;}"""
    else:
        state = "({state:__actualAudio.state,time:__actualAudio.currentTime})"
        predicate = "base=>__actualAudio.state==='running'&&Number.isFinite(__actualAudio.currentTime)&&__actualAudio.currentTime>(base===null?0:base)+0.01"
    try:
        page.wait_for_function(predicate,arg=baseline,timeout=10000,polling=25)
    except Exception:
        # Fixed scalar diagnostics only: no source URI or browser exception text.
        raise AssertionError(('real_clock_did_not_advance',kind,page.evaluate(state))) from None
    return page.evaluate(state)

def playback_guard(page):
    result=page.evaluate('JSON.parse(__memoryProbeForTest())')
    assert result['known'] and result['reason'],('playback_not_protected',result)
    return result

def web_audio(page):
    page.evaluate('''async()=>{const a=new AudioContext();const gain=a.createGain();gain.gain.value=0;gain.connect(a.destination);const o=a.createOscillator();o.connect(gain);o.start();await a.resume();window.__actualAudio=a;}''')
    startup=actual_clock_advance(page,'audio')
    before=actual_clock_advance(page,'audio',startup['time'])
    guard=playback_guard(page)
    after=actual_clock_advance(page,'audio',before['time'])
    return {'startup':startup,'before':before,'guard':guard,'after':after}

def media_play(page):
    page.evaluate('''async src=>{const a=document.createElement('audio');a.src=src;a.loop=true;document.body.append(a);await a.play();window.__actualMedia=a;}''',WAV)
    startup=actual_clock_advance(page,'media')
    before=actual_clock_advance(page,'media',startup['time'])
    guard=playback_guard(page)
    after=actual_clock_advance(page,'media',before['time'])
    return {'startup':startup,'before':before,'guard':guard,'after':after}

if __name__=='__main__':main()
