"""Real Chromium sidebar regression; security/Tauri IPC is a local fixture.

Checks selection versus historical account receipt, not per-record cloud ACKs.
The native security test separately exercises actual encrypted sync and restart.
"""
import ast
import functools
import http.server
import json
import os
import pathlib
import threading
from playwright.sync_api import expect, sync_playwright

root=pathlib.Path(__file__).resolve().parents[1]
module=ast.parse((root/'tests/frontend_controls.py').read_text())
base=next(ast.literal_eval(node.value) for node in module.body if isinstance(node,ast.Assign) and any(isinstance(target,ast.Name) and target.id=='mock' for target in node.targets))
mock=base+r'''
const originalInvoke=window.__TAURI_INTERNALS__.invoke;
window.__tabs=[
 {id:'cloud-open',nazvanie:'Облачная открытая',proxy:'',adres:'about:blank',cvet:'#5b9ee8',privatnaya:false,zapushchena:true},
 {id:'local-open',nazvanie:'Локальная открытая',proxy:'',adres:'about:blank',cvet:'#72b566',privatnaya:false,zapushchena:true},
 {id:'pvt-private',nazvanie:'Приватная Super',proxy:'',adres:'about:blank',cvet:'#b48ad8',privatnaya:true,zapushchena:false}
];window.__active='cloud-open';
window.__closed=[{id:'cloud-saved',nazvanie:'Облачная сохранённая',proxy:'',adres:'about:blank',cvet:'#d4ae55',privatnaya:false},structuredClone(window.__tabs[0]),{id:'pvt-saved',nazvanie:'Приватная сохранённая',proxy:'',adres:'about:blank',privatnaya:true}];
window.__vault={exists:true,unlocked:true};window.__account={configured:true,logged_in:true,username:'browser-fixture',server:'https://browser-sync.fixture.invalid',totp_enabled:false,user_id:'account-one'};
window.__selection={settings:false,bookmarks:false,supers:[],passwords:[]};window.__lastSync=null;window.__report={pushed:0,pulled:0,conflicts:[],errors:[]};window.__syncReads=0;window.__started=[];
window.__deferSync=false;window.__deferredSync=[];
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 window.__args.push({cmd,args});
 if(cmd==='browser_lock_status')return {locked:false,exists:true,revision:1};
 if(cmd==='security_vault')return structuredClone(window.__vault);
 if(cmd==='security_account'){if(args.action==='logout')window.__account.logged_in=false;return structuredClone(window.__account);}
 if(cmd==='security_passwords')return [];
 if(cmd==='security_sync'){
  const snapshot={selection:structuredClone(window.__selection),last_sync:window.__lastSync,report:structuredClone(window.__report),available_supers:[{id:'cloud-open',name:'Облачная открытая'},{id:'cloud-saved',name:'Облачная сохранённая'},{id:'local-open',name:'Локальная открытая'}]};
  if(args.action==='status'){window.__syncReads++;if(window.__deferSync)return new Promise(resolve=>window.__deferredSync.push(()=>resolve(snapshot)));return snapshot;}
  if(args.action==='configure'){window.__selection=structuredClone(args.data.selection);return {...snapshot,selection:structuredClone(window.__selection)};}
  if(args.action==='run'){window.__lastSync=1795000000000;window.__report={pushed:2,pulled:0,conflicts:[],errors:[]};return structuredClone(window.__report);}
 }
 if(cmd==='sohranennye')return structuredClone(window.__closed);
 if(cmd==='vosstanovit'){window.__started.push(args.id);const row=window.__closed.find(v=>v.id===args.id);if(!window.__tabs.some(v=>v.id===args.id))window.__tabs.push({...row,zapushchena:true,sohranena:true});window.__active=args.id;return {ok:true,soobshchenie:''};}
 if(cmd==='plugin:event|emit')return null;
 return originalInvoke(cmd,args);
};
window.__cloudChanged=(detail={})=>window.dispatchEvent(new CustomEvent('nexusbro-security-state-changed',{detail}));
'''

class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*args):pass

