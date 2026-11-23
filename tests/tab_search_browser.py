"""Real Chromium UI; native snapshot/activation IPC is explicitly a fixture."""
import ast,functools,http.server,json,os,pathlib,threading
from playwright.sync_api import sync_playwright,expect
root=pathlib.Path(__file__).resolve().parents[1]
module=ast.parse((root/'tests/frontend_controls.py').read_text())
base=next(ast.literal_eval(n.value) for n in module.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='mock' for t in n.targets))
mock=base+r'''
const oldInvoke=window.__TAURI_INTERNALS__.invoke;
window.__searchRows=Array.from({length:1000},(_,i)=>({super_id:'hosters',super_name:'Хостеры',window:'main',tab_id:i+1,title:i===777?'Особенный хостер':'Страница '+i,address:'https://host'+i+'.invalid/',sleeping:i>5,index:i,route:{kind:'inherit'}}));
window.__searchRows.push({super_id:'work',super_name:'Работа',window:'browser-42',tab_id:42,title:'Почта',address:'https://mail.invalid/',sleeping:false,index:0,route:{kind:'direct'}});
window.__activated=[];window.__activateFails=false;
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='tab_search_snapshot')return structuredClone(window.__searchRows);
 if(cmd==='tab_search_activate'){if(window.__activateFails)throw 'Список вкладок изменился. Повторите поиск.';window.__activated.push(args.entry);return;}
 return oldInvoke(cmd,args);
};
'''
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(root/'desktop/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
with sync_playwright() as pw:
 browser=pw.chromium.launch(channel=os.getenv('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None,headless=True)
 page=browser.new_page();page.add_init_script(mock);page.goto(f'http://127.0.0.1:{server.server_port}');page.wait_for_function('window.__calls.includes("spisok")')
 page.keyboard.press('Control+Shift+A');dialog=page.get_by_role('dialog',name='Поиск по вкладкам');expect(dialog).to_be_visible()
 field=dialog.get_by_role('textbox',name='Найти вкладку или супервкладку');expect(field).to_be_focused()
 expect(dialog.get_by_role('option')).to_have_count(150)
 assert not page.evaluate('window.__calls.includes("zapustit")')
 field.fill('хостеры особенный host777');expect(dialog.get_by_role('option')).to_have_count(1);field.press('Enter');expect(dialog).not_to_be_visible()
 assert page.evaluate('window.__activated[0].tab_id')==778
 page.keyboard.press('Control+Shift+A');field.fill('почта');field.press('Enter');expect(dialog).not_to_be_visible();assert page.evaluate('window.__activated[1].window')=='browser-42'
 page.keyboard.press('Control+Shift+A');field.fill('неттакогосайта');expect(dialog.get_by_text('Ничего не найдено')).to_be_visible()
 field.fill('Почта');page.evaluate('window.__activateFails=true');field.press('Enter');expect(dialog.get_by_role('alert')).to_contain_text('изменился');expect(dialog).to_be_visible();field.press('Escape');expect(dialog).not_to_be_visible()
 assert len(page.evaluate('window.__activated'))==2
 assert page.evaluate('window.__args.filter(x=>x.cmd==="pokazat_dvizhki").at(-1).args.vidno') is True
 browser.close()
server.shutdown()
print(json.dumps({'real_chromium':True,'native_ipc':'fixture','thousand_rows_bounded_render':True,'multiword_name_super_url_search':True,'exact_tab_window_activation':True,'no_engine_launch_while_searching':True,'stale_activation_visible_error':True,'keyboard_escape_restore':True}))
