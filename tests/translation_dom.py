import os,shutil
"""Реальный DOM: перевод на месте, восстановление, поля и устаревшие ответы."""
import json
from pathlib import Path
from playwright.sync_api import sync_playwright
script=(Path(__file__).resolve().parents[1]/'desktop/src-tauri/scripts/perevod.js').read_text()
with sync_playwright() as pw:
 browser=pw.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome' if shutil.which('google-chrome') else '') or None,args=['--no-sandbox'])
 page=browser.new_page()
 page.route('https://translation.test/**',lambda r:r.fulfill(content_type='text/html',body='''<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'none'"><h1>Hello world</h1><p id=dynamic>Original news</p><input value="Private form"><textarea>Private notes</textarea><pre>Code stays English</pre><div contenteditable>Unsent draft</div><div translate=no>Do not translate</div>'''))
 page.goto('https://translation.test/page')
 before=page.url
 snapshot=page.evaluate(script.replace('__TOKEN__',json.dumps('test-one')))
 assert [i['text'] for i in snapshot['items']]==['Hello world','Original news'],snapshot
 page.evaluate("items=>window.__nexusbroTranslation.apply('test-one',items)",[{'id':0,'text':'Привет, мир'},{'id':1,'text':'Новости'}])
 assert page.locator('h1').inner_text()=='Привет, мир'
 assert page.url==before and len(page.context.pages)==1
 assert page.locator('input').input_value()=='Private form'
 assert page.locator('textarea').input_value()=='Private notes'
 assert page.locator('pre').inner_text()=='Code stays English'
 assert page.locator('[contenteditable]').inner_text()=='Unsent draft'
 assert page.locator('[translate=no]').inner_text()=='Do not translate'
 page.locator('#dynamic').evaluate("el=>el.textContent='Updated by site'")
 page.evaluate('window.__nexusbroTranslation.restore()')
 assert page.locator('h1').inner_text()=='Hello world'
 assert page.locator('#dynamic').inner_text()=='Updated by site'
 page.evaluate(script.replace('__TOKEN__',json.dumps('test-two')))
 page.evaluate("window.__nexusbroTranslation.apply('test-one',[{id:0,text:'Wrong'}])")
 assert page.locator('h1').inner_text()=='Hello world'
 page.evaluate("history.pushState({},'', '/changed');window.__nexusbroTranslation.apply('test-two',[{id:0,text:'Wrong'}])")
 assert page.locator('h1').inner_text()=='Hello world'
 print(json.dumps({'same_document':True,'original_restored':True,'forms_code_and_drafts_untouched':True,'site_edits_preserved':True,'stale_token_and_navigation_rejected':True}))
 browser.close()
