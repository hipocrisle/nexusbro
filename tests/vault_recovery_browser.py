"""Shipped recovery UI in real Chromium; IPC is an explicit fixture.

Native cryptography/files and physical OS acceptance run separately in CI.
"""
import ast, functools, http.server, json, os, pathlib, threading
from playwright.sync_api import sync_playwright, expect
root=pathlib.Path(__file__).resolve().parents[1]
module=ast.parse((root/'tests/frontend_controls.py').read_text())
base=next(ast.literal_eval(n.value) for n in module.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='mock' for t in n.targets))
mock=base+r'''
const previous=window.__TAURI_INTERNALS__.invoke;
window.__browserLock={locked:true,exists:true,revision:10};
window.__recovery={resets:0,restores:0,lists:0};
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='browser_lock_status')return structuredClone(window.__browserLock);
 if(cmd==='browser_lock'){window.__browserLock={locked:true,exists:true,revision:window.__browserLock.revision+1};return structuredClone(window.__browserLock);}
 if(cmd==='browser_vault_backups'){window.__recovery.lists++;return [{id:'1730000000-opaque-fixture',created:1730000000}];}
 if(cmd==='browser_vault_reset'){
  window.__recovery.resets++;
  if(!window.__browserLock.locked||args.confirmation!=='СБРОСИТЬ'||args.password!=='new-synthetic-master-26')throw Error('Invalid reset request');
  window.__browserLock={locked:false,exists:true,revision:window.__browserLock.revision+1};return structuredClone(window.__browserLock);
 }
 if(cmd==='browser_vault_restore'){
  window.__recovery.restores++;
  if(!window.__browserLock.locked||args.id!=='1730000000-opaque-fixture')throw Error('Invalid restore request');
  if(args.password!=='old-synthetic-master-26')throw Error('Неверный мастер-пароль или повреждено хранилище');
  window.__browserLock={locked:false,exists:true,revision:window.__browserLock.revision+1};return structuredClone(window.__browserLock);
 }
 return previous(cmd,args);
};
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*_):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
try:
 with sync_playwright() as pw:
  browser=pw.chromium.launch(headless=True,channel=os.getenv('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None)
  page=browser.new_page(viewport={'width':1100,'height':800});page.add_init_script(mock)
  errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
  page.goto(f'http://127.0.0.1:{server.server_port}')
  locked=page.get_by_label('Мастер-пароль для разблокировки браузера',exact=True);expect(locked).to_be_visible()
  page.get_by_role('button',name='Забыли мастер-пароль?',exact=True).click()
  submit=page.get_by_role('button',name='Сбросить хранилище',exact=True)
  page.get_by_label('Новый мастер-пароль',exact=True).fill('new-synthetic-master-26')
  page.get_by_label('Повторите новый мастер-пароль',exact=True).fill('mismatch-synthetic-master')
  page.get_by_label('Подтверждение сброса',exact=True).fill('СБРОСИТЬ')
  expect(submit).to_be_disabled();assert page.evaluate('window.__recovery.resets')==0
  page.get_by_label('Повторите новый мастер-пароль',exact=True).fill('new-synthetic-master-26')
  page.get_by_label('Подтверждение сброса',exact=True).fill('wrong')
  expect(submit).to_be_disabled();page.get_by_role('button',name='Назад',exact=True).click()
  expect(locked).to_be_visible();assert page.evaluate('window.__browserLock.locked&&window.__recovery.resets===0')
  page.get_by_role('button',name='Забыли мастер-пароль?',exact=True).click()
  expect(page.get_by_label('Новый мастер-пароль',exact=True)).to_have_value('')
  page.get_by_label('Новый мастер-пароль',exact=True).fill('new-synthetic-master-26')
  page.get_by_label('Повторите новый мастер-пароль',exact=True).fill('new-synthetic-master-26')
  page.get_by_label('Подтверждение сброса',exact=True).fill('СБРОСИТЬ');submit.click()
  expect(page.get_by_title('Проверить обновления',exact=True)).to_be_visible();assert page.evaluate('window.__recovery.resets===1')
  page.keyboard.press('Control+Shift+L');expect(locked).to_be_visible()
  page.get_by_role('button',name='Восстановить из копии',exact=True).click()
  restore=page.get_by_role('button',name='Восстановить хранилище',exact=True)
  old=page.get_by_label('Мастер-пароль выбранной копии',exact=True)
  old.fill('wrong-synthetic-master');restore.click()
  expect(page.get_by_role('alert')).to_contain_text('Неверный мастер-пароль')
  expect(old).to_be_visible();expect(old).to_have_value('');assert page.evaluate('window.__browserLock.locked')
  old.fill('old-synthetic-master-26');restore.click()
  expect(page.get_by_title('Проверить обновления',exact=True)).to_be_visible()
  assert page.evaluate('window.__recovery.restores===2')
  assert not errors,errors
  browser.close()
finally:server.shutdown();server.server_close()
print(json.dumps({'real_chromium':True,'shipped_app':True,'native_ipc':'explicit fixture','reset_requires_matching_password_and_exact_confirmation':True,'cancel_preserves_lock_and_clears_password':True,'wrong_restore_stays_locked_and_clears_password':True,'reset_and_old_password_restore_ui':True}))
