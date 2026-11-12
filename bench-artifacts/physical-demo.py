import json,subprocess,time,websocket,pathlib,os
ws=websocket.create_connection('ws://127.0.0.1:9223/socket/1/1/WebPage',timeout=5)
seq=0
log=[]
def js(expr):
 global seq
 seq+=1
 ws.send(json.dumps({'id':seq,'method':'Target.sendMessageToTarget','params':{'targetId':'page-7','message':json.dumps({'id':seq,'method':'Runtime.evaluate','params':{'expression':expr,'returnByValue':True}})}}))
 while True:
  m=json.loads(ws.recv())
  if m.get('method')=='Target.dispatchMessageFromTarget':
   r=json.loads(m['params']['message'])
   if r.get('id')==seq:return r['result']['result'].get('value')
def click(expr,button=1):
 point=js('(()=>{let e='+expr+';let r=e.getBoundingClientRect();return [Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)]})()')
 subprocess.run(['xdotool','mousemove',*map(str,point),'click',str(button)],check=True,env={**os.environ,'DISPLAY':':92'})
 time.sleep(.4)
 log.append({'clicked':expr,'button':button})
click('document.querySelector(".vkladka")',3)
click('Array.from(document.querySelectorAll(".stroka-menyu")).find(x=>x.textContent.trim()==="Настройки")')
click('document.querySelector(".dialog input")')
subprocess.run(['xdotool','key','ctrl+a','type','Physical edited'],check=True,env={**os.environ,'DISPLAY':':92'})
click('document.querySelector(".dialog .glavnaya")')
time.sleep(.7)
click('document.querySelector(".vkladka .krest")')
click('document.querySelector(".knopki button")')
assert not js('!!document.querySelector(".zatemnenie")')
click('document.querySelector(".vkladka .krest")')
click('document.querySelector(".knopki .glavnaya")')
assert not js('!!document.querySelector(".zatemnenie")')
click('document.querySelector(".niz button")')
assert js('!!document.querySelector(".dialog input")')
click('Array.from(document.querySelectorAll(".knopki button")).find(x=>x.textContent.trim()==="Отмена")')
click('document.querySelector(".niz button:nth-child(2)")')
click('document.querySelector(".stroka-menyu")')
time.sleep(.8)
assert js('document.querySelector(".imya").textContent')=='Physical edited'
assert not js('!!document.querySelector(".zatemnenie")')
log.append({'success':True,'overlay_remaining':False})
pathlib.Path('bench-artifacts/physical-demo.json').write_text(json.dumps(log,ensure_ascii=False,indent=2)+'\n')
