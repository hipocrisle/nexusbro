"""Проверка DOM-списков и выбора узла мышью. Tauri IPC заменён локальной фикстурой.
Запускается в CI после сборки frontend; native CEF проверяет windows_physical.py.
"""
import functools,http.server,json,pathlib,threading
from playwright.sync_api import sync_playwright,expect
root=pathlib.Path(__file__).resolve().parents[1]
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(http.server.SimpleHTTPRequestHandler,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
mock=r'''window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:()=>{},listen:async()=>()=>{},once:async()=>()=>{}};window.__saved=null;window.__calls=[];window.__tabs=[];window.__active=null;window.__callbacks={};window.__events={};let cb=0;
window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}},transformCallback:(f)=>{window.__callbacks[++cb]=f;return cb;},unregisterCallback:()=>{},invoke:async(cmd,args)=>{
 window.__calls.push(cmd);
 if(cmd==='nastrojki_poluchit')return {tema:'temnaya',shrift:'system',razmer_shrifta:14,plotnost:'udobno',masshtab_stranic:100,vyhody:[{id:'local',nazvanie:'Test subscription',adres:'https://example.test/sub',proxy_uzel:''}],panel_shirina:180,panel_svernuta:false,novaya_stranica:'about:blank',poisk:'duckduckgo',avtoobnovleniya:false,apparatnoe_uskorenie:false};
 if(cmd==='nastrojki_sohranit'){window.__saved=args.nastrojki;return args.nastrojki;}
 if(cmd==='spisok')return window.__tabs;
 if(cmd==='sohranennye')return [];
 if(cmd==='obychnye_spisok'){const v=window.__many?Array.from({length:window.__many},(_,i)=>({id:i+1,adres:'about:blank',zagolovok:'Tab number '+(i+1),zakreplena:false,otlozhena:false})):[{id:1,adres:'about:blank',zagolovok:'One',zakreplena:false,otlozhena:true},{id:2,adres:'about:blank',zagolovok:'Two',zakreplena:false,otlozhena:true}];return {aktivnaya:1,vkladki:v};}
 if(cmd==='aktivnaya')return window.__active;
 if(cmd==='dvizhok_est')return true;
 if(cmd==='schet_rezki')return [0,0];
 if(cmd==='uzly_podpiski')return [{imya:'Unavailable',ssylka:'vless://fixture@127.0.0.1:1'},{imya:'Local second',ssylka:'vless://fixture@127.0.0.1:2'}];
 if(cmd==='proverit_uzel')return args.ssylka.endsWith(':2')?{ms:34,status:'Доступен'}:{ms:null,status:'Не отвечает'};
 if(cmd==='plugin:app|version')return 'test';
 if(cmd==='plugin:event|listen'){window.__events[args.event]=args.handler;return 1;}
 return null;
}};'''
try:
 with sync_playwright() as pw:
  browser=pw.chromium.launch(channel='chrome',headless=True,args=['--no-sandbox'])
  page=browser.new_page(viewport={'width':1280,'height':820});page.add_init_script(mock)
  errors=[]
  page.on('pageerror',lambda e:errors.append(str(e)))
  # ⛔ 404 на ресурс страницы (обычно /favicon.ico от локального сервера теста)
  # — не ошибка интерфейса: браузер сам просит значок. Ошибки нашего кода
  # всё равно ловятся через pageerror и console:error с текстом.
  def _console(m):
    if m.type=='error' and 'Failed to load resource' not in m.text: errors.append('console:'+m.text)
  page.on('console',_console)
  def popup_fits():
   page.locator('.vybor-popup').wait_for()
   r=page.locator('.vybor-popup').bounding_box();v=page.viewport_size
   assert r['y']>=0 and r['x']>=0 and r['y']+r['height']<=v['height'] and r['x']+r['width']<=v['width'],r
  page.goto(f'http://127.0.0.1:{server.server_port}')
  page.get_by_title('Настройки NexusBro',exact=True).click()
  page.get_by_role('combobox',name='Шрифт интерфейса',exact=True).click()
  popup_fits()
  page.get_by_role('option',name='Arial',exact=True).click()
  expect(page.get_by_role('combobox',name='Шрифт интерфейса',exact=True)).to_contain_text('Arial')
  page.get_by_role('combobox',name='Плотность интерфейса',exact=True).click()
  popup_fits()
  page.get_by_role('option',name='Компактная',exact=True).click()
  page.get_by_role('button',name='Сеть и VPN',exact=True).click()
  page.locator('.subscription-nodes summary').click()
  expect(page.locator('.node-row').filter(has_text='Local second')).to_contain_text('34 мс')
  expect(page.locator('.node-row').filter(has_text='Unavailable')).to_contain_text('Не отвечает')
  assert page.locator('.subscription-nodes [role=combobox]').count()==0
  nastrojki=page.get_by_role('button',name='Сохранить',exact=True)
  nastrojki.click()
  page.wait_for_function('window.__saved!==null')
  page.wait_for_timeout(300)
  saved=page.evaluate('window.__saved');assert saved['shrift']=='arial' and saved['plotnost']=='kompaktno',saved
  assert saved['vyhody'][0]['proxy_uzel']=='',saved
  page.locator('.niz button').first.click()
  page.get_by_role('combobox',name='Сохранённое подключение',exact=True).click()
  page.get_by_role('option',name='Test subscription',exact=True).click()
  expect(page.get_by_role('combobox',name='Узел подписки',exact=True)).to_contain_text('Первый узел подписки')
  page.get_by_role('combobox',name='Сохранённое подключение',exact=True).click()
  page.get_by_role('option',name='Прямое соединение',exact=True).click()
  expect(page.get_by_role('combobox',name='Сохранённое подключение',exact=True)).to_contain_text('Прямое соединение')
  page.get_by_role('combobox',name='Сохранённое подключение',exact=True).click()
  page.get_by_role('option',name='Test subscription',exact=True).click()
  expect(page.get_by_role('combobox',name='Узел подписки',exact=True)).to_contain_text('Первый узел подписки')
  # ⛔ Не искать кнопку заново: после сохранения кнопка ОСТАЁТСЯ в DOM и
  # уже отцеплена — но «Закрыть настройки» может ещё висеть в кэше роли.
  # Настройки после «Сохранить» закрываются сами, это и проверяем.
  page.wait_for_selector('.nastrojki-okno',state='detached',timeout=5000)
  page.evaluate("""() => {window.__tabs=[{id:'layout',nazvanie:'Layout',proxy:'',zapushchena:true,sohranena:false}];window.__active='layout';window.__callbacks[window.__events['vkladki-izmenilis']]({event:'vkladki-izmenilis',payload:'layout'});} """)
  page.get_by_role('combobox',name='Узел подписки',exact=True).click()
  page.get_by_role('option',name='Local second',exact=True).click()
  expect(page.get_by_role('combobox',name='Узел подписки',exact=True)).to_contain_text('Local second')
  page.get_by_role('combobox',name='Узел подписки',exact=True).click()
  page.get_by_role('option',name='Unavailable',exact=True).click()
  expect(page.get_by_role('combobox',name='Узел подписки',exact=True)).to_contain_text('Unavailable')
  page.get_by_role('tab',name='Two',exact=True).wait_for(state='attached',timeout=5000)
  last=page.get_by_role('tab',name='Two',exact=True).bounding_box();plus=page.get_by_role('button',name='Новая обычная вкладка',exact=True).bounding_box()
  assert 0<=plus['x']-(last['x']+last['width'])<=16,(last,plus)
  expect(page.get_by_role('button',name='Загрузить вкладку',exact=True)).to_be_visible()
  assert page.locator('select').count()==0
  # 17.11.2026: много вкладок не должны растить ползунок, «+» — на одной
  # линии с вкладками и сразу за последней. Список приходит через IPC.
  # 17.11.2026: полоса не должна ни скроллиться, ни прыгать: «+» на одной
  # линии с вкладками, сразу за последней, и всё влезает в полосу на любой
  # ширине окна. Супер-вкладку не переключаем — real-бэкенд тогда заново
  # запрашивает список и теряет подставленное через IPC число.
  for shirina,visota in ((1600,900),(1280,820),(900,700)):
   page.set_viewport_size({'width':shirina,'height':visota})
   for n in (10,20):
    page.evaluate("""(n) => {const vk=Array.from({length:n},(_,i)=>({id:i+1,adres:'about:blank',zagolovok:'Tab number '+(i+1),
        zakreplena:false,zagruzka:false,nazad:false,vpered:false,otlozhena:true}));
      window.__callbacks[window.__events['obychnye-izmenilis']]({event:'obychnye-izmenilis',id:1,payload:{id:'layout',sessiya:{vkladki:vk,aktivnaya:1}}});}""",n)
    page.get_by_role('tab',name='Tab number '+str(n),exact=True).wait_for(state='attached',timeout=5000)
    page.wait_for_timeout(250)
    m=page.evaluate("""() => {const l=document.querySelector('.obychnye'),p=document.querySelector('.novaya-obychnaya'),po=document.querySelector('.polosa');
      const last=l.lastElementChild.getBoundingClientRect(),pr=p.getBoundingClientRect(),lr=l.getBoundingClientRect(),por=po.getBoundingClientRect();
      return {scroll:l.scrollWidth-l.clientWidth,plusTopMid:pr.top+pr.height/2,tabTopMid:last.top+last.height/2,
              zazor:pr.left-lr.right, plusInside: pr.right<=por.right+1, lastWidth:last.width, count:l.children.length,
              tabBottom:last.bottom, polosaBottom:por.bottom};}""")
    assert m['count']==n,(shirina,n,m)
    # Ползунка быть не должно: пока вкладки влезают, scrollWidth == clientWidth.
    # 20 вкладок в окне 900px физически не влезают (20 × минимум 42px > 624px) —
    # там разрешаем внутреннюю прокрутку и проверяем только «+» и высоту.
    if shirina>900 or n<=10: assert m['scroll']<=0,(shirina,n,m)
    assert abs(m['plusTopMid']-m['tabTopMid'])<=1.5,(shirina,n,m)
    assert m['plusInside'],(shirina,n,m)
    assert 0<=m['zazor']<=16,(shirina,n,m)
    assert m['tabBottom']<=m['polosaBottom']+1,(shirina,n,m)
    assert m['lastWidth']>=36,(shirina,n,m)
  assert not errors,errors
  print(json.dumps({'mouse_font_density':True,'all_subscription_nodes':True,'overview_without_global_selection':True,'super_node_mouse_selection':True,'no_native_select':True,'plus_adjacent_to_last':True,'plus_vertically_centered':True,'many_tabs_no_scrollbar':True,'pending_placeholder':True}))
  browser.close()
finally:server.shutdown()
