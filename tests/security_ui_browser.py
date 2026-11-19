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
import urllib.parse
from playwright.sync_api import sync_playwright, expect

root=pathlib.Path(__file__).resolve().parents[1]
mock=r'''
window.__vault={exists:false,unlocked:false};window.__rows=[];window.__stored={};window.__calls=[];window.__offline=false;window.__conflict=false;window.__private=false;
window.__account={configured:false,logged_in:false,username:'',server:'',totp_enabled:false};window.__selection={settings:false,bookmarks:false,supers:[],passwords:[]};window.__report=null;window.__statusConflict=false;window.__importData=false;window.__supers=[{id:'normal-super',name:'Рабочая Super'},{id:'other-super',name:'Личная Super'}];
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
  if(a.action==='default_server')return {server:'https://nexusbro-sync.hipogas.org'};
  if(a.action==='configure'){if(window.__configureOffline)throw 'Сервис аккаунтов временно недоступен';if(!a.data.server.startsWith('https://'))throw 'Требуется HTTPS';window.__account.server=a.data.server;window.__account.configured=true;}
  if(['register','login','recover'].includes(a.action)){if(window.__offline)throw 'Сервер недоступен';window.__account.logged_in=true;window.__account.username=a.data.username;if(a.action==='register'){if(window.__authPartial){window.__account.logged_in=false;return {...window.__account,recovery_key:'RECOVERY-FIXTURE-ONLY',warning:'Автоматический вход не завершён'};}return {...window.__account,recovery_key:'RECOVERY-FIXTURE-ONLY'};}}
  if(a.action==='logout')window.__account.logged_in=false;
  if(a.action==='totp_enroll')return {secret:'JBSWY3DPEHPK3PXP',otpauth_uri:'otpauth://totp/NexusBro:fixture?secret=JBSWY3DPEHPK3PXP&issuer=NexusBro'};
  if(a.action==='totp_verify'){if(a.data.code!=='123456')throw 'Неверный код 2FA';window.__account.totp_enabled=true;return {...window.__account,recovery_codes:['BACKUP-ONE','BACKUP-TWO']};}
  if(a.action==='totp_disable')window.__account.totp_enabled=false;
  if(a.action==='delete_account')window.__account={configured:true,logged_in:false,username:'',server:window.__account.server,totp_enabled:false};
  return structuredClone(window.__account);
 }
 if(cmd==='security_sync'){
  if(a.action==='configure')window.__selection=structuredClone(a.data.selection);
  if(a.action==='resolve'){
   if(window.__offline)throw 'Сервер недоступен';
   if(!window.__report||!window.__report.conflicts.includes(a.data.id)||!['local','remote'].includes(a.data.choice))throw 'Недопустимое разрешение конфликта';
   window.__report.conflicts=window.__report.conflicts.filter(v=>v!==a.data.id);if(!window.__report.conflicts.length)window.__conflict=false;
  }
  if(a.action==='run'){if(window.__offline)throw 'Сервер недоступен';if(window.__importData){if(window.__selection.supers.includes('*')&&!window.__supers.length)window.__supers.push({id:'remote-super',name:'Облачная Super'});if(window.__selection.passwords.includes('*')&&!window.__rows.length)window.__rows.push({id:'remote-credential',origin:'https://remote.fixture.test',username:'remote-login',title:'Облачный пароль',super_id:'remote-super',updated_at:1795000000000});}window.__report={pushed:window.__importData?0:3,pulled:window.__importData?2:1,conflicts:window.__conflict?['nexusbro.v1/password/cred-1','nexusbro.v1/super/normal-super']:[]};return structuredClone(window.__report);}
  return {selection:structuredClone(window.__selection),available_supers:structuredClone(window.__supers),last_sync:window.__report?1795000000000:null,report:window.__statusConflict&&window.__report?{...structuredClone(window.__report),conflicts:['nexusbro.v1/password/cred-1']}:window.__report};
 }
 throw 'Неизвестная тестовая команда '+cmd;
}};
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*args):pass

with tempfile.TemporaryDirectory(prefix='nexusbro-security-ui-') as temp:
 harness=pathlib.Path(temp);(harness/'src').mkdir()
 for name in ['SecuritySettings.tsx','TotpQr.tsx','security.ts','securitySettings.css','Icons.tsx','Vybor.tsx','App.css']:
  shutil.copy2(root/'desktop/src'/name,harness/'src'/name)
 modules=root/'desktop/node_modules'
 if not modules.exists():modules=pathlib.Path('/matrix/nexusbro/desktop/node_modules')
 if not modules.exists():raise RuntimeError('Install desktop Node dependencies before UI test')
 (harness/'node_modules').symlink_to(modules.resolve(),target_is_directory=True)
 (harness/'package.json').write_text('{"type":"module"}')
 (harness/'index.html').write_text('<div id="root"></div><script type="module" src="/src/main.tsx"></script>')
 (harness/'src/main.tsx').write_text("import React from 'react';import jsQR from 'jsqr';import {createRoot} from 'react-dom/client';import SecuritySettings from './SecuritySettings';import './App.css';(window as any).__decodeTotpQr=()=>{const c=document.querySelector<HTMLCanvasElement>('[data-testid=totp-qr]');if(!c||!c.width)return null;const data=c.getContext('2d')!.getImageData(0,0,c.width,c.height);return jsQR(data.data,c.width,c.height)?.data??null;};createRoot(document.getElementById('root')!).render(<SecuritySettings onClose={()=>{document.body.dataset.closed='yes'}}/>);")
 (harness/'vite.config.mjs').write_text("import {defineConfig} from 'vite';import react from '@vitejs/plugin-react';export default defineConfig({plugins:[react()]});")
 subprocess.run([str(modules/'.bin/vite'),'build','--config',str(harness/'vite.config.mjs')],cwd=harness,check=True,stdout=subprocess.DEVNULL)
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(harness/'dist')))
 threading.Thread(target=server.serve_forever,daemon=True).start()
 try:
  with sync_playwright() as p:
   browser=p.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None,headless=True,args=['--no-sandbox'])
   page=browser.new_page(viewport={'width':1100,'height':820});external_requests=[];page.on('request',lambda r:external_requests.append(True) if r.url.startswith(('https://','http://')) and not r.url.startswith(f'http://127.0.0.1:{server.server_port}/') else None);errors=[];confirm_accept={'value':True};confirmations=[];page.on('pageerror',lambda e:errors.append(str(e)));page.on('dialog',lambda d:(confirmations.append(d.message),d.accept() if confirm_accept['value'] else d.dismiss()));page.add_init_script(mock)
   page.goto(f'http://127.0.0.1:{server.server_port}')
   dialog=page.get_by_role('dialog',name='Пароли, аккаунт и синхронизация NexusBro')
   expect(dialog.get_by_role('heading',name='Создать защищённое хранилище')).to_be_visible()
   expect(dialog.get_by_role('list',name='Как сохраняются пароли')).to_be_visible()
   expect(dialog.get_by_text('Аккаунт NexusBro для сохранения паролей на этом устройстве не нужен.',exact=False)).to_be_visible()
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
   expect(dialog.get_by_text('Подключение NexusBro настроится автоматически.',exact=False)).to_be_visible()
   expect(dialog.get_by_label('Сервер синхронизации NexusBro',exact=True)).not_to_be_visible()
   assert not page.evaluate("window.__calls.some(x=>x.cmd==='security_account'&&x.action==='configure')")
   dialog.get_by_role('button',name='Зарегистрироваться',exact=True).click()
   dialog.get_by_label('Имя аккаунта NexusBro',exact=True).fill('browser-fixture')
   dialog.get_by_label('Пароль аккаунта NexusBro',exact=True).fill('account-fixture-strong')
   dialog.get_by_label('Повторите пароль аккаунта',exact=True).fill('account-fixture-strong')
   dialog.get_by_role('button',name='Создать аккаунт NexusBro',exact=True).click()
   region=dialog.get_by_role('region',name='Ключ восстановления аккаунта',exact=True)
   expect(region).to_contain_text('RECOVERY-FIXTURE-ONLY')
   account_calls=page.evaluate("window.__calls.filter(x=>x.cmd==='security_account'&&['configure','register'].includes(x.action))")
   assert [x['action'] for x in account_calls]==['configure','register'],account_calls
   assert account_calls[0]['data']=={'server':'https://nexusbro-sync.hipogas.org'},account_calls
   assert page.evaluate("window.__calls.filter(x=>x.cmd==='security_account'&&x.action==='default_server').length")==1
   assert page.evaluate('window.__account.logged_in')
   region.get_by_role('button',name='Ключ сохранён',exact=True).click();expect(region).not_to_be_visible()
   dialog.get_by_role('button',name='Настроить двухфакторную аутентификацию',exact=True).click()
   expect(dialog.get_by_text('До подтверждения 2FA не включена.',exact=False)).to_be_visible()
   qr=dialog.get_by_test_id('totp-qr');expect(qr).to_be_visible()
   assert qr.get_attribute('role')=='img'
   decoded=page.evaluate('window.__decodeTotpQr()')
   expected='otpauth://totp/NexusBro:fixture?secret=JBSWY3DPEHPK3PXP&issuer=NexusBro'
   assert decoded==expected,'Rendered local QR did not decode to the enrollment payload'
   parts=urllib.parse.urlsplit(decoded);params=urllib.parse.parse_qs(parts.query)
   assert parts.scheme=='otpauth' and parts.netloc=='totp' and params['issuer']==['NexusBro'],'QR account metadata changed'
   assert not page.evaluate('window.__account.totp_enabled'),'Showing a QR enabled 2FA without verification'
   dialog.locator('summary').filter(has_text='Ввести ключ вручную').click()
   expect(dialog.get_by_test_id('totp-manual-key')).to_be_visible()
   assert dialog.get_by_test_id('totp-manual-key').inner_text()==params['secret'][0],'Manual key differs from the scannable QR'
   dialog.get_by_role('button',name='Скопировать ключ настройки',exact=True).click()
   assert page.evaluate('window.__clipboard')==params['secret'][0],'Manual fallback copied a different key'
   assert 'Тип: по времени (TOTP), 6 цифр, период 30 секунд.' in dialog.inner_text()
   dialog.locator('summary').filter(has_text='Ввести ключ вручную').click()
   page.evaluate("window.__previousTotpCanvas=document.querySelector('[data-testid=totp-qr]')")
   dialog.get_by_label('Код подтверждения 2FA',exact=True).fill('000000')
   dialog.get_by_role('button',name='Подтвердить и включить 2FA',exact=True).click()
   expect(dialog.get_by_role('alert')).to_contain_text('Неверный код 2FA')
   assert not page.evaluate('window.__account.totp_enabled')
   dialog.get_by_label('Код подтверждения 2FA',exact=True).fill('123 456')
   expect(dialog.get_by_label('Код подтверждения 2FA',exact=True)).to_have_value('123456')
   dialog.get_by_role('button',name='Подтвердить и включить 2FA',exact=True).click()
   codes=dialog.get_by_role('region',name='Резервные коды 2FA',exact=True)
   expect(qr).to_have_count(0)
   assert page.evaluate('window.__previousTotpCanvas.width===0&&window.__previousTotpCanvas.height===0'),'Unmounted canvas retained QR pixels'
   assert 'JBSWY3DPEHPK3PXP' not in dialog.inner_text(),'Enrollment key remained visible after confirmation'
   expect(codes).to_contain_text('BACKUP-ONE')
   codes.get_by_role('button',name='Резервные коды сохранены',exact=True).click();expect(codes).not_to_be_visible()
   dialog.get_by_role('button',name='Синхронизация',exact=True).click()
   for box in dialog.get_by_role('checkbox').all():expect(box).not_to_be_checked()
   all_supers=dialog.get_by_role('checkbox',name='Все супер-вкладки, включая новые',exact=False)
   all_passwords=dialog.get_by_role('checkbox',name='Все пароли, включая новые',exact=False)
   all_supers.check();expect(dialog.get_by_role('checkbox',name='Рабочая Super',exact=True)).to_be_disabled();expect(dialog.get_by_role('checkbox',name='Рабочая Super',exact=True)).to_be_checked()
   all_supers.uncheck();expect(dialog.get_by_role('checkbox',name='Рабочая Super',exact=True)).to_be_enabled();expect(dialog.get_by_role('checkbox',name='Рабочая Super',exact=True)).not_to_be_checked()
   all_passwords.check();expect(dialog.get_by_role('group',name='Пароли',exact=True).get_by_role('checkbox',name='Fixture login',exact=False)).to_be_disabled()
   all_passwords.uncheck();expect(dialog.get_by_role('group',name='Пароли',exact=True).get_by_role('checkbox',name='Fixture login',exact=False)).to_be_enabled()
   dialog.get_by_role('checkbox',name='Настройки интерфейса',exact=False).check()
   dialog.get_by_role('group',name='Супер-вкладки',exact=True).get_by_role('checkbox',name='Рабочая Super',exact=True).check()
   dialog.get_by_role('group',name='Пароли',exact=True).get_by_role('checkbox',name='Fixture login',exact=False).check()
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
   password_conflict=dialog.get_by_role('group',name='Конфликт: Пароль — Fixture login',exact=True)
   super_conflict=dialog.get_by_role('group',name='Конфликт: Супер-вкладка — Рабочая Super',exact=True)
   expect(password_conflict).to_be_visible();expect(super_conflict).to_be_visible()
   assert 'nexusbro.v1/password/cred-1' not in dialog.inner_text()
   page.evaluate('window.__offline=true')
   password_conflict.get_by_role('button',name='Оставить на этом устройстве',exact=True).click()
   expect(dialog.get_by_role('alert')).to_contain_text('Сервер недоступен')
   expect(password_conflict).to_be_visible()
   assert 'Синхронизация завершена' not in dialog.inner_text()
   page.evaluate('window.__offline=false')
   password_conflict.get_by_role('button',name='Оставить на этом устройстве',exact=True).click()
   expect(password_conflict).not_to_be_visible();expect(super_conflict).to_be_visible()
   expect(dialog.get_by_role('status')).to_contain_text('Остальные конфликты требуют решения')
   assert 'Синхронизация завершена' not in dialog.inner_text()
   before_resolve=page.evaluate("window.__calls.filter(x=>x.cmd==='security_sync'&&x.action==='resolve').length")
   confirm_accept['value']=False
   super_conflict.get_by_role('button',name='Взять из облака',exact=True).click()
   expect(super_conflict).to_be_visible()
   assert page.evaluate("window.__calls.filter(x=>x.cmd==='security_sync'&&x.action==='resolve').length")==before_resolve
   assert 'Локальная версия будет заменена' in confirmations[-1]
   confirm_accept['value']=True
   page.evaluate("window.__report.errors=['Не удалось синхронизировать другую запись']")
   super_conflict.get_by_role('button',name='Взять из облака',exact=True).click()
   expect(super_conflict).not_to_be_visible()
   expect(dialog.get_by_role('status')).to_contain_text('Проверьте оставшиеся ошибки')
   assert 'Синхронизация завершена' not in dialog.inner_text()
   choices=page.evaluate("window.__calls.filter(x=>x.cmd==='security_sync'&&x.action==='resolve').map(x=>x.data)")
   assert choices[-2:]==[{'id':'nexusbro.v1/password/cred-1','choice':'local'},{'id':'nexusbro.v1/super/normal-super','choice':'remote'}],choices
   page.evaluate('window.__statusConflict=true')
   dialog.get_by_role('button',name='Синхронизировать сейчас',exact=True).click()
   expect(dialog.get_by_text('Обнаружены конфликты.',exact=False)).to_be_visible()
   assert 'Синхронизация завершена' not in dialog.inner_text()
   page.evaluate('window.__statusConflict=false')
   dialog.get_by_role('button',name='Синхронизировать сейчас',exact=True).click()
   expect(dialog.get_by_role('status')).to_contain_text('Синхронизация завершена')
   # A second device starts with an empty local list and must explicitly opt in
   # to receiving newly encountered remote records. This remains an IPC fixture.
   new_device=browser.new_page(viewport={'width':1100,'height':820});new_device.on('pageerror',lambda e:errors.append(str(e)))
   new_device.add_init_script(mock+"window.__vault={exists:true,unlocked:true};window.__account={configured:true,logged_in:true,username:'browser-fixture',server:'https://browser-sync.fixture.invalid',totp_enabled:true};window.__supers=[];window.__rows=[];window.__importData=true;")
   new_device.goto(f'http://127.0.0.1:{server.server_port}')
   fresh=new_device.get_by_role('dialog',name='Пароли, аккаунт и синхронизация NexusBro')
   fresh.get_by_role('button',name='Синхронизация',exact=True).click()
   expect(fresh.get_by_text('Нет доступных обычных супер-вкладок.',exact=True)).to_be_visible()
   expect(fresh.get_by_text('Сначала сохраните пароль.',exact=True)).to_be_visible()
   expect(fresh.get_by_role('checkbox',name='Все супер-вкладки, включая новые',exact=False)).not_to_be_checked()
   expect(fresh.get_by_role('checkbox',name='Все пароли, включая новые',exact=False)).not_to_be_checked()
   fresh.get_by_role('checkbox',name='Все супер-вкладки, включая новые',exact=False).check()
   fresh.get_by_role('checkbox',name='Все пароли, включая новые',exact=False).check()
   fresh.get_by_role('button',name='Сохранить выбор',exact=True).click()
   expect(fresh.get_by_role('status')).to_contain_text('Выбор данных сохранён')
   assert new_device.evaluate('window.__selection')=={'settings':False,'bookmarks':False,'supers':['*'],'passwords':['*']}
   fresh.get_by_role('button',name='Синхронизировать сейчас',exact=True).click()
   expect(fresh.get_by_role('status')).to_contain_text('Синхронизация завершена')
   expect(fresh.get_by_text('Отправлено: 0 · Получено: 2',exact=True)).to_be_visible()
   expect(fresh.get_by_role('checkbox',name='Облачная Super',exact=True)).to_be_checked()
   expect(fresh.get_by_role('checkbox',name='Облачная Super',exact=True)).to_be_disabled()
   expect(fresh.get_by_role('checkbox',name='Облачный пароль',exact=False)).to_be_checked()
   expect(fresh.get_by_role('checkbox',name='Облачный пароль',exact=False)).to_be_disabled()
   new_device.close()
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
   expect(dialog.get_by_role('status')).to_contain_text('Доступ к аккаунту NexusBro восстановлен')
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
   # A previously selected custom endpoint is never replaced by the default.
   custom=browser.new_page(viewport={'width':1100,'height':820});custom.on('pageerror',lambda e:errors.append(str(e)))
   custom.add_init_script(mock+"window.__vault={exists:true,unlocked:true};window.__account={configured:true,logged_in:false,username:'',server:'https://owner-server.fixture.invalid',totp_enabled:false};")
   custom.goto(f'http://127.0.0.1:{server.server_port}')
   cd=custom.get_by_role('dialog',name='Пароли, аккаунт и синхронизация NexusBro')
   cd.get_by_role('button',name='Аккаунт',exact=True).click()
   expect(cd.get_by_text('Используется выбранный вами сервер:',exact=False)).to_be_visible()
   cd.get_by_label('Имя аккаунта NexusBro',exact=True).fill('custom-fixture')
   cd.get_by_label('Пароль аккаунта NexusBro',exact=True).fill('custom-password-fixture')
   cd.get_by_role('button',name='Войти в NexusBro',exact=True).click()
   expect(cd.get_by_role('status')).to_contain_text('Вход в аккаунт NexusBro выполнен')
   assert custom.evaluate('window.__account.server')=='https://owner-server.fixture.invalid'
   assert not custom.evaluate("window.__calls.some(x=>x.cmd==='security_account'&&['configure','default_server'].includes(x.action))")
   custom.close()
   # Connection errors block registration and do not leave a success/pending state.
   offline=browser.new_page(viewport={'width':1100,'height':820});offline.on('pageerror',lambda e:errors.append(str(e)))
   offline.add_init_script(mock+"window.__vault={exists:true,unlocked:true};window.__configureOffline=true;")
   offline.goto(f'http://127.0.0.1:{server.server_port}')
   od=offline.get_by_role('dialog',name='Пароли, аккаунт и синхронизация NexusBro')
   od.get_by_role('button',name='Аккаунт',exact=True).click();od.get_by_role('button',name='Зарегистрироваться',exact=True).click()
   def registration_fields():
    od.get_by_label('Имя аккаунта NexusBro',exact=True).fill('offline-fixture')
    od.get_by_label('Пароль аккаунта NexusBro',exact=True).fill('offline-password-fixture')
    od.get_by_label('Повторите пароль аккаунта',exact=True).fill('offline-password-fixture')
   registration_fields();od.get_by_role('button',name='Создать аккаунт NexusBro',exact=True).click()
   expect(od.get_by_role('alert')).to_contain_text('Сервис аккаунтов временно недоступен')
   expect(od.get_by_role('button',name='Создать аккаунт NexusBro',exact=True)).to_be_visible()
   assert not offline.evaluate("window.__calls.some(x=>x.cmd==='security_account'&&x.action==='register')")
   assert not offline.evaluate('window.__account.logged_in')
   assert 'Аккаунт NexusBro создан' not in od.inner_text()
   # An account created without a successful login must not claim an active session.
   offline.evaluate('window.__configureOffline=false;window.__authPartial=true')
   registration_fields();od.get_by_role('button',name='Создать аккаунт NexusBro',exact=True).click()
   expect(od.get_by_role('region',name='Ключ восстановления аккаунта',exact=True)).to_be_visible()
   expect(od.get_by_role('status')).to_contain_text('Аккаунт создан. Сохраните ключ восстановления и выполните вход.')
   expect(od.get_by_role('alert')).to_contain_text('Автоматический вход не завершён')
   expect(od.get_by_role('button',name='Войти в NexusBro',exact=True)).to_be_visible()
   assert not offline.evaluate('window.__account.logged_in')
   offline.close()
   assert not external_requests,'Security UI made an unexpected external request'
   assert not errors,errors
   print(json.dumps({'real_chromium':True,'native_ipc':'fixture','vault_gate':True,'manual_password_save_and_explicit_reveal':True,'separate_account_and_show_once_recovery':True,'default_account_without_manual_server':True,'custom_endpoint_preserved':True,'connection_failure_blocks_registration':True,'partial_registration_never_claims_login':True,'password_saving_onboarding':True,'local_rendered_totp_qr_independently_decoded':True,'totp_qr_and_manual_key_match':True,'totp_qr_pixels_cleared_on_unmount':True,'no_external_qr_requests':True,'totp_confirmation_and_bad_code':True,'account_change_recover_delete':True,'granular_sync_all_off':True,'new_device_explicit_wildcard_import':True,'offline_and_conflicts_never_success':True,'explicit_conflict_choices_and_cloud_confirmation':True,'capture_consent_and_exact_target':True,'private_target_rejected_before_ipc':True,'small_window_fits':True,'lock_hides_secrets':True,'no_localstorage_secrets':True}))
   browser.close()
 finally:server.shutdown()