server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
try:
 with sync_playwright() as p:
  browser=p.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None,headless=True,args=['--no-sandbox'])
  page=browser.new_page(viewport={'width':1280,'height':850});errors=[];page.on('pageerror',lambda e:errors.append(str(e)));page.add_init_script(mock)
  page.goto(f'http://127.0.0.1:{server.server_port}')
  opened=page.locator('.vkladka[data-super-id="cloud-open"]')
  local=page.locator('.vkladka[data-super-id="local-open"]')
  private=page.locator('.vkladka[data-super-id="pvt-private"]')
  expect(opened).to_have_attribute('data-cloud-state','local')
  page.wait_for_function('window.__syncReads>0')
  assert page.locator('[data-cloud-group="cloud"]').count()==0
  before_fill=opened.evaluate('e=>getComputedStyle(e).backgroundColor')
  before_shadow=opened.evaluate('e=>getComputedStyle(e).boxShadow')
  assert not page.evaluate('window.__started.length')

  # Actual settings component invokes the instrumented bridge; no refresh event
  # is injected by the test for this first selection/change/run path.
  page.evaluate("window.dispatchEvent(new Event('nexusbro-security'))")
  dialog=page.get_by_role('dialog',name='Пароли, аккаунт и синхронизация NexusBro')
  dialog.get_by_role('button',name='Синхронизация',exact=True).click()
  dialog.get_by_role('checkbox',name='Облачная открытая',exact=True).check()
  dialog.get_by_role('checkbox',name='Облачная сохранённая',exact=True).check()
  dialog.get_by_role('button',name='Сохранить выбор',exact=True).click()
  expect(opened).to_have_attribute('data-cloud-state','pending')
  saved=page.locator('.vkladka[data-super-id="cloud-saved"]')
  expect(saved).to_have_attribute('data-cloud-saved','true')
  expect(saved).to_have_attribute('data-cloud-state','pending')
  expect(local).to_have_attribute('data-cloud-state','local');expect(private).to_have_attribute('data-cloud-state','local')
  assert private.locator('.super-cloud-badge').count()==0
  assert page.locator('.vkladka[data-super-id="cloud-open"]').count()==1
  assert page.locator('.vkladka[data-super-id="pvt-saved"]').count()==0
  assert page.evaluate('window.__started.length')==0
  assert 'Подтверждена' not in opened.get_attribute('title')
  dialog.get_by_role('button',name='Синхронизировать сейчас',exact=True).click()
  expect(opened).to_have_attribute('data-cloud-state','selected')
  receipt=page.get_by_test_id('cloud-last-account-sync')
  expect(receipt).to_have_attribute('data-last-sync','1795000000000')
  expect(receipt).to_contain_text('Последняя успешная синхронизация аккаунта')
  assert opened.get_attribute('data-cloud-state')!='synced'
  dialog.get_by_role('button',name='Закрыть',exact=True).click()
  expect(opened.locator('.super-cloud-badge')).to_have_count(0)
  assert opened.evaluate('e=>getComputedStyle(e).backgroundColor')==before_fill
  assert opened.evaluate('e=>getComputedStyle(e).boxShadow')==before_shadow

  # Selected closed records are discoverable but no engine starts until click.
  saved.click()
  page.wait_for_function("window.__started.length===1")
  assert page.evaluate('window.__started')==['cloud-saved']
  expect(page.locator('.vkladka[data-super-id="cloud-saved"]')).to_have_count(1)
  expect(page.locator('.vkladka[data-super-id="cloud-saved"]')).not_to_have_attribute('data-cloud-saved','true')
  assert page.locator('[data-cloud-group="cloud"] .vkladka').count()==2
  page.get_by_role('button',name='Свернуть левую панель',exact=True).click()
  expect(opened.locator('.imya')).not_to_be_visible()
  expect(opened.locator('.super-cloud-badge')).to_have_count(0)
  expect(page.locator('[data-cloud-group="cloud"] .super-cloud-heading-icon svg')).to_be_visible()
  for button,short in [(opened,'Обл'),(local,'Лок'),(private,'При')]:
   expect(button.locator('.kratkoe-imya')).to_have_text(short)
   box=button.locator('.kratkoe-imya').bounding_box();row=button.bounding_box()
   assert box['x']>=row['x'] and box['x']+box['width']<=row['x']+row['width']
   assert button.locator('.kratkoe-imya').evaluate('(e)=>e.scrollWidth<=e.clientWidth')
  expect(private.locator('.super-private-badge')).to_be_visible()
  assert private.locator('.super-cloud-badge').count()==0
  page.screenshot(path='/tmp/nexusbro-cloud-collapsed.png')
  page.get_by_role('button',name='Развернуть левую панель',exact=True).click()

  # A conflict is per-record proof; an account receipt alone never clears it.
  page.evaluate("window.__report.conflicts=['nexusbro.v1/super/cloud-open'];window.__cloudChanged()")
  expect(opened).to_have_attribute('data-cloud-state','conflict')
  assert 'Конфликт версий' in opened.locator('.super-cloud-badge').get_attribute('title')
  expect(page.locator('.vkladka[data-super-id="cloud-saved"]')).to_have_attribute('data-cloud-state','selected')
  page.evaluate("window.__report.conflicts=[];window.__cloudChanged({area:'sync',action:'run',failed:true})")
  expect(opened).to_have_attribute('data-cloud-state','pending')
  expect(receipt).to_have_attribute('data-last-sync','1795000000000')
  page.evaluate("window.__cloudChanged({area:'sync',action:'run',failed:false})")
  expect(opened).to_have_attribute('data-cloud-state','selected')

  # Wildcard membership excludes private definitions and adds no engine calls.
  page.evaluate("window.__selection.supers=['*'];window.__cloudChanged()")
  expect(local).to_have_attribute('data-cloud-state','selected')
  expect(private).to_have_attribute('data-cloud-state','local')
  assert private.locator('.super-cloud-badge').count()==0
  page.evaluate("window.__account.logged_in=false;window.__cloudChanged({area:'account',action:'logout'})")
  expect(opened).to_have_attribute('data-cloud-state','paused')
  assert 'Войдите в аккаунт' in opened.locator('.super-cloud-badge').get_attribute('title')
  page.evaluate("window.__account.logged_in=true;window.__cloudChanged({area:'account',action:'login'})")
  expect(opened).to_have_attribute('data-cloud-state','selected')

  # No polling of protected data: idle wait creates no new metadata reads.
  page.wait_for_timeout(150)
  count=page.evaluate('window.__syncReads')
  page.wait_for_timeout(2200)
  assert page.evaluate('window.__syncReads')==count
  page.evaluate("window.dispatchEvent(new Event('focus'))")
  page.wait_for_function('(count)=>window.__syncReads>count',arg=count)

  # Late status from the previous account cannot resurrect its cloud selection.
  page.evaluate("window.__deferSync=true;window.__cloudChanged()")
  page.wait_for_function('window.__deferredSync.length>0')
  page.evaluate("window.__deferSync=false;window.__account.username='other-browser-owner';window.__account.user_id='account-two';window.__selection.supers=[];window.__lastSync=null;window.__cloudChanged()")
  expect(opened).to_have_attribute('data-cloud-state','local')
  page.evaluate('window.__deferredSync.splice(0).forEach(resolve=>resolve())')
  page.wait_for_timeout(150)
  expect(opened).to_have_attribute('data-cloud-state','local')
  assert page.locator('[data-cloud-group="cloud"]').count()==0

  # Reopening the app reads persisted backend selection, not a browser cache.
  page.evaluate("window.__selection.supers=['cloud-open'];window.__lastSync=1795000000000;window.__cloudChanged()")
  expect(opened).to_have_attribute('data-cloud-state','selected')
  page.screenshot(path='/tmp/nexusbro-cloud-expanded.png')
  persisted=page.evaluate('({__tabs:window.__tabs,__closed:window.__closed,__account:window.__account,__selection:window.__selection,__lastSync:window.__lastSync,__report:window.__report})')
  fresh_context=browser.new_context(viewport={'width':1280,'height':850})
  fresh=fresh_context.new_page();fresh.on('pageerror',lambda e:errors.append(str(e)))
  fresh.add_init_script(mock+'\nObject.assign(window,'+json.dumps(persisted)+');')
  fresh.goto(f'http://127.0.0.1:{server.server_port}')
  expect(fresh.locator('.vkladka[data-super-id="cloud-open"]')).to_have_attribute('data-cloud-state','selected')
  expect(fresh.get_by_test_id('cloud-last-account-sync')).to_have_attribute('data-last-sync','1795000000000')
  assert fresh.evaluate('window.__started.length')==0
  assert fresh.evaluate('localStorage.length')==0
  fresh_context.close()
  assert page.evaluate('localStorage.length')==0
  assert not errors,errors
  print(json.dumps({'real_chromium':True,'tauri_ipc':'fixture','selection_is_not_per_super_confirmation':True,'selected_saved_discoverable_deduplicated_manual_open_only':True,'expanded_collapsed_cloud_and_private_identity':True,'colors_and_active_indicator_preserved':True,'actual_settings_bridge_invalidates_sidebar':True,'failed_run_and_conflict_are_visible':True,'no_protected_status_polling':True,'stale_account_status_rejected':True,'cold_renderer_rehydrates_backend_selection':True,'no_plaintext_browser_cache':True}))
  browser.close()
finally:server.shutdown()
