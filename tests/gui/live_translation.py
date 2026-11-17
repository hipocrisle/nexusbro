"""Manual real-service gate: CI binary -> CEF snapshot -> Super proxy -> Google -> DOM.

Only the short synthetic German fixture is sent. No user profile or page is used.
Not part of deterministic CI: requires an explicitly supplied existing proxy.
"""
import argparse,http.server,json,os,pathlib,re,subprocess,threading,time,urllib.parse
from client import Client

p=argparse.ArgumentParser();p.add_argument('--exe',required=True);p.add_argument('--cef',required=True)
p.add_argument('--proxy',required=True);p.add_argument('--out',required=True);p.add_argument('--display',default=':98');p.add_argument('--port',type=int,default=9248)
a=p.parse_args();root=pathlib.Path(a.out).resolve();root.mkdir(parents=True,exist_ok=False)
reports=[];checks=[];app=None;x=None;c=None
class Page(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  if self.path.startswith('/report?'):
   reports.append(urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query));self.send_response(204);self.end_headers();return
  body='''<!doctype html><html lang="de"><title>LiveGermanFixture</title><main><h1>Guten Tag</h1><p>Berlin ist die Hauptstadt von Deutschland.</p><p>Dies ist ein kurzer Text für einen Übersetzungstest.</p></main><input value="PRIVATE FORM MUST NOT BE SENT"><script>
const report=()=>fetch('/report?'+new URLSearchParams({path:location.pathname,text:document.querySelector('main').innerText}));
new MutationObserver(report).observe(document.querySelector('main'),{subtree:true,characterData:true,childList:true});report();</script></html>'''.encode()
  self.send_response(200);self.send_header('Content-Type','text/html;charset=utf-8');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Page);threading.Thread(target=server.serve_forever,daemon=True).start()
base=f'http://127.0.0.1:{server.server_port}'

def translated(path):
 deadline=time.monotonic()+40
 while time.monotonic()<deadline:
  assert app.poll() is None,'Browser exited'
  rows=[r for r in reports if r.get('path')==[path] and re.search('[А-Яа-яЁё]',r.get('text',[''])[0])]
  if rows:
   status=c.js('document.querySelector(".translation-bar")?.innerText||""')
   m=re.search(r'(\d+) из (\d+)',status)
   if m and m[1]==m[2]:return rows[-1],status
  time.sleep(.15)
 raise AssertionError(('No complete translated DOM',path,c.js('document.body.innerText')))

try:
 env={**os.environ,'DISPLAY':a.display,'CEF_PATH':str(pathlib.Path(a.cef).resolve()),'LD_LIBRARY_PATH':str(pathlib.Path(a.cef).resolve()),
      'NEXUSBRO_TEST_DATA':str(root/'data'),'NEXUSBRO_PROBA':'0','WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS':'1','WEBKIT_INSPECTOR_HTTP_SERVER':f'127.0.0.1:{a.port}'}
 env.pop('NEXUSBRO_TEST_TRANSLATE_URL',None)
 x=subprocess.Popen(['Xvfb',a.display,'-screen','0','1280x900x24','-nolisten','tcp'],stdout=(root/'xvfb.log').open('w'),stderr=subprocess.STDOUT);time.sleep(.4)
 app=subprocess.Popen(['dbus-run-session','--',str(pathlib.Path(a.exe).resolve())],env=env,stdout=(root/'app.log').open('w'),stderr=subprocess.STDOUT)
 deadline=time.monotonic()+30
 while time.monotonic()<deadline:
  assert app.poll() is None,'Application exited'
  try:c=Client(display=a.display,port=a.port);break
  except (ConnectionError,OSError):time.sleep(.2)
 assert c,'No inspector';c.wait('!!window.__TAURI_INTERNALS__?.invoke')
 c.command('otkryt',nastrojki={'id':'live-translation','nazvanie':'Synthetic German','proxy':a.proxy,'adres':base+'/one','rezat_reklamu':True,'pustaya':False,'privatnaya':False})
 deadline=time.monotonic()+25;session=None
 while time.monotonic()<deadline:
  try:session=c.command('obychnye_spisok',id='live-translation')
  except AssertionError:time.sleep(.2);continue
  if session['vkladki'] and not session['vkladki'][0]['zagruzka'] and session['vkladki'][0]['zagolovok']=='LiveGermanFixture':break
  time.sleep(.2)
 assert session;tab=session['aktivnaya'];start=time.monotonic()
 c.command('obychnaya_komanda',id='live-translation',komanda={'deystvie':'perevesti','id':tab})
 report,status=translated('/one');checks.append({'real_google_through_native_super_proxy':True,'initialSeconds':round(time.monotonic()-start,3),'status':status})
 c.command('obychnaya_komanda',id='live-translation',komanda={'deystvie':'perevod_avto','id':tab,'enabled':True})
 c.command('obychnaya_komanda',id='live-translation',komanda={'deystvie':'pereyti','id':tab,'adres':base+'/two'})
 start=time.monotonic();report,status=translated('/two');checks.append({'auto_translation_after_real_cef_navigation':True,'seconds':round(time.monotonic()-start,3)})
 c.command('obychnaya_komanda',id='live-translation',komanda={'deystvie':'perevod_original','id':tab})
 deadline=time.monotonic()+8
 while time.monotonic()<deadline and not any(r.get('path')==['/two'] and r.get('text',[''])[0].startswith('Guten Tag') for r in reports):time.sleep(.1)
 assert any(r.get('path')==['/two'] and r.get('text',[''])[0].startswith('Guten Tag') for r in reports)
 c.command('obychnaya_komanda',id='live-translation',komanda={'deystvie':'pereyti','id':tab,'adres':base+'/three'})
 deadline=time.monotonic()+10
 while time.monotonic()<deadline and not any(r.get('path')==['/three'] for r in reports):time.sleep(.1)
 assert any(r.get('path')==['/three'] for r in reports),'Third document did not load'
 time.sleep(3);assert not any(r.get('path')==['/three'] and re.search('[А-Яа-яЁё]',r.get('text',[''])[0]) for r in reports)
 assert c.js('!document.querySelector(".translation-bar")'),'Auto mode unexpectedly restarted'
 checks.append({'original_restored_and_auto_cancelled':True,'synthetic_text_only':True,'success':True})
 print(json.dumps(checks,ensure_ascii=False))
finally:
 (root/'results.json').write_text(json.dumps(checks,ensure_ascii=False,indent=2)+'\n')
 if c:c.close()
 if app:
  import psutil
  try:children=psutil.Process(app.pid).children(recursive=True)
  except psutil.NoSuchProcess:children=[]
  for child in reversed(children):
   try:child.terminate()
   except psutil.NoSuchProcess:pass
  app.terminate()
  try:app.wait(timeout=5)
  except subprocess.TimeoutExpired:app.kill();app.wait(timeout=5)
  _,alive=psutil.wait_procs(children,timeout=3)
  for child in alive:
   try:child.kill()
   except psutil.NoSuchProcess:pass
 if x:x.terminate();x.wait(timeout=5)
 server.shutdown()
