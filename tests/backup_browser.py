"""Shipped App/BackupDialog in one real Chromium; native pickers/backup IPC are fixtures.

Build frontend first. This does not prove encryption, native file dialogs, file IO,
transaction rollback or OS installation; those require the Rust/native CI gates.
"""
import ast
import functools
import http.server
import json
import os
import pathlib
import threading
from playwright.sync_api import expect, sync_playwright

ROOT=pathlib.Path(__file__).resolve().parents[1]
DIST=pathlib.Path(os.environ.get('NEXUSBRO_TEST_DIST',str(ROOT/'desktop/dist')))
module=ast.parse((ROOT/'tests/frontend_controls.py').read_text())
base=next(ast.literal_eval(n.value) for n in module.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='mock' for t in n.targets))
mock=base+r'''
const originalBackupInvoke=window.__TAURI_INTERNALS__.invoke;
window.__backup={calls:[],unlocked:true,cancelPicker:false,expired:false,badPassword:false,holdApply:false,release:null,nextToken:0};
window.__TAURI_INTERNALS__.invoke=async(cmd,args={})=>{
 const s=window.__backup;
 if(cmd==='security_vault')return {exists:true,unlocked:s.unlocked};
 if(cmd!=='security_backup')return originalBackupInvoke(cmd,args);
 s.calls.push(structuredClone(args));
 const action=args.action,data=args.data||{};
 if(action==='export_config')return {exported:true,kind:'config'};
 if(action==='export_passwords'){
  if(data.passphrase!=='independent-backup-password')throw 'Unexpected fixture passphrase';
  return {exported:true,kind:'passwords',count:3};
 }
 if(action==='preview_config'||action==='preview_passwords'){
  if(s.cancelPicker){s.cancelPicker=false;return {cancelled:true};}
  if(action==='preview_passwords'&&(s.badPassword||data.passphrase!=='independent-backup-password'))throw 'Неверный пароль резервной копии или файл повреждён';
  const password=action==='preview_passwords';
  return {token:'preview-'+(++s.nextToken),kind:password?'passwords':'config',bookmarks:4,passwords:3,settings_available:!password,
   supers:password?[{id:'password-group',name:'Рабочие аккаунты'}]:[{id:'config-a',name:'Хостеры'},{id:'config-b',name:'Личные сайты'}],
   local_routes:[{id:'ru-route',name:'Российский VPN'}],local_supers:[{id:'local-work',name:'Рабочие сайты'}]};
 }
 if(action==='prepare'){
  if(s.expired)throw 'Предварительный просмотр истёк. Откройте файл заново.';
  if(!data.token)throw 'Missing preview token';
  if(Object.keys(data.super_mapping||{}).length){if(data.super_mapping['password-group']!=='local-work')throw 'Invalid password mapping';}
  else if(data.routes['config-a']!=='ru-route'||data.routes['config-b']!=='direct')throw 'Every route must be explicitly selected';
  return {ready:true,token:data.token};
 }
 if(action==='apply'){
  if(data.confirm!==true)throw 'Missing explicit confirmation';
  if(s.holdApply)await new Promise(resolve=>s.release=resolve);
  return {imported:true};
 }
 if(action==='cancel')return {cancelled:true};
 throw 'Unknown fixture backup action';
};
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self,*_args):pass

server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(DIST)))
server.daemon_threads=True
threading.Thread(target=server.serve_forever,daemon=True).start()
try:
    with sync_playwright() as p:
        channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','')
        browser=p.chromium.launch(headless=True,**({'channel':channel} if channel else {}))
        page=browser.new_page(viewport={'width':1200,'height':900})
        errors=[];external=[]
        page.on('pageerror',lambda e:errors.append(str(e)))
        page.on('request',lambda r:external.append(r.url) if r.url.startswith(('http://','https://')) and not r.url.startswith(f'http://127.0.0.1:{server.server_port}/') else None)
        page.add_init_script(mock)
        page.goto(f'http://127.0.0.1:{server.server_port}/')
        expect(page.get_by_role('button',name='Проверить обновления',exact=True)).to_be_visible()
        def open_dialog():
            page.evaluate('window.dispatchEvent(new Event("nexusbro-backup"))')
            dialog=page.get_by_role('dialog',name='Резервные копии NexusBro',exact=True)
            expect(dialog).to_be_visible()
            return dialog
        dialog=open_dialog()
        expect(dialog.get_by_text('Cookies, данные сайтов, VPN-секреты и приватные сессии в файл не входят.',exact=False)).to_be_visible()
        dialog.get_by_role('button',name='Сохранить настройки в файл',exact=True).click()
        expect(dialog.get_by_role('status')).to_have_text('Копия настроек сохранена.')
        exported=page.evaluate('window.__backup.calls.find(c=>c.action==="export_config")')
        assert exported['data']=={},'UI must not pass raw profiles, URLs or device paths to config export'
        # Native chooser cancellation is represented explicitly, no prepare/apply.
        page.evaluate('window.__backup.cancelPicker=true')
        dialog.get_by_role('button',name='Импортировать настройки',exact=True).click()
        expect(dialog.get_by_role('heading',name='Предпросмотр импорта')).not_to_be_visible()
        dialog.get_by_role('button',name='Импортировать настройки',exact=True).click()
        expect(dialog.get_by_role('heading',name='Предпросмотр импорта')).to_be_visible()
        confirm=dialog.get_by_role('button',name='Подтвердить импорт',exact=True)
        expect(confirm).to_be_disabled()
        assert not page.evaluate('window.__backup.calls.some(c=>["prepare","apply"].includes(c.action))')
        def select(name,value):
            combo=dialog.get_by_role('combobox',name=name,exact=True)
            if combo.evaluate('(e)=>e.tagName')=='SELECT':
                combo.select_option(label=value)
            else:
                combo.click()
                page.locator('.vybor-popup').get_by_role('option',name=value,exact=True).click()
        select('Путь импортируемой супервкладки: Хостеры','Российский VPN')
        expect(confirm).to_be_disabled()
        select('Путь импортируемой супервкладки: Личные сайты','Прямое соединение')
        expect(confirm).to_be_enabled()
        dialog.get_by_role('checkbox',name='Также применить оформление и настройки браузера').check()
        page.evaluate('window.__backup.holdApply=true')
        confirm.click()
        page.wait_for_function('window.__backup.calls.some(c=>c.action==="prepare")')
        prepared=page.evaluate('window.__backup.calls.find(c=>c.action==="prepare")')
        assert prepared['data']['routes']=={'config-a':'ru-route','config-b':'direct'},prepared
        page.wait_for_function('typeof window.__backup.release==="function"')
        expect(confirm).to_be_disabled()
        expect(dialog.get_by_role('button',name='Отмена',exact=True)).to_be_disabled()
        assert page.evaluate('window.__backup.calls.filter(c=>c.action==="apply").length')==1
        page.evaluate('window.__backup.release();window.__backup.holdApply=false')
        expect(dialog.get_by_role('status')).to_have_text('Резервная копия импортирована.')
        prepared=page.evaluate('window.__backup.calls.find(c=>c.action==="prepare")')
        assert prepared['data']['routes']=={'config-a':'ru-route','config-b':'direct'} and prepared['data']['settings'] is True
        sequence=page.evaluate('window.__backup.calls.map(c=>c.action)')
        assert sequence.index('prepare')<sequence.index('apply')
        # Secret fields are masked, confirmation must match for export, success wipes both.
        password=dialog.get_by_label('Пароль резервной копии',exact=True)
        again=dialog.get_by_label('Повторите пароль резервной копии',exact=True)
        save=dialog.get_by_role('button',name='Сохранить зашифрованные пароли',exact=True)
        expect(save).to_be_disabled()
        password.fill('independent-backup-password');again.fill('different-password')
        expect(save).to_be_disabled();again.fill('independent-backup-password');expect(save).to_be_enabled()
        assert password.get_attribute('type')=='password' and again.get_attribute('type')=='password'
        save.click();expect(dialog.get_by_role('status')).to_have_text('Зашифрованная копия паролей сохранена.')
        expect(password).to_have_value('');expect(again).to_have_value('')
        assert 'independent-backup-password' not in dialog.inner_text()
        # Wrong archive password is visible; no prepare/apply occurs.
        password.fill('independent-backup-password');page.evaluate('window.__backup.badPassword=true')
        previous=page.evaluate('window.__backup.calls.filter(c=>c.action==="apply").length')
        dialog.get_by_role('button',name='Открыть копию паролей',exact=True).click()
        expect(dialog.get_by_role('alert')).to_contain_text('Неверный пароль резервной копии')
        assert page.evaluate('window.__backup.calls.filter(c=>c.action==="apply").length')==previous
        page.evaluate('window.__backup.badPassword=false')
        dialog.get_by_role('button',name='Открыть копию паролей',exact=True).click()
        expect(confirm).to_be_disabled()
        select('Куда импортировать пароли: Рабочие аккаунты','Рабочие сайты')
        expect(confirm).to_be_enabled();confirm.click()
        expect(dialog.get_by_role('status')).to_have_text('Резервная копия импортирована.')
        expect(password).to_have_value('');expect(again).to_have_value('')
        last=page.evaluate('window.__backup.calls.filter(c=>c.action==="apply").at(-1)')
        assert last['data']['confirm'] is True and last['data']['passphrase']=='independent-backup-password'
        # Expiry is surfaced without applying; cancellation wipes the held password.
        password.fill('independent-backup-password')
        dialog.get_by_role('button',name='Открыть копию паролей',exact=True).click()
        select('Куда импортировать пароли: Рабочие аккаунты','Рабочие сайты')
        page.evaluate('window.__backup.expired=true')
        previous=page.evaluate('window.__backup.calls.filter(c=>c.action==="apply").length')
        confirm.click();expect(dialog.get_by_role('alert')).to_contain_text('Предварительный просмотр истёк')
        assert page.evaluate('window.__backup.calls.filter(c=>c.action==="apply").length')==previous
        dialog.get_by_role('button',name='Отмена',exact=True).click()
        expect(password).to_have_value('');expect(again).to_have_value('')
        assert page.evaluate('window.__backup.calls.at(-1).action')=='cancel'
        # Closing an outstanding preview cancels its token; reopening clears secrets.
        page.evaluate('window.__backup.expired=false')
        password.fill('independent-backup-password')
        dialog.get_by_role('button',name='Открыть копию паролей',exact=True).click()
        dialog.get_by_role('button',name='Закрыть резервные копии',exact=True).click()
        expect(dialog).not_to_be_visible()
        page.wait_for_function('window.__backup.calls.at(-1).action==="cancel"')
        dialog=open_dialog();expect(dialog.get_by_label('Пароль резервной копии',exact=True)).to_have_value('')
        dialog.get_by_role('button',name='Закрыть резервные копии',exact=True).click()
        page.evaluate('window.__backup.unlocked=false')
        dialog=open_dialog()
        expect(dialog.get_by_text('Сначала разблокируйте хранилище',exact=False)).to_be_visible()
        expect(dialog.get_by_label('Пароль резервной копии',exact=True)).not_to_be_visible()
        assert page.evaluate('localStorage.length===0&&sessionStorage.length===0')
        assert not errors,errors
        assert not external,external
        print(json.dumps({'real_chromium':True,'actual_ui':'shipped App and BackupDialog','native_picker_crypto_and_file_io':'explicit IPC fixture, NOT native acceptance','config_export_no_raw_payload_or_device_paths':True,'config_preview_no_automatic_apply':True,'all_routes_explicit_before_apply':True,'prepare_before_confirmed_apply':True,'busy_dedup':True,'password_confirmation_and_masking':True,'password_error_visible':True,'password_group_mapping':True,'expiry_no_apply':True,'cancel_and_close_revoke_preview':True,'secrets_clear_after_success_cancel_close':True,'no_external_requests_or_plaintext_browser_storage':True}))
        browser.close()
finally:
    server.shutdown();server.server_close()
