"""Shipped BrowserGuard with trusted keys and reordered native IPC fixtures.

This is real Chromium/frontend acceptance, not a native CEF/Win32 claim.
"""
import ast, functools, http.server, json, os, pathlib, threading
from playwright.sync_api import sync_playwright, expect
root=pathlib.Path(__file__).resolve().parents[1]
module=ast.parse((root/'tests/frontend_controls.py').read_text())
base=next(ast.literal_eval(n.value) for n in module.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='mock' for t in n.targets))
mock=base+r'''
const original=window.__TAURI_INTERNALS__.invoke;
window.__browserLock={locked:true,exists:true,revision:10};
window.__race={unlockPending:false,lockCalls:0,waiters:[],keys:[],unlockReply:null};
window.__race.emit=(locked,revision)=>{
 window.__browserLock={locked,exists:true,revision};
 const cb=window.__callbacks[window.__events['browser-lock-changed']];
 if(!cb)throw Error('Real BrowserGuard listener missing');
 cb({event:'browser-lock-changed',id:1,payload:structuredClone(window.__browserLock)});
};
window.__race.completeLocks=()=>{
 if(window.__race.waiters.length&&!window.__browserLock.locked)window.__race.emit(true,12);
 window.__race.waiters.splice(0).forEach(resolve=>resolve(structuredClone(window.__browserLock)));
};
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='browser_lock_status')return structuredClone(window.__browserLock);
 if(cmd==='browser_unlock'){
  window.__race.unlockPending=true;
  return new Promise(resolve=>window.__race.unlockReply=()=>{window.__race.unlockPending=false;resolve({locked:false,exists:true,revision:11});});
 }
 if(cmd==='browser_lock'){
  window.__race.lockCalls++;
  return new Promise(resolve=>window.__race.waiters.push(resolve));
 }
 return original(cmd,args);
};
window.addEventListener('keydown',e=>{
 if(e.ctrlKey&&e.shiftKey&&e.code==='KeyL'){
  const row={trusted:e.isTrusted,repeat:e.repeat,pending:window.__race.unlockPending,hasFocus:document.hasFocus(),prevented:null};
  window.__race.keys.push(row);setTimeout(()=>row.prevented=e.defaultPrevented,0);
 }
},true);
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*_):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
try:
 with sync_playwright() as pw:
  browser=pw.chromium.launch(headless=True,channel=os.getenv('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None)
  page=browser.new_page();page.add_init_script(mock);errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
  for ordering in ('lock_reply_first','unlock_reply_first'):
   page.goto(f'http://127.0.0.1:{server.server_port}')
   field=page.get_by_label('Мастер-пароль для разблокировки браузера',exact=True);expect(field).to_be_visible()
   field.fill('synthetic-master-fixture');page.get_by_role('button',name='Разблокировать браузер',exact=True).click()
   page.wait_for_function('window.__race.unlockPending')
   page.evaluate('window.__race.emit(false,11)')
   expect(page.get_by_title('Проверить обновления',exact=True)).to_be_visible()
   page.evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');page.wait_for_timeout(100)
   page.keyboard.down('Control');page.keyboard.down('Shift');page.keyboard.down('L');page.keyboard.down('L')
   page.keyboard.up('L');page.keyboard.up('Shift');page.keyboard.up('Control')
   page.wait_for_function('window.__race.keys.length===2&&window.__race.keys.every(k=>k.prevented===true)')
   data=page.evaluate('({calls:window.__race.lockCalls,keys:window.__race.keys})')
   assert data['calls']==1 and data['keys'][0]['trusted'] and not data['keys'][0]['repeat'] and data['keys'][1]['repeat'] and all(k['pending'] and k['hasFocus'] for k in data['keys']),data
   # A distinct second press expresses a new intent; do not lose it behind an
   # earlier lock response when an unlock may have been queued between them.
   page.keyboard.press('Control+Shift+L');page.wait_for_function('window.__race.lockCalls===2')
   if ordering=='lock_reply_first':
    page.evaluate('window.__race.completeLocks()');expect(field).to_be_visible()
    page.evaluate('window.__race.unlockReply()')
   else:
    page.evaluate('window.__race.unlockReply()');page.wait_for_function('!window.__race.unlockPending')
    page.evaluate('window.__race.completeLocks()')
   expect(field).to_be_visible();expect(field).to_be_enabled()
   page.wait_for_timeout(100);expect(field).to_be_visible()
   assert page.evaluate('window.__browserLock.locked&&window.__browserLock.revision===12')
  # Lock during an unlock is also a fresh intent while the local screen still
  # says locked; native serialization, not a stale frontend flag, orders it.
  page.goto(f'http://127.0.0.1:{server.server_port}')
  field=page.get_by_label('Мастер-пароль для разблокировки браузера',exact=True);field.fill('synthetic-master-fixture')
  page.get_by_role('button',name='Разблокировать браузер',exact=True).click();page.wait_for_function('window.__race.unlockPending')
  page.keyboard.press('Control+Shift+L');page.wait_for_function('window.__race.lockCalls===1')
  page.evaluate('window.__race.emit(false,11);window.__race.completeLocks();window.__race.unlockReply()')
  expect(field).to_be_visible();expect(field).to_be_enabled();page.wait_for_timeout(100);expect(field).to_be_visible()
  assert not errors,errors
  browser.close()
finally:server.shutdown();server.server_close()
print(json.dumps({'real_chromium':True,'shipped_app':True,'native_ipc':'explicit reordered fixture','lock_during_pending_unlock_not_lost':True,'trusted_held_key_repeat_deduplicated':True,'distinct_lock_intents_preserved':True,'late_old_unlock_cannot_reopen_locked_browser':True,'both_reply_orders_verified':True,'lock_intent_before_unlock_event_preserved':True}))
