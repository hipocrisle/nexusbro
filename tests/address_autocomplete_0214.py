"""Chromium UI regression: address datalist, stale IPC and auto-translation controls.

Tauri IPC uses a fixture. This does not prove popup placement above a native CEF HWND,
actual history ranking/IO, translation throughput or automatic CEF navigation behavior.
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
window.__suggestQueries=[];window.__suggestPending=null;window.__translationHandlers=[];
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd==='plugin:event|listen'&&args.event==='stranica-ui')window.__translationHandlers.push(args.handler);
 if(cmd==='biblioteka_podskazki'){
  window.__suggestQueries.push(args);
  if(args.query==='slow.')return new Promise(resolve=>window.__suggestPending=resolve);
  return [{adres:'https://de.wikipedia.org/wiki/Berlin',nazvanie:'Берлин — сохранённая страница',istochnik:'istoriya',vremya:1790000000}];
 }
 return originalInvoke(cmd,args);
};window.__tabs=[{id:'layout',nazvanie:'Address fixture',proxy:'',adres:'about:blank',rezat_reklamu:true,pustaya:false}];window.__active='layout';
window.__emitTranslation=data=>window.__translationHandlers.forEach(handler=>window.__callbacks[handler]?.({payload:{super_id:'layout',data:{tip:'perevod',id:1,...data}}}));
'''


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=str(root / "desktop/dist")))
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
        address = page.get_by_label("Адрес страницы", exact=True)
        address.fill("")
        page.evaluate("window.__suggestQueries=[]")
        address.press_sequentially("de.wiki", delay=5)
        page.wait_for_function("window.__suggestQueries.length===1")
        assert page.evaluate("window.__suggestQueries")==[{"query": "de.wiki", "limit": 8}]
        options = page.locator("#address-history option")
        expect(options).to_have_count(1)
        expect(options.first).to_have_attribute("value", "https://de.wikipedia.org/wiki/Berlin")
        expect(options.first).to_have_attribute("label", "Берлин — сохранённая страница")
        expect(address).to_have_attribute("list", "address-history")

        address.fill("slow.")
        page.wait_for_function("window.__suggestPending!==null")
        address.fill("de.wiki")
        page.wait_for_function("window.__suggestQueries.filter(x=>x.query==='de.wiki').length===2")
        expect(options).to_have_count(1)
        page.evaluate("window.__suggestPending([{adres:'https://stale.example.test/',nazvanie:'STALE',istochnik:'istoriya'}])")
        page.wait_for_timeout(100)
        expect(options).to_have_count(1)
        expect(options.first).to_have_attribute("value", "https://de.wikipedia.org/wiki/Berlin")

        # Browser-native datalist selection can be unavailable in headless mode.
        # Attempt real keys; never substitute a JS assignment for a successful selection.
        before = page.evaluate("window.__args.length")
        address.press("ArrowDown")
        address.press("Enter")
        address.press("Enter")
        page.wait_for_timeout(100)
        navigations = page.evaluate("n=>window.__args.slice(n).filter(x=>x.cmd==='obychnaya_komanda'&&x.args.komanda.deystvie==='pereyti').map(x=>x.args.komanda.adres)", before)
        keyboard_selection = "https://de.wikipedia.org/wiki/Berlin" in navigations

        arrow = page.get_by_role("button", name="Варианты новой вкладки", exact=True)
        expect(arrow.locator("svg")).to_have_count(1)
        assert arrow.locator("svg path").get_attribute("d") == "m7 10 5 5 5-5"
        assert "⌄" not in arrow.inner_text()
        arrow.click()
        page.get_by_role("menuitem", name="Новая приватная вкладка", exact=True).click()
        assert page.evaluate("window.__args.filter(x=>x.cmd==='obychnaya_komanda').at(-1).args.komanda.deystvie") == "novaya_privatnaya"

        page.evaluate("window.__emitTranslation({auto:false,done:2,total:2})")
        auto = page.get_by_role("checkbox", name="Автоперевод этого сайта", exact=True)
        expect(auto).not_to_be_checked()
        auto.click()
        assert page.evaluate("window.__args.some(x=>x.cmd==='obychnaya_komanda'&&x.args.id==='layout'&&x.args.komanda.deystvie==='perevod_avto'&&x.args.komanda.id===1&&x.args.komanda.enabled===true)")
        # Checked state follows the CEF acknowledgement, rather than just the click.
        page.evaluate("window.__emitTranslation({auto:true,done:0,total:3})")
        expect(auto).to_be_checked()
        auto.click()
        assert page.evaluate("window.__args.some(x=>x.cmd==='obychnaya_komanda'&&x.args.komanda.deystvie==='perevod_avto'&&x.args.komanda.enabled===false)")
        page.evaluate("window.__emitTranslation({auto:false,done:3,total:3})")
        expect(auto).not_to_be_checked()
        page.get_by_role("button", name="Показать оригинал и выключить", exact=True).click()
        assert page.evaluate("window.__args.filter(x=>x.cmd==='obychnaya_komanda').at(-1).args.komanda.deystvie") == "perevod_original"
        page.evaluate("window.__emitTranslation({original:true,auto:false})")
        expect(auto).not_to_be_visible()
        assert not errors, errors
        print(json.dumps({"debounced_history_ipc": True, "datalist_values_and_labels": True,
                          "stale_response_discarded": True, "native_datalist_keyboard_selection": keyboard_selection,
                          "arrow_svg_and_private_menu": True, "auto_translation_commands_and_ack": True,
                          "restore_command_and_hide_ack": True,
                          "native_cef_hwnd_popup_placement": "not_tested", "native_cef_auto_navigation": "not_tested"}))
        browser.close()
finally:
    server.shutdown()
