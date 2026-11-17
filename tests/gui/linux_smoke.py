"""Реальный CI-бинарник под Xvfb: процесс CEF, документ, два окна и две Super."""
import argparse,http.server,json,os,pathlib,signal,subprocess,threading,time
from client import Client
p=argparse.ArgumentParser();p.add_argument('--exe',required=True);p.add_argument('--cef',required=True);p.add_argument('--out',required=True);a=p.parse_args()
root=pathlib.Path(a.out).resolve();root.mkdir(parents=True,exist_ok=False);exe=pathlib.Path(a.exe).resolve();cef=pathlib.Path(a.cef).resolve();checks=[]
class Page(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  body=b'<!doctype html><title>LinuxNativeSmoke</title><style>body{background:#185541}</style><p>CEF is alive</p>'
  self.send_response(200);self.send_header('Content-Type','text/html');self.end_headers();self.wfile.write(body)
 def log_message(self,*a):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Page);threading.Thread(target=server.serve_forever,daemon=True).start();url=f'http://127.0.0.1:{server.server_port}/page'
env={**os.environ,'DISPLAY':':96','CEF_PATH':str(cef),'LD_LIBRARY_PATH':str(cef),'NEXUSBRO_TEST_DATA':str(root/'data'),'NEXUSBRO_PROBA':'0','WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS':'1','WEBKIT_INSPECTOR_HTTP_SERVER':'127.0.0.1:9236'}
x=None;app=None;c=None;c2=None
try:
 x=subprocess.Popen(['Xvfb',':96','-screen','0','1280x900x24','-nolisten','tcp'],stdout=(root/'xvfb.log').open('w'),stderr=subprocess.STDOUT);time.sleep(.4)
 app=subprocess.Popen(['dbus-run-session','--',str(exe)],env=env,stdout=(root/'app.log').open('w'),stderr=subprocess.STDOUT)
 deadline=time.monotonic()+30
 while time.monotonic()<deadline:
  assert app.poll() is None,'Application exited'
  try:c=Client(display=':96',port=9236);break
  except (ConnectionError,OSError):time.sleep(.2)
 assert c,'No WebKit inspector'
 c.wait('!!window.__TAURI_INTERNALS__?.invoke',timeout=15)
 def choose(client,name):
  point=client.wait('(()=>{const e=[...document.querySelectorAll(".vkladka")].find(e=>e.querySelector(".imya")?.textContent==='+json.dumps(name)+');if(!e)return null;const r=e.getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]})()')
  client.mouse(*point);client.wait('document.querySelector(".vkladka.aktivna .imya")?.textContent==='+json.dumps(name))
 def loaded(client,id):
  deadline=time.monotonic()+20
  while time.monotonic()<deadline:
   rows=client.command('spisok')
   if not next(r for r in rows if r['id']==id)['zapushchena']:time.sleep(.1);continue
   session=client.command('obychnye_spisok',id=id)
   if any(t['zagolovok']=='LinuxNativeSmoke' and not t['zagruzka'] for t in session['vkladki']):return session
   time.sleep(.1)
  raise AssertionError(('Document not loaded',session))
 for id,name in [('linux-a','LinuxA'),('linux-b','LinuxB')]:c.command('otkryt',nastrojki={'id':id,'nazvanie':name,'proxy':'','adres':url,'rezat_reklamu':True,'pustaya':False,'privatnaya':False});loaded(c,id)
 choose(c,'LinuxA');loaded(c,'linux-a');checks.append({'native_cef_document_loaded':True})
 c.command('novoe_okno',id='linux-a',privatnaya=False);deadline=time.monotonic()+20
 while time.monotonic()<deadline:
  try:c2=Client(display=':96',port=9236,page_index=1);break
  except (ConnectionError,OSError):time.sleep(.2)
 assert c2,'No secondary window';c2.wait('document.querySelectorAll(".vkladka").length===2')
 def navigate(client,id):
  session=client.command('obychnye_spisok',id=id);client.command('obychnaya_komanda',id=id,komanda={'deystvie':'pereyti','id':session['aktivnaya'],'adres':url});loaded(client,id)
 choose(c2,'LinuxB');navigate(c2,'linux-b');choose(c2,'LinuxA');navigate(c2,'linux-a');choose(c2,'LinuxB');loaded(c2,'linux-b');assert c.command('aktivnaya')=='linux-a'
 checks.append({'secondary_super_switch_and_main_unchanged':True})
 c2.command('fokus_obolochke');c2.click('[aria-label="Меню NexusBro"]');c2.text_button('Настройки NexusBro');c2.wait('!!document.querySelector(".nastrojki-okno")');c2.click('[aria-label="Закрыть настройки"]');checks.append({'secondary_local_settings_menu':True})
 c.command('zakryt',id='linux-b',sohranit=False);assert app.poll() is None;loaded(c,'linux-a');checks.append({'delete_other_super_keeps_windows':True})
 from PIL import ImageGrab
 ImageGrab.grab(xdisplay=':96').save(root/'windows.png')
 checks.append({'success':True});print(json.dumps(checks))
finally:
 (root/'results.json').write_text(json.dumps(checks,indent=2)+'\n')
 if c2:c2.close()
 if c:c.close()
 if app:
  import psutil
  try:process=psutil.Process(app.pid);children=process.children(recursive=True)
  except psutil.NoSuchProcess:children=[]
  for child in reversed(children):
   try:child.terminate()
   except psutil.NoSuchProcess:pass
  app.terminate()
  try:app.wait(timeout=5)
  except subprocess.TimeoutExpired:app.kill()
  _,alive=psutil.wait_procs(children,timeout=3)
  for child in alive:
   try:child.kill()
   except psutil.NoSuchProcess:pass
 if x:x.terminate();x.wait(timeout=5)
 server.shutdown()
