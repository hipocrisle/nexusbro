"""Real Chromium sidebar regression; security/Tauri IPC is a local fixture.

Checks authoritative per-record states, compact layout and nondestructive opt-out.
The native test separately exercises authenticated ACK hashes and a real outage.
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
window.__deferSync=false;window.__deferredSync=[];window.__holdRun=false;window.__runWaiters=[];window.__runCount=0;window.__runConflicts=false;
window.__recordStates={};window.__omitAuthoritative=false;window.__runFails=false;
window.__normalIds=()=>[...new Set([...window.__tabs,...window.__closed].filter(v=>!v.privatnaya&&!v.id.startsWith('pvt-')).map(v=>v.id))];
window.__selected=id=>window.__selection.supers.some(key=>key==='*'||key===id);
window.__status=()=>{
 const states=Object.fromEntries(window.__normalIds().map(id=>[id,{state:window.__selected(id)?(window.__recordStates[id]||'pending'):'off',last_success:window.__lastSync}]));
 const selected=Object.entries(states).filter(([id])=>window.__selected(id)).map(([,value])=>value.state);
 const state=!selected.length?'off':selected.some(s=>s==='error'||s==='conflict')||window.__report.errors.length||window.__report.conflicts.length?'error':selected.every(s=>s==='ok')?'ok':'pending';
 return {selection:structuredClone(window.__selection),last_sync:window.__lastSync,report:structuredClone(window.__report),
  available_supers:window.__normalIds().map(id=>({id,name:[...window.__tabs,...window.__closed].find(v=>v.id===id).nazvanie})),
  ...(window.__omitAuthoritative?{}:{super_states:states,sync_state:{state,last_success:window.__lastSync}})};
};
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 window.__args.push({cmd,args});
 if(cmd==='browser_lock_status')return {locked:false,exists:true,revision:1};
 if(cmd==='security_vault')return structuredClone(window.__vault);
 if(cmd==='security_account'){if(args.action==='logout')window.__account.logged_in=false;return structuredClone(window.__account);}
 if(cmd==='security_passwords')return [];
 if(cmd==='security_sync'){
  const snapshot=window.__status();
  if(args.action==='status'){window.__syncReads++;if(window.__deferSync)return new Promise(resolve=>window.__deferredSync.push(()=>resolve(snapshot)));return snapshot;}
  if(args.action==='configure'){window.__selection=structuredClone(args.data.selection);return window.__status();}
  if(args.action==='run'){
   window.__runCount++;if(window.__holdRun)await new Promise(resolve=>window.__runWaiters.push(resolve));
   if(window.__runConflicts){window.__recordStates['cloud-open']='conflict';window.__report={pushed:0,pulled:0,conflicts:['nexusbro.v1/super/cloud-open'],errors:[]};return structuredClone(window.__report);}
   for(const id of window.__normalIds().filter(window.__selected))window.__recordStates[id]=window.__runFails?'error':'ok';
   if(window.__runFails){window.__report={pushed:0,pulled:0,conflicts:[],errors:['Fixture transport failed']};throw Error('Fixture transport failed');}
   window.__lastSync=1795000000000;window.__report={pushed:2,pulled:0,conflicts:[],errors:[]};return structuredClone(window.__report);
  }
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

def cloud_badge(tab):
 return tab.locator('..').locator('.super-cloud-badge')

def no_row_clouds(page):
 expect(page.locator('.super-cloud-badge')).to_have_count(0)

def color_kind(locator, expected):
 rgb=locator.evaluate('e=>getComputedStyle(e).color.match(/\\d+/g).slice(0,3).map(Number)')
 red,green,blue=rgb
 assert {'green':green>red*1.15 and green>blue*1.05,'red':red>green*1.25 and red>blue*1.15,
         'gray':max(rgb)-min(rgb)<=55}[expected],(expected,rgb)

def super_hint(tab,name):
 # Native title popups can intercept clicks in the embedded Linux shell.
 # Require the actual DOM hint; absence is a regression, never an empty fallback.
 hint=tab.get_attribute('data-super-hint')
 assert isinstance(hint,str) and hint.startswith(name),hint
 assert tab.get_attribute('title') is None
 assert tab.locator('[title]').count()==0
 aria=tab.get_attribute('aria-label')
 assert isinstance(aria,str) and aria.startswith(name),aria
 paint=tab.evaluate("e=>{const p=getComputedStyle(e,'::after');return {content:p.content,pointer:p.pointerEvents,background:p.backgroundColor}}")
 assert name in paint['content'],paint
 assert paint['pointer']=='none',paint
 assert paint['background'] not in ('transparent','rgba(0, 0, 0, 0)'),paint
 return hint

def compact_header(page):
 no_row_clouds(page)
 header=page.get_by_test_id('cloud-last-account-sync')
 expect(header.locator('button.super-cloud-heading-icon')).to_have_count(1)
 expect(header).to_have_text('Облако')
 assert header.bounding_box()['height']<=32,header.bounding_box()
 assert 'Последняя успешная синхронизация аккаунта' in header.get_attribute('title')
 assert '1795' not in header.inner_text()
 return header

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
  no_row_clouds(page)
  expect(page.locator('[data-cloud-group="local"]')).to_be_visible()
  expect(cloud_badge(opened)).to_have_count(0);expect(cloud_badge(local)).to_have_count(0)
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
  no_row_clouds(page)
  assert cloud_badge(private).count()==0
  assert page.locator('.vkladka[data-super-id="cloud-open"]').count()==1
  assert page.locator('.vkladka[data-super-id="pvt-saved"]').count()==0
  assert page.evaluate('window.__started.length')==0
  pending_hint=super_hint(opened,'Облачная открытая')
  assert 'Ожидает' in pending_hint and 'Подтверждена' not in pending_hint
  super_hint(saved,'Облачная сохранённая');super_hint(private,'Приватная Super')
  # Closing a modal restores CEF on the next animation frame. Deliberately
  # delay that frame and await its real IPC before attributing later commands
  # to the group-sync button. Keep the no-engine-side-effects assertion below.
  page.evaluate('window.__restoreFrame=window.requestAnimationFrame.bind(window);window.requestAnimationFrame=cb=>window.__restoreFrame(t=>setTimeout(()=>cb(t),30));true')
  dialog.get_by_role('button',name='Закрыть',exact=True).click()
  page.wait_for_function('!document.querySelector(".security-panel")&&window.__args.filter(c=>c.cmd==="pokazat_dvizhki").at(-1)?.args.vidno===true')
  page.evaluate('window.requestAnimationFrame=window.__restoreFrame;delete window.__restoreFrame')
  # The actual shipped pseudo-tooltip paints on hover without a native TITLE
  # or an input-catching DOM layer. This changes no cloud state or IPC.
  opened.hover()
  page.wait_for_function("(()=>{const e=document.querySelector('.vkladka[data-super-id=cloud-open]'),p=e&&getComputedStyle(e,'::after');return p?.visibility==='visible'&&Number(p.opacity)>.99})()")
  super_hint(opened,'Облачная открытая')
  heading_sync=page.get_by_role('button',name='Синхронизировать выбранные данные',exact=True)
  selection_before=page.evaluate('JSON.stringify(window.__selection)')
  active_before=page.evaluate('window.__active');args_before=page.evaluate('window.__args.length')
  page.evaluate('window.__holdRun=true')
  heading_sync.focus();heading_sync.press('Enter')
  page.wait_for_function('window.__runCount===1')
  expect(heading_sync).to_be_disabled();expect(heading_sync).to_have_attribute('aria-busy','true')
  no_row_clouds(page)
  expect(opened).to_have_attribute('data-cloud-state','pending')
  box=heading_sync.bounding_box();page.mouse.click(box['x']+box['width']/2,box['y']+box['height']/2)
  page.wait_for_timeout(80);assert page.evaluate('window.__runCount')==1
  assert page.evaluate('JSON.stringify(window.__selection)')==selection_before
  page.evaluate('window.__holdRun=false;window.__runWaiters.splice(0).forEach(resolve=>resolve())')
  expect(opened).to_have_attribute('data-cloud-state','selected')
  expect(heading_sync).to_be_enabled()
  assert page.evaluate('window.__active')==active_before
  assert not page.evaluate('(start)=>window.__args.slice(start).some(c=>["pereklyuchit","vosstanovit","pokazat_dvizhki"].includes(c.cmd))',args_before)
  assert not page.evaluate('window.__started.length')
  assert page.locator('button button').count()==0
  runs=page.evaluate('window.__runCount');heading_sync.click()
  page.wait_for_function('(before)=>window.__runCount===before+1',arg=runs)
  expect(heading_sync).to_be_enabled();assert not page.evaluate('window.__started.length')
  assert page.evaluate('window.__active')==active_before
  assert page.evaluate('JSON.stringify(window.__selection)')==selection_before
  # Late duplicated window events may invalidate the immediate post-run read.
  # Retain an unknown state until the newest authoritative read resolves.
  page.evaluate('window.__deferSync=true;window.__holdRun=true')
  runs=page.evaluate('window.__runCount');heading_sync.click()
  page.wait_for_function('(before)=>window.__runCount===before+1',arg=runs)
  page.wait_for_function('window.__deferredSync.length>=1')
  page.evaluate('window.__holdRun=false;window.__runWaiters.splice(0).forEach(resolve=>resolve())')
  page.wait_for_function('window.__deferredSync.length>=2')
  page.evaluate("window.__cloudChanged({area:'sync',action:'run',phase:'end',opId:'late-window-event'})")
  page.wait_for_function('window.__deferredSync.length>=3')
  page.evaluate('window.__deferredSync.splice(0,2).forEach(resolve=>resolve())')
  expect(heading_sync).to_be_enabled();expect(opened).to_have_attribute('data-cloud-state','paused')
  page.evaluate('window.__deferSync=false;window.__deferredSync.splice(0).forEach(resolve=>resolve())')
  expect(opened).to_have_attribute('data-cloud-state','selected')
  receipt=page.get_by_test_id('cloud-last-account-sync')
  expect(receipt).to_have_attribute('data-last-sync','1795000000000')
  compact_header(page);color_kind(receipt.locator('.super-cloud-heading-icon'),'green')
  assert opened.get_attribute('data-cloud-state')!='synced'
  no_row_clouds(page);expect(heading_sync).to_be_visible()
  assert opened.evaluate('e=>getComputedStyle(e).backgroundColor')==before_fill
  assert opened.evaluate('e=>getComputedStyle(e).boxShadow')==before_shadow
  color_kind(heading_sync,'green');expect(cloud_badge(local)).to_have_count(0)

  # Opt out through actual SecuritySettings controls. A closed saved Super must
  # move to Local, preserving its data and staying dormant, with no delete IPC.
  saved_before=page.evaluate('JSON.stringify(window.__closed)')
  page.evaluate("window.dispatchEvent(new Event('nexusbro-security'))")
  dialog.get_by_role('button',name='Синхронизация',exact=True).click()
  dialog.get_by_role('checkbox',name='Облачная сохранённая',exact=True).uncheck()
  dialog.get_by_role('button',name='Сохранить выбор',exact=True).click()
  expect(saved).to_have_attribute('data-cloud-state','local')
  expect(page.locator('[data-cloud-group="local"] [data-super-id="cloud-saved"]')).to_have_count(1)
  expect(saved).to_have_attribute('data-cloud-saved','true');expect(cloud_badge(saved)).to_have_count(0)
  assert page.evaluate('JSON.stringify(window.__closed)')==saved_before
  assert not page.evaluate('window.__started.length')
  assert not page.evaluate('window.__args.some(c=>/delete|udalit|zakryt/i.test(c.cmd))')
  dialog.get_by_role('button',name='Закрыть',exact=True).click()
  page.screenshot(path='/tmp/nexusbro-cloud-disabled-saved-local.png')
  page.evaluate("window.dispatchEvent(new Event('nexusbro-security'))")
  dialog.get_by_role('button',name='Синхронизация',exact=True).click()
  dialog.get_by_role('checkbox',name='Облачная сохранённая',exact=True).check()
  dialog.get_by_role('button',name='Сохранить выбор',exact=True).click()
  dialog.get_by_role('button',name='Синхронизировать сейчас',exact=True).click()
  expect(saved).to_have_attribute('data-cloud-state','selected')
  dialog.get_by_role('button',name='Закрыть',exact=True).click()

  # Selected closed records are discoverable but no engine starts until click.
  saved.click()
  page.wait_for_function("window.__started.length===1")
  assert page.evaluate('window.__started')==['cloud-saved']
  expect(page.locator('.vkladka[data-super-id="cloud-saved"]')).to_have_count(1)
  expect(page.locator('.vkladka[data-super-id="cloud-saved"]')).not_to_have_attribute('data-cloud-saved','true')
  assert page.locator('[data-cloud-group="cloud"] .vkladka').count()==2
  page.get_by_role('button',name='Свернуть левую панель',exact=True).click()
  expect(opened.locator('.imya')).not_to_be_visible()
  no_row_clouds(page);expect(heading_sync).to_be_visible()
  expect(page.locator('[data-cloud-group="cloud"] .super-cloud-heading-icon svg')).to_be_visible()
  for button,name in [(opened,'Облачная открытая'),(local,'Локальная открытая'),(private,'Приватная Super')]:super_hint(button,name)
  for button,short in [(opened,'Обл'),(local,'Лок'),(private,'При')]:
   expect(button.locator('.kratkoe-imya')).to_have_text(short)
   box=button.locator('.kratkoe-imya').bounding_box();row=button.bounding_box()
   assert box['x']>=row['x'] and box['x']+box['width']<=row['x']+row['width']
   assert button.locator('.kratkoe-imya').evaluate('(e)=>e.scrollWidth<=e.clientWidth')
  compact_header(page)
  runs=page.evaluate('window.__runCount');heading_sync.focus();heading_sync.press('Space')
  page.wait_for_function('(before)=>window.__runCount===before+1',arg=runs)
  expect(heading_sync).to_be_enabled();assert page.evaluate('window.__active')=='cloud-saved'
  no_row_clouds(page)
  badge=heading_sync.bounding_box();row=receipt.bounding_box()
  assert badge['x']>=row['x'] and badge['x']+badge['width']<=row['x']+row['width']
  assert badge['y']>=row['y'] and badge['y']+badge['height']<=row['y']+row['height']
  expect(private.locator('.super-private-badge')).to_be_visible()
  assert cloud_badge(private).count()==0
  page.screenshot(path='/tmp/nexusbro-cloud-collapsed.png')
  page.get_by_role('button',name='Развернуть левую панель',exact=True).click()

  # A historical account receipt cannot turn a dirty or legacy/unacknowledged
  # record acknowledged. Another row retains its own ACK evidence while the
  # single aggregate cloud remains gray until every selected row is confirmed.
  page.evaluate("window.__recordStates['cloud-open']='pending';window.__cloudChanged()")
  expect(opened).to_have_attribute('data-cloud-state','pending')
  expect(saved).to_have_attribute('data-cloud-state','selected')
  color_kind(heading_sync,'gray')
  expect(receipt).to_have_attribute('data-cloud-state','pending')
  expect(receipt).to_have_attribute('data-last-sync','1795000000000')
  page.evaluate("window.__omitAuthoritative=true;window.__cloudChanged()")
  expect(saved).to_have_attribute('data-cloud-state','pending')
  expect(opened).to_have_attribute('data-cloud-state','pending')
  page.evaluate("window.__omitAuthoritative=false;window.__recordStates['cloud-open']='ok';window.__cloudChanged()")
  expect(opened).to_have_attribute('data-cloud-state','selected')

  # A conflict is per-record evidence; an account receipt alone never clears it.
  page.evaluate("window.__recordStates['cloud-open']='conflict';window.__report.conflicts=['nexusbro.v1/super/cloud-open'];window.__cloudChanged()")
  expect(opened).to_have_attribute('data-cloud-state','conflict')
  assert 'Конфликт версий' in super_hint(opened,'Облачная открытая')
  expect(receipt).to_have_attribute('data-cloud-state','error')
  no_row_clouds(page)
  color_kind(heading_sync,'red');color_kind(receipt.locator('.super-cloud-heading-icon'),'red')
  page.screenshot(path='/tmp/nexusbro-cloud-red.png')
  expect(page.locator('.vkladka[data-super-id="cloud-saved"]')).to_have_attribute('data-cloud-state','selected')

  # Start/end events hide stale green while work is in progress. Duplicate or
  # late starts after a completed operation cannot leave the sidebar busy.
  page.evaluate("window.__report.conflicts=[];window.__recordStates['cloud-open']='ok';window.__cloudChanged({area:'sync',action:'run',phase:'start',opId:'native-operation'})")
  expect(opened).to_have_attribute('data-cloud-state','pending')
  page.evaluate("window.__cloudChanged({area:'sync',action:'run',phase:'end',opId:'native-operation',failed:false});window.__cloudChanged({area:'sync',action:'run',phase:'start',opId:'native-operation'})")
  expect(opened).to_have_attribute('data-cloud-state','selected')

  # Run the sidebar sync handler against a failed transport fixture, persist
  # the returned authoritative error, and reopen a fresh renderer: it stays red.
  page.evaluate('window.__runFails=true');heading_sync.click()
  expect(opened).to_have_attribute('data-cloud-state','error')
  expect(heading_sync).to_be_enabled()
  expect(page.locator('.oshibka')).to_contain_text('Fixture transport failed')
  color_kind(heading_sync,'red');color_kind(receipt.locator('.super-cloud-heading-icon'),'red')
  persisted_error=page.evaluate('({__tabs:window.__tabs,__closed:window.__closed,__account:window.__account,__selection:window.__selection,__lastSync:window.__lastSync,__report:window.__report,__recordStates:window.__recordStates})')
  failed_context=browser.new_context(viewport={'width':1280,'height':850})
  failed_page=failed_context.new_page();failed_page.on('pageerror',lambda e:errors.append(str(e)))
  failed_page.add_init_script(mock+'\nObject.assign(window,'+json.dumps(persisted_error)+');')
  failed_page.goto(f'http://127.0.0.1:{server.server_port}')
  expect(failed_page.locator('[data-super-id="cloud-open"]')).to_have_attribute('data-cloud-state','error')
  color_kind(failed_page.locator('[data-cloud-group="cloud"] .super-cloud-heading-icon'),'red')
  no_row_clouds(failed_page)
  failed_context.close()
  page.evaluate('window.__runFails=false');heading_sync.click()
  expect(opened).to_have_attribute('data-cloud-state','selected')
  page.evaluate('window.__runConflicts=true');heading_sync.click()
  expect(opened).to_have_attribute('data-cloud-state','conflict');color_kind(heading_sync,'red')
  expect(page.locator('.oshibka')).to_contain_text('Конфликт версий')
  page.evaluate("window.__runConflicts=false;window.__recordStates['cloud-open']='pending'")
  heading_sync.click();expect(opened).to_have_attribute('data-cloud-state','selected')

  # Wildcard membership excludes private definitions and adds no engine calls.
  page.evaluate("window.__selection.supers=['*'];window.__cloudChanged()")
  expect(local).to_have_attribute('data-cloud-state','pending')
  expect(private).to_have_attribute('data-cloud-state','local')
  assert cloud_badge(private).count()==0
  page.evaluate("window.__account.logged_in=false;window.__cloudChanged({area:'account',action:'logout'})")
  expect(opened).to_have_attribute('data-cloud-state','local')
  expect(page.locator('[data-cloud-group="local"] [data-super-id="cloud-open"]')).to_have_count(1)
  expect(cloud_badge(opened)).to_have_count(0)
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
  no_row_clouds(page)

  # Reopening the app reads persisted backend selection, not a browser cache.
  page.evaluate("window.__selection.supers=['cloud-open'];window.__lastSync=1795000000000;window.__cloudChanged()")
  expect(opened).to_have_attribute('data-cloud-state','selected')
  page.screenshot(path='/tmp/nexusbro-cloud-expanded.png')
  persisted=page.evaluate('({__tabs:window.__tabs,__closed:window.__closed,__account:window.__account,__selection:window.__selection,__lastSync:window.__lastSync,__report:window.__report,__recordStates:window.__recordStates})')
  fresh_context=browser.new_context(viewport={'width':1280,'height':850})
  fresh=fresh_context.new_page();fresh.on('pageerror',lambda e:errors.append(str(e)))
  fresh.add_init_script(mock+'\nObject.assign(window,'+json.dumps(persisted)+');')
  fresh.goto(f'http://127.0.0.1:{server.server_port}')
  expect(fresh.locator('.vkladka[data-super-id="cloud-open"]')).to_have_attribute('data-cloud-state','selected')
  expect(fresh.get_by_test_id('cloud-last-account-sync')).to_have_attribute('data-last-sync','1795000000000')
  assert fresh.evaluate('window.__started.length')==0
  no_row_clouds(fresh)
  no_row_clouds(page)
  assert fresh.evaluate('localStorage.length')==0
  fresh_context.close()
  assert page.evaluate('localStorage.length')==0
  no_row_clouds(page)
  assert not errors,errors
  print(json.dumps({'real_chromium':True,'tauri_ipc':'fixture','green_requires_authoritative_per_record_ack':True,'historical_account_date_and_legacy_status_do_not_turn_green':True,'compact_header_32px_date_tooltip_only':True,'green_red_gray_actual_computed_colors':True,'disabled_saved_moves_local_data_unchanged_no_delete_no_start':True,'expanded_collapsed_cloud_and_private_identity':True,'colors_and_active_indicator_preserved':True,'actual_settings_bridge_invalidates_sidebar':True,'failed_exchange_red_rehydrates_after_restart':True,'operation_events_ignore_late_duplicate_start':True,'no_protected_status_polling':True,'stale_account_status_rejected':True,'cold_renderer_rehydrates_backend_selection':True,'no_plaintext_browser_cache':True,'all_open_saved_local_private_rows_have_no_cloud_badges':True,'only_group_header_cloud_sync_control':True,'group_header_click_enter_space_retry_busy_dedup_no_switch_or_engine_start':True,'superseded_post_run_metadata_cannot_reuse_old_green':True}))
  browser.close()
finally:server.shutdown()
