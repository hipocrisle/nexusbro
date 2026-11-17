"""Live Google protocol+DOM check with synthetic text and an explicitly chosen existing proxy.

Does not build Rust and does not claim to execute native perevod.rs. Native engine
and retry/concurrency are checked by Windows CI separately.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import urllib.parse

from playwright.sync_api import sync_playwright

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--proxy-url',required=True)
args=p.parse_args()
proxy=urllib.parse.urlsplit(args.proxy_url)
assert proxy.scheme=='http' and proxy.hostname in ('127.0.0.1','localhost') and proxy.port
script=(Path(__file__).resolve().parents[1]/'desktop/src-tauri/scripts/perevod.js').read_text()
with sync_playwright() as pw:
 browser=pw.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome' if shutil.which('google-chrome') else '') or None,args=['--no-sandbox'])
 page=browser.new_page()
 page.route('https://translation.test/**',lambda r:r.fulfill(content_type='text/html',body='<main><h1>Guten Tag.</h1><p>Dies ist ein kurzer Übersetzungstest.</p><p>Die Bibliothek ist heute geöffnet.</p><input value="Unsent private draft"></main>'))
 page.goto('https://translation.test/first')
 snap=page.evaluate(script.replace('__TOKEN__',json.dumps('live-0214')))
 query='\n'.join(f'\ue000{i}\ue001\n{item["text"]}' for i,item in enumerate(snap['items']))
 url='https://translate.googleapis.com/translate_a/single?'+urllib.parse.urlencode({'client':'dict-chrome-ex','sl':'auto','tl':'ru','dt':'t','q':query})
 start=time.monotonic()
 response=subprocess.run(['curl','--fail-with-body','--silent','--show-error','--max-time','20','--proxy',args.proxy_url,url],capture_output=True,text=True,timeout=25)
 assert response.returncode==0,{'exit':response.returncode,'reason':'live translator request failed; no direct fallback'}
 result=json.loads(response.stdout)
 translated=''.join(sentence[0] for sentence in result[0])
 # Validate the marker protocol; translated newline count is deliberately irrelevant.
 values=[]
 for i,item in enumerate(snap['items']):
  marker=f'\ue000{i}\ue001'
  assert translated.startswith(marker),'provider changed segment boundaries'
  translated=translated[len(marker):]
  if i+1<len(snap['items']):
   marker=f'\ue000{i+1}\ue001'
   text,_,rest=translated.partition(marker)
   assert rest,'provider removed a segment marker'
   translated=marker+rest
  else:text=translated
  values.append({'id':item['id'],'text':text.strip()})
 page.evaluate("items=>window.__nexusbroTranslation.apply('live-0214',items)",values)
 assert any('А'<=c<='я' for c in page.locator('h1').inner_text())
 assert page.locator('input').input_value()=='Unsent private draft'
 page.evaluate('window.__nexusbroTranslation.restore()')
 assert page.locator('h1').inner_text()=='Guten Tag.'
 print(json.dumps({'live_google_via_explicit_existing_proxy':True,'segment_boundaries_preserved':True,'synthetic_document_translated_and_restored':True,'draft_not_sent':True,'requests':1,'seconds':round(time.monotonic()-start,3),'scope':'live wire protocol and actual Chromium DOM; native Rust not executed'}))
 browser.close()
