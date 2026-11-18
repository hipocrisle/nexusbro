"""Actual Chromium UI regression; Tauri security IPC is a synthetic fixture.

Builds only this component in an ephemeral Vite harness. No browser credentials,
production account, external server or persistent profile are touched. Root's
native acceptance must separately prove vault encryption and CEF origin binding.
"""
import functools
import http.server
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import threading
from playwright.sync_api import sync_playwright, expect

root=pathlib.Path(__file__).resolve().parents[1]
mock=r'''
window.__vault={exists:false,unlocked:false};window.__rows=[];window.__stored={};window.__calls=[];window.__offline=false;window.__conflict=false;window.__private=false;
window.__account={configured:false,logged_in:false,username:'',server:'',totp_enabled:false};window.__selection={settings:false,bookmarks:false,supers:[],passwords:[]};window.__report=null;
window.__TAURI_INTERNALS__={invoke:async(cmd,args)=>{
 const a=args||{};window.__calls.push({cmd,action:a.action,data:a.data?structuredClone(a.data):undefined});
 if(cmd==='security_vault'){
  if(a.action==='create'){if(a.password.length<12)throw 'Слишком короткий мастер-пароль';window.__master=a.password;window.__vault={exists:true,unlocked:true};}
  if(a.action==='unlock'){if(a.password!==window.__master)throw 'Неверный мастер-пароль';window.__vault.unlocked=true;}
  if(a.action==='lock')window.__vault.unlocked=false;return structuredClone(window.__vault);
 }
 if(cmd==='aktivnaya')return 'normal-super';
 if(cmd==='obychnye_spisok')return {aktivnaya:7,vkladki:[{id:7,privatnaya:window.__private,otlozhena:false}]};
 if(cmd==='bufer_zapisat'){window.__clipboard=a.text;return null;}
 if(!window.__vault.unlocked)throw 'Хранилище заблокировано';
 if(cmd==='security_passwords'){
  if(a.action==='list')return structuredClone(window.__rows);
  if(a.action==='save'){const d=a.data,id='cred-'+(window.__rows.length+1);window.__stored[id]=d.password;const row={id,origin:d.origin,username:d.username,title:d.title,super_id:d.super_id,updated_at:1795000000000};window.__rows.push(row);return structuredClone(row);}
  if(a.action==='reveal')return window.__stored[a.data.id];
  if(a.action==='delete'){window.__rows=window.__rows.filter(v=>v.id!==a.data.id);delete window.__stored[a.data.id];return {};}
  if(a.action==='capture')return {draft_id:'draft-fixture',origin:'https://capture.fixture.test',username:'captured-login'};
  if(a.action==='save_draft'){const id='captured-credential',row={id,origin:'https://capture.fixture.test',username:a.data.username,title:a.data.title,super_id:'normal-super',updated_at:1795000000001};window.__stored[id]='captured-fixture-password';window.__rows.push(row);return row;}
  if(a.action==='fill')return {};
 }
 if(cmd==='security_account'){
  if(a.action==='configure'){if(!a.data.server.startsWith('https://'))throw 'Требуется HTTPS';window.__account.server=a.data.server;window.__account.configured=true;}
  if(['register','login','recover'].includes(a.action)){if(window.__offline)throw 'Сервер недоступен';window.__account.logged_in=true;window.__account.username=a.data.username;if(a.action==='register')return {...window.__account,recovery_key:'RECOVERY-FIXTURE-ONLY'};}
  if(a.action==='logout')window.__account.logged_in=false;
  if(a.action==='totp_enroll')return {secret:'JBSWY3DPEHPK3PXP',otpauth_uri:'otpauth://totp/NexusBro:fixture?secret=JBSWY3DPEHPK3PXP&issuer=NexusBro'};
  if(a.action==='totp_verify'){if(a.data.code!=='123456')throw 'Неверный код 2FA';window.__account.totp_enabled=true;return {...window.__account,recovery_codes:['BACKUP-ONE','BACKUP-TWO']};}
  if(a.action==='totp_disable')window.__account.totp_enabled=false;
  if(a.action==='delete_account')window.__account={configured:true,logged_in:false,username:'',server:window.__account.server,totp_enabled:false};
  return structuredClone(window.__account);
 }
 if(cmd==='security_sync'){
  if(a.action==='configure')window.__selection=structuredClone(a.data.selection);
  if(a.action==='run'){if(window.__offline)throw 'Сервер недоступен';window.__report={pushed:3,pulled:1,conflicts:window.__conflict?['nexusbro.v1/password/conflict']:[]};return structuredClone(window.__report);}
  return {selection:structuredClone(window.__selection),available_supers:[{id:'normal-super',name:'Рабочая Super'},{id:'other-super',name:'Личная Super'}],last_sync:window.__report?1795000000000:null,report:window.__report};
 }
 throw 'Неизвестная тестовая команда '+cmd;
}};
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*args):pass

with tempfile.TemporaryDirectory(prefix='nexusbro-security-ui-') as temp:
 harness=pathlib.Path(temp);(harness/'src').mkdir()
 for name in ['SecuritySettings.tsx','security.ts','securitySettings.css','Icons.tsx','Vybor.tsx','App.css']:
  shutil.copy2(root/'desktop/src'/name,harness/'src'/name)
 modules=root/'desktop/node_modules'
 if not modules.exists():modules=pathlib.Path('/matrix/nexusbro/desktop/node_modules')
 if not modules.exists():raise RuntimeError('Install desktop Node dependencies before UI test')
 (harness/'node_modules').symlink_to(modules.resolve(),target_is_directory=True)
 (harness/'package.json').write_text('{"type":"module"}')
 (harness/'index.html').write_text('<div id="root"></div><script type="module" src="/src/main.tsx"></script>')
 (harness/'src/main.tsx').write_text("import React from 'react';import {createRoot} from 'react-dom/client';import SecuritySettings from './SecuritySettings';import './App.css';createRoot(document.getElementById('root')!).render(<SecuritySettings onClose={()=>{document.body.dataset.closed='yes'}}/>);")
 (harness/'vite.config.mjs').write_text("import {defineConfig} from 'vite';import react from '@vitejs/plugin-react';export default defineConfig({plugins:[react()]});")
 subprocess.run([str(modules/'.bin/vite'),'build','--config',str(harness/'vite.config.mjs')],cwd=harness,check=True,stdout=subprocess.DEVNULL)
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(harness/'dist')))
 threading.Thread(target=server.serve_forever,daemon=True).start()
 try:
  with sync_playwright() as p:
   browser=p.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None,headless=True,args=['--no-sandbox'])
   page=browser.new_page(viewport={'width':1100,'height':820});errors=[];page.on('pageerror',lambda e:errors.append(str(e)));page.on('dialog',lambda d:d.accept());page.add_init_script(mock)
   page.goto(f'http://127.0.0.1:{server.server_port}')
   dialog=page.get_by_role('dialog',name='Пароли, аккаунт и синхронизация NexusBro')
   expect(dialog.get_by_role('heading',name='Создать защищённое хранилище')).to_be_visible()
   assert page.evaluate("window.__calls.every(x=>x.cmd==='security_vault')")
   dialog.get_by_label('Мастер-пароль',exact=True).fill('master-fixture-strong')
   dialog.get_by_label('Повторите мастер-пароль',exact=True).fill('different-password')
   expect(dialog.get_by_role('button',name='Создать хранилище',exact=True)).to_be_disabled()
   dialog.get_by_label('Повторите мастер-пароль',exact=True).fill('master-fixture-strong')
   dialog.get_by_role('button',name='Создать хранилище',exact=True).click()
   expect(dialog.get_by_role('heading',name='Сохранённые логины и пароли')).to_be_visible()
   dialog.get_by_role('button',name='Добавить вручную',exact=True).click()
   dialog.get_by_label('Адрес сайта для пароля').fill('https://login.fixture.test')
   dialog.get_by_label('Логин сайта',exact=True).fill('fixture-login')
   dialog.get_by_label('Пароль сайта',exact=True).fill('site-fixture-password')
   dialog.get_by_label('Название записи пароля').fill('Fixture login')
   dialog.get_by_role('combobox',name='Супер-вкладка пароля',exact=True).click()
   page.get_by_role('option',name='Рабочая Super',exact=True).click()
   dialog.get_by_role('button',name='Сохранить пароль',exact=True).click()
   expect(dialog.get_by_text('Fixture login',exact=True)).to_be_visible()
   assert 'site-fixture-password' not in dialog.inner_text()
   assert page.evaluate('localStorage.length')==0
   dialog.get_by_role('button',name='Показать пароль: Fixture login',exact=True).click()
   expect(dialog.get_by_label('Пароль: Fixture login',exact=True)).to_have_value('site-fixture-password')
   dialog.get_by_role('button',name='Аккаунт',exact=True).click()
   expect(dialog.get_by_label('Пароль: Fixture login',exact=True)).not_to_be_visible()
   expect(dialog.get_by_text('Сервер ещё не настроен.',exact=False)).to_be_visible()
   dialog.get_by_label('Сервер синхронизации NexusBro',exact=True).fill('https://browser-sync.fixture.invalid')
   dialog.get_by_role('button',name='Сохранить сервер',exact=True).click()
   expect(dialog.get_by_role('status')).to_contain_text('Подключение проверяется при входе')
   dialog.get_by_role('button',name='Зарегистрироваться',exact=True).click()
   dialog.get_by_label('Имя аккаунта NexusBro',exact=True).fill('browser-fixture')
   dialog.get_by_label('Пароль аккаунта NexusBro',exact=True).fill('account-fixture-strong')
   dialog.get_by_label('Повторите пароль аккаунта',exact=True).fill('account-fixture-strong')
   dialog.get_by_role('button',name='Создать аккаунт NexusBro',exact=True).click()
   region=dialog.get_by_role('region',name='Ключ восстановления аккаунта',exact=True)
   expect(region).to_contain_text('RECOVERY-FIXTURE-ONLY')
   region.get_by_role('button',name='Ключ сохранён',exact=True).click();expect(region).not_to_be_visible()
   dialog.get_by_role('button',name='Настроить двухфакторную аутентификацию',exact=True).click()
   expect(dialog.get_by_text('До подтверждения 2FA не включена.',exact=False)).to_be_visible()
   dialog.get_by_label('Код подтверждения 2FA',exact=True).fill('000000')
   dialog.get_by_role('button',name='Подтвердить и включить 2FA',exact=True).click()
   expect(dialog.get_by_role('alert')).to_contain_text('Неверный код 2FA')
   assert not page.evaluate('window.__account.totp_enabled')
   dialog.get_by_label('Код подтверждения 2FA',exact=True).fill('123456')
   dialog.get_by_role('button',name='Подтвердить и включить 2FA',exact=True).click()
   codes=dialog.get_by_role('region',name='Резервные коды 2FA',exact=True)
   expect(codes).to_contain_text('BACKUP-ONE')
   codes.get_by_role('button',name='Резервные коды сохранены',exact=True).click();expect(codes).not_to_be_visible()
   dialog.get_by_role('button',name='Синхронизация',exact=True).click()
   for box in dialog.get_by_role('checkbox').all():expect(box).not_to_be_checked()
   dialog.get_by_role('checkbox',name='Настройки интерфейса',exact=False).check()
   dialog.get_by_role('group',name='Супер-вкладки',exact=True).get_by_role('checkbox',name='Рабочая Super',exact=True).check()
   dialog.get_by_role('group',name='Пароли',exact=True).get_by_role('checkbox').first.check()
   dialog.get_by_role('button',name='Сохранить выбор',exact=True).click()
   expect(dialog.get_by_role('status')).to_contain_text('Синхронизация ещё не запускалась')
   selected=page.evaluate('window.__selection');assert selected=={'settings':True,'bookmarks':False,'supers':['normal-super'],'passwords':['cred-1']},selected
   page.evaluate('window.__offline=true')
   dialog.get_by_role('button',name='Синхронизировать сейчас',exact=True).click()
   expect(dialog.get_by_role('alert')).to_contain_text('Сервер недоступен')
   assert 'Синхронизация завершена' not in dialog.inner_text()
   page.evaluate('window.__offline=false;window.__conflict=true')
   dialog.get_by_role('button',name='Синхронизировать сейчас',exact=True).click()
   expect(dialog.get_by_text('Обнаружены конфликты.',exact=False)).to_be_visible()
   assert 'Синхронизация завершена' not in dialog.inner_text()
   page.evaluate('window.__conflict=false')
   dialog.get_by_role('button',name='Синхронизировать сейчас',exact=True).click()
   expect(dialog.get_by_role('status')).to_contain_text('Синхронизация завершена')
   dialog.get_by_role('button',name='Аккаунт',exact=True).click()
   dialog.get_by_role('button',name='Изменить пароль аккаунта',exact=True).click()
   dialog.get_by_label('Текущий пароль аккаунта',exact=True).fill('account-fixture-strong')
   dialog.get_by_label('Новый пароль аккаунта',exact=True).fill('new-account-fixture-strong')
   dialog.get_by_label('Повторите новый пароль аккаунта',exact=True).fill('new-account-fixture-strong')
   dialog.get_by_label('Код 2FA для смены пароля',exact=True).fill('123456')
   dialog.get_by_role('button',name='Сохранить новый пароль аккаунта',exact=True).click()
   expect(dialog.get_by_role('status')).to_contain_text('Пароль аккаунта изменён')
   dialog.get_by_label('Код для отключения 2FA',exact=True).fill('123456')
   dialog.get_by_role('button',name='Отключить 2FA',exact=True).click()
   expect(dialog.get_by_role('status')).to_contain_text('Двухфакторная аутентификация отключена')
   dialog.get_by_role('button',name='Выйти из аккаунта',exact=True).click()
   expect(dialog.get_by_role('button',name='Войти в NexusBro',exact=True)).to_be_visible()
   dialog.get_by_role('button',name='Восстановить доступ',exact=True).click()
   dialog.get_by_label('Ключ восстановления аккаунта',exact=True).fill('RECOVERY-FIXTURE-ONLY')
   dialog.get_by_label('Новый пароль аккаунта',exact=True).fill('recovered-account-fixture')
   dialog.get_by_label('Повторите пароль аккаунта',exact=True).fill('recovered-account-fixture')
   dialog.get_by_role('button',name='Восстановить аккаунт',exact=True).click()
   expect(dialog.get_by_role('status')).to_contain_text('Вход в аккаунт NexusBro выполнен')
   dialog.locator('summary').filter(has_text='Удаление аккаунта').click()
   dialog.get_by_label('Подтверждение удаления аккаунта',exact=True).fill('browser-fixture')
   dialog.get_by_label('Пароль для удаления аккаунта',exact=True).fill('recovered-account-fixture')
   dialog.get_by_role('button',name='Удалить аккаунт безвозвратно',exact=True).click()
   expect(dialog.get_by_role('status')).to_contain_text('Аккаунт NexusBro удалён')
   assert page.evaluate('window.__rows.length')==1
   for action in ['change_password','totp_disable','logout','recover','delete_account']:
    assert page.evaluate('(action)=>window.__calls.some(x=>x.cmd===\'security_account\'&&x.action===action)',action)
   dialog.get_by_role('button',name='Пароли',exact=True).click()
   dialog.get_by_role('button',name='Сохранить с открытой страницы',exact=True).click()
   expect(dialog.get_by_text('https://capture.fixture.test',exact=True)).to_be_visible()
   dialog.get_by_label('Название записи пароля').fill('Captured fixture')
   dialog.get_by_role('button',name='Сохранить пароль',exact=True).click()
   expect(dialog.get_by_text('Captured fixture',exact=True)).to_be_visible()
   dialog.get_by_role('button',name='Заполнить на странице',exact=True).first.click()
   expect(dialog.get_by_role('status')).to_contain_text('Данные заполнены')
   call=page.evaluate("window.__calls.filter(x=>x.cmd==='security_passwords'&&x.action==='fill').at(-1)")
   assert call['data']=={'id':'cred-1','super_id':'normal-super','tab_id':7},call
   page.evaluate('window.__private=true')
   before=page.evaluate("window.__calls.filter(x=>x.cmd==='security_passwords'&&x.action==='fill').length")
   dialog.get_by_role('button',name='Заполнить на странице',exact=True).first.click()
   expect(dialog.get_by_role('alert')).to_contain_text('Пароли приватных вкладок')
   assert page.evaluate("window.__calls.filter(x=>x.cmd==='security_passwords'&&x.action==='fill').length")==before
   page.evaluate('window.__private=false')
   page.set_viewport_size({'width':900,'height':560})
   rect=dialog.bounding_box();assert rect['y']>=0 and rect['y']+rect['height']<=560 and rect['width']>=800
   assert dialog.evaluate('(e)=>e.scrollWidth<=e.clientWidth+1')
   expect(dialog.get_by_role('button',name='Закрыть',exact=True)).to_be_in_viewport()
   page.screenshot(path='/tmp/nexusbro-security-ui.png')
   dialog.get_by_role('button',name='Заблокировать хранилище',exact=True).click()
   expect(dialog.get_by_role('heading',name='Разблокировать хранилище',exact=True)).to_be_visible()
   expect(dialog.get_by_text('Fixture login',exact=True)).not_to_be_visible()
   dialog.get_by_label('Мастер-пароль',exact=True).fill('wrong-password')
   dialog.get_by_role('button',name='Разблокировать',exact=True).click()
   expect(dialog.get_by_role('alert')).to_contain_text('Неверный мастер-пароль')
   expect(dialog.get_by_label('Мастер-пароль',exact=True)).to_have_value('')
   assert page.locator('select').count()==0
   assert page.evaluate('localStorage.length')==0
   assert not errors,errors
   print(json.dumps({'real_chromium':True,'native_ipc':'fixture','vault_gate':True,'manual_password_save_and_explicit_reveal':True,'separate_account_and_show_once_recovery':True,'totp_confirmation_and_bad_code':True,'account_change_recover_delete':True,'granular_sync_all_off':True,'offline_and_conflicts_never_success':True,'capture_consent_and_exact_target':True,'private_target_rejected_before_ipc':True,'small_window_fits':True,'lock_hides_secrets':True,'no_localstorage_secrets':True}))
   browser.close()
 finally:server.shutdown()
