"""Реальный Chromium: исходный helper выделения, без сборки приложения."""
import json,pathlib,subprocess
from playwright.sync_api import sync_playwright
root=pathlib.Path(__file__).resolve().parents[1]
# Только преобразование небольшого проверяемого модуля для браузера теста.
js=subprocess.check_output(['node','-e',"const fs=require('fs'),ts=require('./desktop/node_modules/typescript');console.log(ts.transpileModule(fs.readFileSync('desktop/src/addressSelection.ts','utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.None}}).outputText.replace(/export /g,''))"],cwd=root,text=True)
# CommonJS export boilerplate не нужен в HTML-странице стенда.
js=js.replace('Object.defineProperty(exports, "__esModule", { value: true });','').replace('"use strict";','')
js='var exports={};\n'+js
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,args=['--no-sandbox']);page=browser.new_page()
 page.set_content('<div id=root><button id=other>outside</button><input id=address value="https://example.test/word/next" style="font:20px monospace;width:600px"></div>')
 page.add_script_tag(content=js)
 page.evaluate('''() => {
 const input=document.querySelector('#address'),root=document.querySelector('#root');let start=null;
 window.shellFocus=0;window.contextPrevented=null;
 root.addEventListener('mousedown',e=>{if(e.button!==0||editableTarget(e.target))return;window.shellFocus++;},true);
 root.addEventListener('contextmenu',e=>{if(!editableTarget(e.target))e.preventDefault();});
 input.addEventListener('pointerdown',e=>start=addressDown(input,e.button,e.clientX,e.clientY));
 input.addEventListener('mouseup',e=>{if(addressUp(input,start,e.clientX,e.clientY,e.detail))e.preventDefault();start=null;});
 document.addEventListener('contextmenu',e=>window.contextPrevented=e.defaultPrevented);
 }''')
 inp=page.locator('#address');inp.click()
 assert inp.evaluate('(e)=>e.selectionStart===0&&e.selectionEnd===e.value.length')
 # Координаты слова в настоящем input, без ручной установки selectionRange.
 box=inp.bounding_box();x=box['x']+8+22*12.04;y=box['y']+box['height']/2
 page.mouse.dblclick(x,y)
 selected=inp.evaluate('(e)=>e.value.slice(e.selectionStart,e.selectionEnd)')
 assert selected=='word',selected
 page.mouse.click(x,y,button='right')
 assert inp.evaluate('(e)=>e.value.slice(e.selectionStart,e.selectionEnd)')=='word'
 assert page.evaluate('window.contextPrevented') is False
 assert page.evaluate('window.shellFocus')==0
 page.keyboard.press('Escape')
 page.mouse.move(box['x']+8+8*12.04,y);page.mouse.down();page.mouse.move(box['x']+8+15*12.04,y,steps=10);page.mouse.up()
 selected=inp.evaluate('(e)=>e.value.slice(e.selectionStart,e.selectionEnd)')
 assert selected and selected!=inp.input_value(),selected
 print(json.dumps({'first_click_selects_all':True,'double_click_selects_word':True,'right_click_keeps_selection_and_native_menu':True,'drag_selects_substring':True,'no_forced_shell_focus':True}))
 browser.close()
