"""Пользовательские регрессии 0.2.12: меню, RU-сочетания, звезда, адрес и загрузки."""
import ast,functools,http.server,json,os,pathlib,threading
from playwright.sync_api import sync_playwright,expect
root=pathlib.Path(__file__).resolve().parents[1]
module=ast.parse((root/'tests/frontend_controls.py').read_text())
mock=next(ast.literal_eval(n.value) for n in module.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='mock' for t in n.targets))
mock+='''
const originalInvoke=window.__TAURI_INTERNALS__.invoke;window.__books=[];
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='plugin:menu|new'){window.__nativeMenu=args.options.items;return [111,'address'];}
 if(cmd==='plugin:menu|popup'){window.__menuPopped=true;return null;}
 if(cmd==='bufer_chitat')return navigator.clipboard.readText();
 if(cmd==='bufer_zapisat')return navigator.clipboard.writeText(args.text);
 if(cmd==='biblioteka_spisok'&&args.razdel==='zakladki')return window.__books;
 if(cmd==='zakladki_pereklyuchit'){let exists=window.__books.some(x=>x.adres===args.adres);window.__books=exists?[]:[{id:'bookmark',adres:args.adres,nazvanie:args.nazvanie}];return !exists;}
 if(cmd==='set_status')return {mode:'Прокси',node:'Fixture',reachable:window.__health??true,running:true};
 if(cmd==='import_istochniki')return [{id:'chrome',browser:'Chrome',profile:'Default'},{id:'edge',browser:'Edge',profile:'Profile 1'}];
 if(cmd==='import_nachat')return 9;
 if(cmd==='import_status'){if(args.id===0)throw 'No tasks';return {id:9,status:'Готово',read:4,imported:3,skipped:1,error:null};}
 if(cmd==='zagruzki_spisok')return [{id:'completed',super_id:'layout',name:'invoice.pdf',url:'https://mail.example.test/attachment?secret=THIS_TOKEN_MUST_NOT_RENDER',path:'fixture.pdf',percent:100,received:5120,total:5120,speed:0,status:'Готово',privatnaya:false}];
 return originalInvoke(cmd,args);
};window.__tabs=[{id:'layout',nazvanie:'Layout fixture',proxy:'',adres:'about:blank',rezat_reklamu:true,pustaya:false}];window.__active='layout';
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*a):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
try:
 with sync_playwright() as p:
  browser=p.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None,headless=True,args=['--no-sandbox'])
  context=browser.new_context(permissions=['clipboard-read','clipboard-write'],viewport={'width':1280,'height':820})
  page=context.new_page();errors=[];page.on('pageerror',lambda e:errors.append(str(e)));page.add_init_script(mock);page.goto(f'http://127.0.0.1:{server.server_port}')
  page.get_by_role('tab',name='One',exact=True).wait_for()
  address=page.get_by_label('Адрес страницы',exact=True);address.fill('https://example.test/enter');address.press('Enter');expect(page.get_by_role('dialog',name='Сетевой выход',exact=True)).not_to_be_visible();assert page.evaluate("window.__args.some(x=>x.cmd==='obychnaya_komanda'&&x.args.komanda.deystvie==='pereyti')")
  assert page.locator('.library-shortcuts').count()==0
  network=page.get_by_role('button',name='Сетевой выход супер-вкладки',exact=True);expect(network).to_have_class('network-button network-up')
  buttons=[network,page.get_by_role('button',name='Защита супер-вкладки',exact=True),page.get_by_role('button',name='Меню NexusBro',exact=True)]
  boxes=[b.bounding_box() for b in buttons];assert all(b['width']==30 and b['height']==30 for b in boxes),boxes;assert boxes[0]['x']<boxes[1]['x']<boxes[2]['x'],boxes
  assert network.inner_text()==''
  page.evaluate('window.__health=false');expect(network).to_have_class('network-button network-down');page.evaluate('window.__health=true')

  page.evaluate("""()=>window.__callbacks[window.__events['obychnye-izmenilis']]({payload:{id:'layout',sessiya:{aktivnaya:1,vkladki:[{id:1,adres:'https://example.test/word/next',zagolovok:'Fixture',privatnaya:true}]}}})""")
  page.get_by_role('tab',name='Fixture',exact=True).wait_for()
  page.get_by_role('button',name='Новая обычная вкладка',exact=True).click()
  assert page.evaluate("window.__args.filter(x=>x.cmd==='obychnaya_komanda').at(-1).args.komanda.deystvie")=='novaya'
  page.get_by_role('button',name='Варианты новой вкладки',exact=True).click();page.get_by_role('menuitem',name='Новая приватная вкладка',exact=True).click()
  assert page.evaluate("window.__args.filter(x=>x.cmd==='obychnaya_komanda').at(-1).args.komanda.deystvie")=='novaya_privatnaya'
  print('stage: dropdown',flush=True)
  star=page.locator('.bookmark-star');star.click();expect(star).to_have_attribute('aria-pressed','true');expect(star).to_have_text('★');star.click();expect(star).to_have_attribute('aria-pressed','false')
  print('stage: bookmark',flush=True)
  page.evaluate("document.dispatchEvent(new KeyboardEvent('keydown',{key:'а',code:'KeyF',ctrlKey:true,bubbles:true}))")
  expect(page.get_by_role('textbox',name='Найти на странице',exact=True)).to_be_visible();page.keyboard.press('Escape')
  print('stage: russian',flush=True)
  page.evaluate("document.dispatchEvent(new KeyboardEvent('keydown',{key:'е',code:'Unidentified',ctrlKey:true,bubbles:true}))")
  assert page.evaluate("window.__args.filter(x=>x.cmd==='obychnaya_komanda').at(-1).args.komanda.deystvie")=='novaya'
  hides_before=page.evaluate("window.__args.filter(x=>x.cmd==='pokazat_dvizhki'&&x.args.vidno===false).length")
  address=page.get_by_label('Адрес страницы',exact=True);address.focus();address.press('Control+a');selected=address.input_value();address.click(button='right')
  print('stage: address',flush=True)
  page.wait_for_function('window.__menuPopped===true')
  assert page.evaluate("window.__args.filter(x=>x.cmd==='pokazat_dvizhki'&&x.args.vidno===false).length")==hides_before
  items=page.evaluate('window.__nativeMenu.map(i=>({id:i.id.split("-").at(-1),text:i.text,enabled:i.enabled}))');assert [i['id'] for i in items]==['undo','redo','cut','copy','paste','delete','selectAll'],items
  assert not any(s in str(items).lower() for s in ['эмодзи','парол','направлен','инструмент'])
  assert next(i for i in items if i['id']=='copy')['enabled']
  page.evaluate("window.__nativeMenu.find(i=>i.id.endsWith('-copy')).handler.onmessage('copy')");page.wait_for_timeout(100)
  assert page.evaluate("Promise.race([navigator.clipboard.readText(),new Promise(r=>setTimeout(()=>r('TIMEOUT'),2500))])")==selected
  page.get_by_role('button',name='Меню NexusBro',exact=True).click();page.get_by_role('menuitem',name='Загрузки Ctrl+J',exact=True).click()
  expect(page.get_by_text('invoice.pdf',exact=True)).to_be_visible();assert 'THIS_TOKEN_MUST_NOT_RENDER' not in page.locator('.library-list').inner_text()
  page.get_by_role('button',name='Убрать из списка',exact=True).click();assert page.evaluate("window.__args.some(x=>x.cmd==='zagruzki_ochistit'&&x.args.id==='completed')")
  page.get_by_role('button',name='Очистить список',exact=True).click();assert page.evaluate("window.__args.some(x=>x.cmd==='zagruzki_ochistit'&&x.args.id===null)")
  page.get_by_role('button',name='Закладки',exact=True).click();page.get_by_role('button',name='Перенести закладки из другого браузера…',exact=True).click();page.get_by_role('radio',name='Edge Profile 1',exact=True).click();page.get_by_role('button',name='Начать перенос закладок',exact=True).click();expect(page.get_by_text('Готово · Прочитано: 4 · Добавлено: 3 · Пропущено повторов и неподдерживаемых адресов: 1',exact=True)).to_be_visible()
  page.get_by_role('button',name='Закрыть библиотеку',exact=True).click()
  page.keyboard.press('Control+Shift+b');expect(page.get_by_role('navigation',name='Панель закладок',exact=True)).to_be_visible()
  assert page.evaluate('window.__saved.panel_zakladok') is True
  page.keyboard.press('Control+Shift+b');expect(page.get_by_role('navigation',name='Панель закладок',exact=True)).not_to_be_visible()
  assert not errors,errors
  print(json.dumps({'ordinary_plus_from_private':True,'private_dropdown':True,'bookmark_toggle':True,'russian_shortcut':True,'address_menu_and_copy':True,'compact_download_no_token':True,'download_clear':True,'browser_import_wizard':True}))
  browser.close()
finally:server.shutdown()
