"""Shipped App startup wiring in Chromium; updater IPC returns no update (fixture)."""
import ast,functools,http.server,json,os,pathlib,threading
from playwright.sync_api import sync_playwright,expect
root=pathlib.Path(__file__).resolve().parents[1]
module=ast.parse((root/'tests/frontend_controls.py').read_text())
base=next(ast.literal_eval(n.value) for n in module.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='mock' for t in n.targets))
mock=base+r'''
const original=window.__TAURI_INTERNALS__.invoke;
window.__updateChecks=0;
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='plugin:updater|check'){window.__updateChecks++;return null;}
 const value=await original(cmd,args);
 return cmd==='nastrojki_poluchit'?{...value,avtoobnovleniya:true}:value;
};
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
try:
 with sync_playwright() as pw:
  browser=pw.chromium.launch(headless=True,channel=os.getenv('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None)
  main=browser.new_context();page=main.new_page();page.add_init_script(mock);page.goto(f'http://127.0.0.1:{server.server_port}')
  page.wait_for_function('window.__updateChecks===1');assert page.evaluate('window.__updateChecks')==1;main.close()
  extra=browser.new_context();page=extra.new_page();page.add_init_script(mock);page.goto(f'http://127.0.0.1:{server.server_port}/?okno=browser-test&super=layout')
  page.wait_for_function('window.__calls.includes("nastrojki_poluchit")&&document.querySelector("button[title=\\"Проверить обновления\\"]")')
  page.evaluate('()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))')
  assert page.evaluate('window.__updateChecks')==0,'Restored secondary window started a competing automatic update'
  page.get_by_role('button',name='Проверить обновления',exact=True).click()
  page.wait_for_function('window.__updateChecks===1');expect(page.get_by_text('Обновлений нет — стоит последняя версия.',exact=True)).to_be_visible()
  extra.close();browser.close()
finally:server.shutdown()
print(json.dumps({'real_chromium':True,'shipped_app':True,'updater_ipc':'fixture','main_automatic_check_once':True,'secondary_startup_no_competing_check':True,'secondary_manual_check_preserved':True}))
