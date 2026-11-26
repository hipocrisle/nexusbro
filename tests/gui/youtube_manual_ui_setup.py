"""Bounded operator-supervised native UI diagnostic; never paired acceptance."""
import json,os,re,stat,subprocess,time
from pathlib import Path
from security_native import call
from youtube_paired_native import Blocked,focus_normal_cef
from youtube_player_native import native_app_identity,native_app_window,native_app_identity_alive
def manual_setup(client,device,out,index,result):
 from PIL import ImageGrab
 identity=native_app_identity(device);window=native_app_window(device,identity)
 env={**os.environ,'DISPLAY':device.display}
 directory=Path(out)/('manual-'+str(index));directory.mkdir(mode=0o700)
 def crop():
  native_app_window(device,identity)
  facts=subprocess.check_output(['xwininfo','-id',str(window)],env=env,text=True,timeout=3)
  def number(key):
   found=re.search(r'^\s*'+re.escape(key)+r':\s*(-?\d+)',facts,re.M)
   if not found:raise Blocked('manual_window_geometry_unavailable')
   return int(found.group(1))
  x,y=number('Absolute upper-left X'),number('Absolute upper-left Y')
  w,h=number('Width'),number('Height')
  if min(x,y)<0 or not(300<=w<=4096 and 200<=h<=2160):raise Blocked('manual_window_bounds')
  with ImageGrab.grab(bbox=(x,y,x+w,y+h),xdisplay=device.display) as image:image.save(directory/'owned.png')
  return x,y,w,h
 def inside(x,y):
  label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
  origin=call(client,'plugin:window|inner_position',label=label)
  r=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height,k:devicePixelRatio||1}})()')
  return origin['x']+r['x']*r['k']<=x<origin['x']+(r['x']+r['w'])*r['k'] and origin['y']+r['y']*r['k']<=y<origin['y']+(r['y']+r['h'])*r['k']
 time.sleep(3);bounds=crop()
 (directory/'ready.json').write_text(json.dumps({'ready':True,'diagnostic_only':True,'actions':0}))
 deadline=time.monotonic()+240;count=0;last_capture=0
 while time.monotonic()<deadline:
  request=directory/'command.json'
  if not request.exists():
   if time.monotonic()-last_capture>1:
    bounds=crop();last_capture=time.monotonic()
    (directory/'ready.json').write_text(json.dumps({'ready':True,'diagnostic_only':True,'actions':count,'crop_origin':list(bounds[:2]),'coordinates':'relative_to_owned_crop'}))
   time.sleep(.2);continue
  mode=request.lstat()
  if not stat.S_ISREG(mode.st_mode) or mode.st_uid!=os.getuid() or stat.S_IMODE(mode.st_mode)!=0o600 or mode.st_size>1024:raise Blocked('manual_command_file_invalid')
  command=json.loads(request.read_text());request.unlink()
  if command=={'action':'done'}:
   result['manual_ui_actions']=count;return
  if count>=12:raise Blocked('manual_action_limit')
  x,y=command.get('x'),command.get('y')
  if type(x)is not int or type(y)is not int:raise Blocked('manual_coordinate_type')
  x+=bounds[0];y+=bounds[1]
  if not inside(x,y) or not native_app_identity_alive(device,identity):raise Blocked('manual_target_invalid')
  focus_normal_cef(client,device)
  action=command.get('action')
  if action=='click' and set(command)=={'action','x','y'}:tail=['click','1']
  elif action=='scroll' and set(command)=={'action','x','y','down'} and type(command['down'])is int and 1<=command['down']<=8:tail=['click','--repeat',str(command['down']),'--delay','100','5']
  else:raise Blocked('manual_action_invalid')
  subprocess.run(['xdotool','mousemove',str(x),str(y),*tail],env=env,check=True,timeout=5,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  count+=1;time.sleep(1);bounds=crop()
  (directory/'ready.json').write_text(json.dumps({'ready':True,'diagnostic_only':True,'actions':count}))
 raise Blocked('manual_ui_deadline')
