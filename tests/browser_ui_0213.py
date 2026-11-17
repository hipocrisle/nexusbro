"""Пользовательские регрессии 0.2.12: меню, RU-сочетания, звезда, адрес и загрузки."""
import ast,functools,http.server,json,os,pathlib,threading
from playwright.sync_api import sync_playwright,expect
root=pathlib.Path(__file__).resolve().parents[1]
module=ast.parse((root/'tests/frontend_controls.py').read_text())
mock=next(ast.literal_eval(n.value) for n in module.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='mock' for t in n.targets))
mock+='''
const originalInvoke=window.__TAURI_INTERNALS__.invoke;window.__books=[];
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='bufer_chitat')return navigator.clipboard.readText();
 if(cmd==='bufer_zapisat')return navigator.clipboard.writeText(args.text);
 if(cmd==='biblioteka_spisok'&&args.razdel==='zakladki')return window.__books;
 if(cmd==='zakladki_pereklyuchit'){let exists=window.__books.some(x=>x.adres===args.adres);window.__books=exists?[]:[{id:'bookmark',adres:args.adres,nazvanie:args.nazvanie}];return !exists;}
 if(cmd==='set_status')return {mode:'Прокси',node:'Fixture',reachable:true,running:true};
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
  address=page.get_by_role('textbox',name='Адрес страницы',exact=True);address.fill('https://example.test/enter');address.press('Enter');expect(page.get_by_role('dialog',name='Сетевой выход',exact=True)).not_to_be_visible();assert page.evaluate("window.__args.some(x=>x.cmd==='obychnaya_komanda'&&x.args.komanda.deystvie==='pereyti')")
  assert page.locator('.library-shortcuts').count()==0
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
  address=page.get_by_role('textbox',name='Адрес страницы',exact=True);address.focus();address.evaluate('(e)=>e.setSelectionRange(21,25)');selected=address.evaluate('(e)=>e.value.slice(e.selectionStart,e.selectionEnd)');position=address.evaluate("e=>{const c=document.createElement('canvas').getContext('2d');c.font=getComputedStyle(e).font;const r=e.getBoundingClientRect();return {x:r.x+parseFloat(getComputedStyle(e).paddingLeft)+c.measureText(e.value.slice(0,23)).width,y:r.y+r.height/2};}");page.mouse.click(position['x'],position['y'],button='right')
  print('stage: address',flush=True)
  menu=page.get_by_role('menu',name='Адресная строка',exact=True);expect(menu).to_be_visible();assert not any(s in menu.inner_text().lower() for s in ['эмодзи','парол','направлен','инструмент'])
  menu.get_by_text('Копировать',exact=True).click();assert page.evaluate("Promise.race([navigator.clipboard.readText(),new Promise(r=>setTimeout(()=>r('TIMEOUT'),2500))])")==selected
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
