"""Shipped network UI layout and physical Retry; native events/IPC are fixtures."""
import ast,functools,http.server,json,os,pathlib,threading
from playwright.sync_api import sync_playwright,expect
root=pathlib.Path(__file__).resolve().parents[1]
module=ast.parse((root/'tests/frontend_controls.py').read_text())
base=next(ast.literal_eval(n.value) for n in module.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='mock' for t in n.targets))
mock=base+r'''
window.__tabs=[{id:'network',nazvanie:'Сетевой профиль',proxy:'',adres:'about:blank',zapushchena:true,sohranena:false}];window.__active='network';
const old=window.__TAURI_INTERNALS__.invoke;
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='security_vault')return {exists:true,unlocked:true};
 if(cmd==='security_passwords')return [];
 if(cmd==='security_account')return {configured:false,logged_in:false,username:'',server:'',totp_enabled:false};
 if(cmd==='security_sync')return args.action==='history'?{entries:[{entry_id:'old',name:'Архивная Super',reason:'local_delete',created_at:1795000000000,currently_exists:false}]}:{selection:{settings:false,bookmarks:false,supers:[],passwords:[]},available_supers:[],last_sync:null,report:null};
 if(cmd==='obychnye_spisok')return {aktivnaya:1,vkladki:[{id:1,adres:'https://failed.fixture.invalid/',zagolovok:'Сетевой тест',zakreplena:false,zagruzka:false,otlozhena:false,browser_id:11,route:{kind:'direct'},route_generation:4}]};
 return old(cmd,args);
};
window.__showNetworkError=()=>window.__callbacks[window.__events['stranica-ui']]({event:'stranica-ui',payload:{super_id:'network',data:{tip:'navigation-error',id:1,code:-324,route_generation:4}}});
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
try:
 with sync_playwright() as pw:
  browser=pw.chromium.launch(headless=True,channel=os.getenv('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None)
  page=browser.new_page(viewport={'width':900,'height':700});page.add_init_script(mock);page.goto(f'http://127.0.0.1:{server.server_port}')
  address=page.get_by_role('combobox',name='Адрес страницы');expect(address).to_have_value('https://failed.fixture.invalid/')
  page.wait_for_function('window.__events["stranica-ui"]&&document.querySelector(".obychnaya.vybrana")')
  address.fill('https://unsent.fixture.invalid/')
  page.evaluate('window.__showNetworkError()');banner=page.locator('.network-error[role=alert]');expect(banner).to_be_visible()
  expect(banner).to_contain_text('Сайт закрыл соединение без ответа.');expect(banner).to_contain_text('Путь: Прямое соединение')
  layout=banner.evaluate('e=>{const r=e.getBoundingClientRect(),n=document.querySelector("form.navigaciya").getBoundingClientRect(),p=document.querySelector(".pole").getBoundingClientRect();return {outside:!e.closest("form"),top:r.top,bottom:r.bottom,navBottom:n.bottom,pageTop:p.top,width:r.width,scroll:e.scrollWidth,client:e.clientWidth,buttons:[...e.querySelectorAll("button")].map(b=>{const q=b.getBoundingClientRect();return {type:b.type,left:q.left,right:q.right,top:q.top,bottom:q.bottom};})};}')
  assert layout['outside'] and layout['top']>=layout['navBottom']-1 and layout['bottom']<=layout['pageTop']+1,layout
  assert layout['scroll']<=layout['client']+1 and all(b['type']=='button' and 0<=b['left']<b['right']<=900 and b['bottom']<=layout['bottom']+1 for b in layout['buttons']),layout
  before=page.evaluate('window.__args.length');banner.get_by_role('button',name='Повторить',exact=True).click();expect(banner).not_to_be_visible()
  calls=page.evaluate('start=>window.__args.slice(start).filter(c=>c.cmd==="obychnaya_komanda").map(c=>c.args.komanda)',before)
  assert len(calls)==1 and calls[0]=={'deystvie':'obnovit','id':1},calls
  assert not any(c.get('deystvie')=='pereyti' for c in calls),'Retry submitted the address form'
  page.get_by_title('Настройки NexusBro',exact=True).click();settings=page.get_by_role('dialog',name='Настройки NexusBro',exact=True);expect(settings).to_be_visible()
  settings.get_by_role('button',name='Океан',exact=True).click()
  settings.get_by_role('button',name='Пароли, аккаунт и синхронизация',exact=True).click()
  security=page.get_by_role('dialog',name='Пароли, аккаунт и синхронизация NexusBro',exact=True);expect(security).to_be_visible();expect(settings).not_to_be_visible()
  expect(page.locator('[role=dialog]:visible')).to_have_count(1)
  security.get_by_role('button',name='Синхронизация',exact=True).click()
  summary=security.locator('summary').filter(has_text='Корзина и история супервкладок');expect(summary).to_be_visible();summary.click()
  security.get_by_role('button',name='Показать историю',exact=True).click();expect(security.get_by_text('Архивная Super',exact=True)).to_be_visible()
  security.get_by_role('button',name='Закрыть пароли и синхронизацию',exact=True).click();expect(settings).to_be_visible();expect(settings.get_by_role('button',name='Океан',exact=True)).to_have_attribute('aria-pressed','true')
  assert page.evaluate('window.__args.filter(c=>c.cmd==="pokazat_dvizhki").at(-1).args.vidno') is False,'Closing security exposed CEF over preserved settings'
  settings.get_by_role('button',name='Отмена',exact=True).click();expect(settings).not_to_be_visible();page.wait_for_function('window.__args.filter(c=>c.cmd==="pokazat_dvizhki").at(-1)?.args.vidno===true')
  browser.close()
finally:server.shutdown()
print(json.dumps({'real_chromium':True,'native_events_and_ipc':'fixture','banner_outside_address_form':True,'visible_wrapped_route_and_buttons_at_900px':True,'native_content_layout_reserves_banner_height':True,'physical_retry_exactly_one_reload_no_address_submit':True,'settings_security_navigation_one_visible_modal_and_unsaved_settings_preserved':True,'local_history_visible_without_account_login':True}))
