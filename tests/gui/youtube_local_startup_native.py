#!/usr/bin/env python3
"""Local isolated about:blank preflight: no public media requests or readiness claim."""
import argparse,json,os,sys,time,threading
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parent))
from youtube_paired_home import BenchmarkDevice
from youtube_embed_home_probe import validate_private_root,validate_inside_root,LocalWindowManager,local_close
from youtube_paired_native import digest,super_record,switch_super,rows,click_control,wait_for,require,Blocked

def run(a):
 require(sys.platform=='linux' and os.getuid()==os.geteuid()==a.expect_uid and a.expect_uid!=0,'Local uid invalid')
 require(os.environ.get('GITHUB_ACTIONS')!='true' and not os.environ.get('WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS'),'Local sandbox required')
 root=validate_private_root(a.private_root,a.expect_uid);exe=validate_inside_root(a.exe,root,'exe');cef=validate_inside_root(a.cef,root,'cef')
 require(digest(exe)==a.exe_sha256,'Binary mismatch')
 parent=root/'out';parent.mkdir(mode=0o700,exist_ok=True);require(not parent.is_symlink(),'Output symlink');parent.chmod(0o700)
 out=parent/('startup-'+str(time.time_ns()));out.mkdir(mode=0o700)
 class Page(BaseHTTPRequestHandler):
  def do_GET(self):
   body=b'<!doctype html><meta charset=utf-8><title>Private preflight</title><p>Local page</p>';self.send_response(200);self.send_header('Content-Type','text/html; charset=utf-8');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
  def log_message(self,*args):pass
 server=ThreadingHTTPServer(('127.0.0.1',0),Page);threading.Thread(target=server.serve_forever,daemon=True).start();url='http://127.0.0.1:'+str(server.server_port)+'/'
 report={'preflight_complete':False,'public_playback_acceptance':False,'paired_acceptance':False,'stage':'startup','sandbox_preserved':True};device=wm=None
 try:
  device=BenchmarkDevice(out/'device',exe,cef);identifier='startup-blank'
  item=super_record(identifier,'Local preflight',url,proxy='')
  profile=device.data/'profili'/identifier;profile.mkdir(parents=True)
  (profile/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':url,'zagolovok':'Blank','otlozhena':True}],'aktivnaya':1,'sleduyushchaya':2}))
  for name in ('vkladki.json','sohranennye.json'):(device.data/name).write_text(json.dumps([item]))
  (device.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False,'memory_saver':False}))
  client=device.start();report['stage']='blank-native-frame'
  wm=LocalWindowManager(device,out/'wm');device.player_wm=wm
  switch_super(client,identifier)
  if client.js('!!document.querySelector(".pusto button")'):click_control(client,'.pusto button',10)
  wait_for(lambda:any(t.get('browser_id',0)>0 for t in rows(client,identifier)['vkladki']),'Native blank frame missing',30)
  local_close(client,device,report);report['preflight_complete']=True;report['stage']='complete'
 except Exception as error:
  codes={'Tauri IPC did not initialize under the preserved sandbox':'tauri_ipc_not_ready','Sandboxed native application exited during startup':'app_exited','Native blank frame missing':'blank_frame_missing','sandbox_startup_blocked':'inspector_not_ready'}
  report['failure_code']=codes.get(str(error),'startup_preflight_unavailable');report['failure_type']=type(error).__name__
 finally:
  clean=True
  for obj in (device,wm):
   if obj:
    try:obj.close()
    except Exception:clean=False
  server.shutdown();server.server_close()
  report['cleanup_completed']=clean
  if not clean:report['preflight_complete']=False
  (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
 print(json.dumps(report));return 0 if report['preflight_complete'] else 1
if __name__=='__main__':
 p=argparse.ArgumentParser(description=__doc__)
 for name in ('private-root','exe','cef','exe-sha256'):p.add_argument('--'+name,required=True)
 p.add_argument('--expect-uid',type=int,required=True)
 raise SystemExit(run(p.parse_args()))
