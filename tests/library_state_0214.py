"""Real Chromium regression for library response races and clear-data IPC acknowledgement.

The browser/UI is real; native Tauri IPC is a deterministic delayed fixture.
This test does not claim native CEF cookie/cache deletion.
"""
import functools
import http.server
import json
import os
import pathlib
import threading

from playwright.sync_api import expect, sync_playwright

root = pathlib.Path(__file__).resolve().parents[1]
base = (root / "tests/frontend_controls.py").read_text().split("mock=r'''", 1)[1].split("'''", 1)[0]
mock = base + r'''
const originalInvoke=window.__TAURI_INTERNALS__.invoke;
window.__bookPending=null;window.__deferBook=false;window.__sitePending=null;window.__deferHide=false;window.__hidePending=null;
window.__history=[{id:'h-1',adres:'https://history.example.test/',nazvanie:'History only',vremya:1790000000,papka:''}];
window.__books=[{id:'b-1',adres:'https://bookmark.example.test/',nazvanie:'Bookmark only',vremya:1790000000,papka:'Saved'}];
window.__downloads=[{id:'d-1',super_id:'layout',name:'download-only.zip',url:'https://downloads.example.test/file?secret=never-render',path:'fixture.zip',percent:100,received:5,total:5,speed:0,status:'Готово',privatnaya:false}];
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 window.__args.push({cmd,args});
 if(cmd==='pokazat_dvizhki'&&args.vidno===false&&window.__deferHide){window.__deferHide=false;return new Promise(resolve=>window.__hidePending=resolve);}
 if(cmd==='biblioteka_spisok'){
  if(args.razdel==='zakladki'&&window.__deferBook){window.__deferBook=false;return new Promise(resolve=>window.__bookPending=()=>resolve(window.__books));}
  return args.razdel==='zakladki'?window.__books:window.__history;
 }
 if(cmd==='biblioteka_udalit'){if(args.razdel==='zakladki')window.__books=[];else window.__history=[];return null;}
 if(cmd==='zagruzki_spisok')return window.__downloads;
 if(cmd==='zagruzki_ochistit'){window.__downloads=[];return 1;}
 if(cmd==='dannye_obem')return {istoriya:2,istoriya_vsego:12,zagruzki:3};
 if(cmd==='dannye_saytov_statistika')return {cookies:7,cache_bytes:1572864,cache_memory:false};
 if(cmd==='dannye_ochistit')return {istoriya:2,zagruzki:1};
 if(cmd==='ochistit_dannye_saytov')return new Promise(resolve=>window.__sitePending=resolve);
 return originalInvoke(cmd,args);
};window.__tabs=[{id:'layout',nazvanie:'Library fixture',proxy:'',adres:'about:blank',rezat_reklamu:true,pustaya:false}];window.__active='layout';
'''


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


