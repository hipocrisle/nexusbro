"""Цвет профиля и явная приватность в свёрнутой панели: реальный Chromium, IPC fixture."""
import ast, functools, http.server, os, pathlib, threading
from playwright.sync_api import sync_playwright, expect

root = pathlib.Path(__file__).resolve().parents[1]
module = ast.parse((root / 'tests/frontend_controls.py').read_text())
mock = next(ast.literal_eval(n.value) for n in module.body if isinstance(n, ast.Assign)
            and any(isinstance(t, ast.Name) and t.id == 'mock' for t in n.targets))
mock += '''
const originalInvoke = window.__TAURI_INTERNALS__.invoke;
window.__tabs=[
 {id:'normal',nazvanie:'Работа',proxy:'',adres:'about:blank',cvet:'#5b9ee8',privatnaya:false,zapushchena:true},
 {id:'pvt-private',nazvanie:'Работа приватно',proxy:'',adres:'about:blank',cvet:'#b48ad8',privatnaya:true,zapushchena:false},
 {id:'invalid',nazvanie:'Старый профиль',proxy:'',adres:'about:blank',cvet:'url(https://invalid.example)',privatnaya:false,zapushchena:false}
]; window.__active='normal';
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='izmenit'||cmd==='otkryt'){
  window.__args.push({cmd,args});window.__lastProfile=structuredClone(args.nastrojki);
  const i=window.__tabs.findIndex(v=>v.id===args.nastrojki.id);
  if(i>=0)window.__tabs[i]={...window.__tabs[i],...args.nastrojki};
  else window.__tabs.push({...args.nastrojki,zapushchena:false});
  return {ok:true,soobshchenie:''};
 }
 return originalInvoke(cmd,args);
};
'''

class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a): pass

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Quiet, directory=str(root / 'desktop/dist')))
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with sync_playwright() as p:
        browser = p.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL', 'chrome') or None,
                                    headless=True, args=['--no-sandbox'])
        page = browser.new_page(viewport={'width':1280,'height':900})
        errors=[]
        page.on('pageerror', lambda e: errors.append(str(e)))
        page.add_init_script(mock)
        page.goto(f'http://127.0.0.1:{server.server_port}')
        normal = page.get_by_role('button', name='Работа', exact=True)
        private = page.get_by_role('button', name='Работа приватно — приватная супер-вкладка', exact=True)
        expect(normal).to_be_visible()
        expect(private.locator('.super-private-badge')).to_be_visible()
        active_shadow=normal.evaluate("e=>getComputedStyle(e).boxShadow")
        initial_fill=normal.evaluate("e=>getComputedStyle(e).backgroundColor")
        private_fill=private.evaluate("e=>getComputedStyle(e).backgroundColor")
        assert initial_fill!=private_fill and initial_fill!='rgba(0, 0, 0, 0)'
        assert page.get_by_role('button', name='Старый профиль', exact=True).get_attribute('style') is None
        page.get_by_role('button', name='Свернуть левую панель', exact=True).click()
        expect(normal.locator('.imya')).not_to_be_visible()
        expect(normal.locator('.kratkoe-imya')).to_be_visible()
        expect(private.locator('.super-private-badge')).to_be_visible()
        assert normal.locator('.super-private-badge').count()==0
        assert normal.evaluate('e=>getComputedStyle(e).backgroundColor')==initial_fill
        assert private.evaluate('e=>getComputedStyle(e).backgroundColor')==private_fill
        nb, pb = normal.bounding_box(), private.bounding_box()
        badge = private.locator('.super-private-badge').bounding_box()
        assert pb['x']<=badge['x'] and badge['x']+badge['width']<=pb['x']+pb['width']
        assert pb['y']<=badge['y'] and badge['y']+badge['height']<=pb['y']+pb['height']
        assert pb['y']>=nb['y']+nb['height']
        page.screenshot(path='/tmp/nexusbro-super-colors-collapsed.png')
        page.get_by_role('button', name='Развернуть левую панель', exact=True).click()
        normal.click(button='right')
        page.locator('.stroka-menyu').filter(has_text='Настройки').click()
        expect(page.get_by_role('radio', name='Синий', exact=True)).to_be_checked()
        page.get_by_role('radio', name='Зелёный', exact=True).check()
        page.get_by_role('button', name='Сохранить', exact=True).click()
        expect(page.locator('.super-color-picker')).not_to_be_visible()
        assert page.evaluate('window.__lastProfile.cvet') == '#72b566'
        assert normal.evaluate('e=>getComputedStyle(e).backgroundColor')!=initial_fill
        assert normal.evaluate('e=>getComputedStyle(e).boxShadow')==active_shadow
        assert not page.evaluate("window.__args.some(x=>['zapustit','ostanovit'].includes(x.cmd))")
        assert page.evaluate('window.__lastProfile.privatnaya') is False
        normal.click(button='right')
        page.locator('.stroka-menyu').filter(has_text='Настройки').click()
        expect(page.get_by_role('radio', name='Зелёный', exact=True)).to_be_checked()
        page.get_by_role('radio', name='Без цвета', exact=True).check()
        page.get_by_role('button', name='Сохранить', exact=True).click()
        expect(normal).not_to_have_attribute('style', '--super-color: #72b566;')
        assert 'super-colored' not in normal.get_attribute('class').split()
        assert page.evaluate('window.__lastProfile.cvet') == ''
        # Навигация и сохранение цвета не меняют приватный признак соседней Super.
        private.click(button='right')
        page.locator('.stroka-menyu').filter(has_text='Настройки').click()
        expect(page.get_by_role('radio', name='Фиолетовый', exact=True)).to_be_checked()
        expect(page.get_by_role('checkbox', name='Приватная супер-вкладка', exact=False)).to_be_checked()
        page.get_by_role('radio', name='Оранжевый', exact=True).check()
        page.get_by_role('button', name='Сохранить', exact=True).click()
        assert page.evaluate('window.__lastProfile.privatnaya') is True
        assert page.evaluate('window.__lastProfile.cvet') == '#dc9365'
        assert not errors, errors
        print('PASS: collapsed color/private identity, swatch edit/clear/reopen, invalid CSS rejection; IPC is a fixture, native persistence requires CI')
        browser.close()
finally:
    server.shutdown()
