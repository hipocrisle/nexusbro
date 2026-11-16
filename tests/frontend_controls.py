"""Проверка DOM-списков и выбора узла мышью. Tauri IPC заменён локальной фикстурой.
Запускается в CI после сборки frontend; native CEF проверяет windows_physical.py.
"""
import functools,http.server,json,pathlib,threading
from playwright.sync_api import sync_playwright,expect
root=pathlib.Path(__file__).resolve().parents[1]
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(http.server.SimpleHTTPRequestHandler,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
mock=r'''window.__saved=null;window.__calls=[];let cb=0;
window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}},transformCallback:()=>++cb,unregisterCallback:()=>{},invoke:async(cmd,args)=>{
 window.__calls.push(cmd);
 if(cmd==='nastrojki_poluchit')return {tema:'temnaya',shrift:'system',razmer_shrifta:14,plotnost:'udobno',masshtab_stranic:100,vyhody:[{id:'local',nazvanie:'Test subscription',adres:'https://example.test/sub',proxy_uzel:''}],panel_shirina:180,panel_svernuta:false,novaya_stranica:'about:blank',poisk:'duckduckgo',avtoobnovleniya:false,apparatnoe_uskorenie:false};
 if(cmd==='nastrojki_sohranit'){window.__saved=args.nastrojki;return args.nastrojki;}
 if(cmd==='spisok'||cmd==='sohranennye')return [];
 if(cmd==='aktivnaya')return null;
 if(cmd==='dvizhok_est')return true;
 if(cmd==='uzly_podpiski')return [{imya:'Unavailable',ssylka:'vless://fixture@127.0.0.1:1'},{imya:'Local second',ssylka:'vless://fixture@127.0.0.1:2'}];
 if(cmd==='plugin:app|version')return 'test';
 if(cmd==='plugin:event|listen')return 1;
 return null;
}};'''
try:
 with sync_playwright() as pw:
  browser=pw.chromium.launch(channel='chrome',headless=True,args=['--no-sandbox'])
  page=browser.new_page(viewport={'width':1280,'height':820});page.add_init_script(mock)
  errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
  page.goto(f'http://127.0.0.1:{server.server_port}')
  page.get_by_title('Настройки NexusBro',exact=True).click()
  page.get_by_role('combobox',name='Шрифт интерфейса',exact=True).click()
  page.get_by_role('option',name='Arial',exact=True).click()
  expect(page.get_by_role('combobox',name='Шрифт интерфейса',exact=True)).to_contain_text('Arial')
  page.get_by_role('combobox',name='Плотность интерфейса',exact=True).click()
  page.get_by_role('option',name='Компактная',exact=True).click()
  page.get_by_role('button',name='Сеть и VPN',exact=True).click()
  page.locator('.subscription-nodes summary').click()
  page.get_by_role('combobox',name='Узел сохранённой подписки',exact=True).click()
  page.get_by_role('option',name='Local second',exact=True).click()
  page.get_by_role('button',name='Сохранить',exact=True).click()
  page.wait_for_function('window.__saved!==null')
  saved=page.evaluate('window.__saved');assert saved['shrift']=='arial' and saved['plotnost']=='kompaktno',saved
  assert saved['vyhody'][0]['proxy_uzel'].endswith(':2'),saved
  page.locator('.niz button').first.click()
  page.get_by_role('combobox',name='Сохранённое подключение',exact=True).click()
  page.get_by_role('option',name='Test subscription',exact=True).click()
  expect(page.get_by_role('combobox',name='Узел подписки',exact=True)).to_contain_text('Local second')
  page.get_by_role('combobox',name='Узел подписки',exact=True).click()
  page.get_by_role('option',name='Unavailable',exact=True).click()
  expect(page.get_by_role('combobox',name='Узел подписки',exact=True)).to_contain_text('Unavailable')
  assert page.locator('select').count()==0
  assert not errors,errors
  print(json.dumps({'mouse_font_density':True,'all_subscription_nodes':True,'saved_node_default_in_super':True,'super_node_mouse_selection':True,'no_native_select':True}))
  browser.close()
finally:server.shutdown()