dist = pathlib.Path(os.environ.get("NEXUSBRO_FRONTEND_DIST", str(root / "desktop/dist")))
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=str(dist)))
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with sync_playwright() as p:
        browser = p.chromium.launch(channel=os.environ.get("NEXUSBRO_PLAYWRIGHT_CHANNEL", "chrome") or None,
                                    headless=True, args=["--no-sandbox"])
        page = browser.new_page(viewport={"width": 1280, "height": 850})
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.add_init_script(mock)
        page.goto(f"http://127.0.0.1:{server.server_port}")
        page.get_by_role("tab", name="One", exact=True).wait_for()
        page.evaluate("window.__deferHide=true;window.dispatchEvent(new CustomEvent('nexusbro-library',{detail:'zakladki'}))")
        page.wait_for_function("window.__hidePending!==null")
        page.evaluate("window.dispatchEvent(new CustomEvent('nexusbro-library',{detail:'istoriya'}))")
        first_history = page.get_by_role("dialog", name="История", exact=True)
        page.evaluate("window.__history[0].adres='https://history.example.test/'+('long-address'*200)")
        expect(first_history.get_by_text("History only", exact=True)).to_be_visible()
        assert first_history.locator('.library-list').evaluate('(e)=>e.scrollWidth<=e.clientWidth+1')
        assert first_history.evaluate('(e)=>e.scrollWidth<=e.clientWidth+1')
        page.evaluate("window.__hidePending(null)")
        page.wait_for_timeout(100)
        expect(page.get_by_role("dialog", name="Закладки", exact=True)).not_to_be_visible()
        first_history.get_by_role("button", name="Закрыть библиотеку", exact=True).click()
        page.evaluate("window.__deferBook=true;window.dispatchEvent(new CustomEvent('nexusbro-library',{detail:'zakladki'}))")
        books = page.get_by_role("dialog", name="Закладки", exact=True)
        books.wait_for()
        page.wait_for_function("window.__bookPending!==null")
        books.get_by_role("button", name="История", exact=True).click()
        history = page.get_by_role("dialog", name="История", exact=True)
        expect(history.get_by_text("History only", exact=True)).to_be_visible()
        page.evaluate("window.__bookPending()")
        page.wait_for_timeout(100)
        expect(history.get_by_text("Bookmark only", exact=True)).not_to_be_visible()
        expect(history.get_by_text("History only", exact=True)).to_be_visible()
        history.get_by_role("button", name="Загрузки", exact=True).click()
        downloads = page.get_by_role("dialog", name="Загрузки", exact=True)
        expect(downloads.get_by_text("download-only.zip", exact=True)).to_be_visible()
        assert "never-render" not in downloads.inner_text()
        expect(downloads.get_by_text("Bookmark only", exact=True)).not_to_be_visible()
        downloads.get_by_role("button", name="Убрать из списка", exact=True).click()
        expect(downloads.get_by_text("download-only.zip", exact=True)).not_to_be_visible()
        assert page.evaluate("window.__args.some(x=>x.cmd==='zagruzki_ochistit'&&x.args.id==='d-1')")
        downloads.get_by_role("button", name="Закладки", exact=True).click()
        expect(books.get_by_text("Bookmark only", exact=True)).to_be_visible()
        books.get_by_role("button", name="Удалить запись: Bookmark only", exact=True).click()
        expect(books.get_by_text("Bookmark only", exact=True)).not_to_be_visible()
        assert page.evaluate("window.__args.some(x=>x.cmd==='biblioteka_udalit'&&x.args.razdel==='zakladki'&&x.args.id==='b-1')")
        books.get_by_role("button", name="Закрыть библиотеку", exact=True).click()

        page.keyboard.press("Control+Shift+Delete")
        clear = page.get_by_role("dialog", name="Удаление данных супер-вкладки", exact=True)
        clear.wait_for()
        clear.get_by_role("combobox", name="Период очистки истории", exact=True).click()
        page.get_by_role("option",name="За всё время",exact=True).click()
        expect(clear.get_by_role("combobox",name="Период очистки истории")).to_contain_text("За всё время")
        expect(clear.get_by_test_id("history-size")).to_contain_text("12 за всё время")
        expect(clear.get_by_test_id("downloads-size")).to_contain_text("3 записей")
        expect(clear.get_by_test_id("cookies-size")).to_contain_text("7 cookies")
        expect(clear.get_by_test_id("cache-size")).to_contain_text("1,5 МБ")
        clear.get_by_role("combobox",name="Период очистки истории").click()
        page.get_by_role("option",name="Последние 7 дней",exact=True).click()
        clear.get_by_role("checkbox", name="Список завершённых загрузок текущей супер-вкладки — за всё время", exact=True).check()
        clear.get_by_role("checkbox", name="Cookies текущей супер-вкладки — за всё время", exact=True).check()
        clear.get_by_role("button", name="Удалить данные", exact=True).click()
        page.wait_for_function("window.__sitePending!==null")
        expect(clear.get_by_role("button", name="Удаление…", exact=True)).to_be_disabled()
        assert "Очистка завершена" not in clear.inner_text()
        page.evaluate("window.__sitePending({cookies:false,cache:false})")
        expect(clear.get_by_role("alert")).to_contain_text("не подтвердил")
        page.evaluate("window.__sitePending=null")
        clear.get_by_role("button", name="Удалить данные", exact=True).click()
        page.wait_for_function("window.__sitePending!==null")
        page.evaluate("window.__sitePending({cookies:true,cache:false})")
        expect(clear.get_by_role("status")).to_contain_text("Очистка завершена")
        assert page.evaluate("window.__args.some(x=>x.cmd==='dannye_ochistit'&&x.args.period==='week'&&x.args.istoriya&&x.args.zagruzki)")
        assert page.evaluate("window.__args.some(x=>x.cmd==='ochistit_dannye_saytov'&&x.args.id==='layout'&&x.args.cookies&&!x.args.cache)")
        clear.get_by_role("button", name="Готово", exact=True).click()
        expect(clear).not_to_be_visible()
        assert not errors, errors
        print(json.dumps({"stale_library_response_rejected": True, "stale_open_rejected": True, "section_dom_isolated": True,
                          "bookmark_and_download_delete": True, "ctrl_shift_delete": True,
                          "period_and_scope": True, "native_clear_ack_required": True}))
        browser.close()
finally:
    server.shutdown()
