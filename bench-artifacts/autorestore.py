import pathlib,os,subprocess,time,json,re,websocket
root=pathlib.Path.cwd()
data=root/'bench-artifacts/acceptance-data'
env={**os.environ,'DISPLAY':':92','CEF_PATH':str(root/'desktop/src-tauri/cef'),
 'XDG_DATA_HOME':str(data),'XDG_CONFIG_HOME':str(root/'bench-artifacts/acceptance-config'),
 'XDG_CACHE_HOME':str(root/'bench-artifacts/acceptance-cache'),
 'WEBKIT_INSPECTOR_HTTP_SERVER':'127.0.0.1:9223','WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS':'1'}
log=(root/'bench-artifacts/autorestore-app.log').open('w')
oldlog=(data/'NexusBro/zhurnal.log').read_text()
tab=json.loads((data/'NexusBro/vkladki.json').read_text())[0]
app=subprocess.Popen([str(root/'desktop/src-tauri/target/debug/nexusbro')],env=env,stdout=log,stderr=log)
result={}
try:
 time.sleep(.9)
 win=subprocess.check_output(['xdotool','search','--onlyvisible','--name','^NexusBro$'],env=env,text=True).splitlines()[0]
 subprocess.run(['xdotool','windowfocus',win],env=env,check=True)
 subprocess.run(['xdotool','windowsize',win,'1230','780'],env=env,check=True)
 started=time.monotonic()
 for _ in range(100):
  newlog=(data/'NexusBro/zhurnal.log').read_text()[len(oldlog):]
  matches=re.findall(re.escape(tab['id'])+r': запуск ок, окно (\d+)',newlog)
  if matches:break
  time.sleep(.05)
 assert matches,newlog
 startup_ms=round((time.monotonic()-started)*1000,1)
 handle=matches[-1]
 time.sleep(2)
 engines=[]
 for entry in pathlib.Path('/proc').iterdir():
  if not entry.name.isdigit():continue
  try:
   cmd=(entry/'cmdline').read_bytes().split(b'\0')
   if b'--nb-dvizhok' in cmd and any(str(data).encode() in x for x in cmd):engines.append(int(entry.name))
  except OSError:pass
 assert len(engines)==1,engines
 assert int(re.search(r'PPid:\s*(\d+)',pathlib.Path(f'/proc/{engines[0]}/status').read_text())[1])==app.pid
 info=subprocess.check_output(['xwininfo','-id',handle,'-all'],env=env,text=True)
 assert 'Map State: IsViewable' in info and '1 child:' in info,info
 subprocess.run(['xdotool','windowsize',win,'1100','700'],env=env,check=True)
 time.sleep(.4)
 resized=subprocess.check_output(['xwininfo','-id',handle,'-all'],env=env,text=True)
 width=int(re.search(r'Width:\s*(\d+)',resized)[1]); height=int(re.search(r'Height:\s*(\d+)',resized)[1])
 x=int(re.search(r'Absolute upper-left X:\s*(\d+)',resized)[1]); y=int(re.search(r'Absolute upper-left Y:\s*(\d+)',resized)[1])
 assert width+x==1100 and height+y==700,resized
 assert re.search(fr'{width}x{height}\+0\+0',resized),resized
 tree=subprocess.check_output(['xwininfo','-root','-tree'],env=env,text=True)
 (root/'bench-artifacts/autorestore-tree.txt').write_text(tree.strip()+'\n')
 result={'restored_name':tab['nazvanie'],'native_ready_after_focus_ms':startup_ms,
  'engine_alive_after_launcher_thread_exit':True,'single_engine':True,'native_mapped':True,
  'resize_verified':True}
 ws=websocket.create_connection('ws://127.0.0.1:9223/socket/1/1/WebPage',timeout=5)
 ws.send(json.dumps({'id':1,'method':'Target.sendMessageToTarget','params':{'targetId':'page-7',
  'message':json.dumps({'id':1,'method':'Runtime.evaluate','params':{'expression':'window.__TAURI_INTERNALS__.invoke("plugin:process|exit",{code:0});true'}})}}))
 app.wait(timeout=8)
 time.sleep(.5)
 remaining=[]
 for entry in pathlib.Path('/proc').iterdir():
  if not entry.name.isdigit():continue
  try:
   if any(str(data).encode() in x for x in (entry/'cmdline').read_bytes().split(b'\0')):
    remaining.append(int(entry.name))
  except OSError:pass
 assert not remaining,remaining
 result.update(exit_code=app.returncode,live_profile_processes_after_exit=remaining,success=True)
 print(json.dumps(result,ensure_ascii=False,indent=2))
finally:
 if app.poll() is None:
  app.terminate()
 log.close()
 (root/'bench-artifacts/autorestore-results.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
