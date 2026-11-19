"""Actual Chromium regression and measured cost of the shipped cosmetic script.

Only local synthetic pages; baseline read from an explicit Git ref. This does
not measure total NexusBro CPU or establish the cause of a user's screenshot.
"""
import argparse
import json
import os
import pathlib
import subprocess
from playwright.sync_api import sync_playwright

ROOT = pathlib.Path(__file__).resolve().parents[1]
SOURCE = 'desktop/src-tauri/src/borba_s_reklamoy.rs'


def script(rust):
    extractor = (ROOT / 'tests/reklama_stranicy.js').read_text().split('// ── мок страницы')[0]
    start = extractor.index('const rust =')
    end = extractor.index('// ── сборка скрипта')
    extractor = extractor[:start] + 'const rust=' + json.dumps(rust) + ';\n' + extractor[end:]
    return subprocess.check_output(['node', '-e', extractor + '\nconsole.log(gotovyj);'], cwd=ROOT / 'tests', text=True)


INSTRUMENT = r'''() => {
 window.__cost={rect:0,text:0,slices:0,longestSlice:0};
 const rect=Element.prototype.getBoundingClientRect;
 Element.prototype.getBoundingClientRect=function(){__cost.rect++;return rect.call(this)};
 const text=Object.getOwnPropertyDescriptor(HTMLElement.prototype,'innerText');
 Object.defineProperty(HTMLElement.prototype,'innerText',{get(){__cost.text++;return text.get.call(this)},set(v){return text.set.call(this,v)},configurable:true});
 const timer=window.setTimeout;
 window.setTimeout=function(fn,delay,...args){return timer.call(this,()=>{const t=performance.now();try{return fn(...args)}finally{__cost.slices++;__cost.longestSlice=Math.max(__cost.longestSlice,performance.now()-t)}},delay)};
}'''

FIXTURE = r'''() => {
 const root=document.createElement('main');root.id='feed';document.body.append(root);
 for(let i=0;i<320;i++){
  const article=document.createElement('article');article.id='story-'+i;root.append(article);
  for(let j=0;j<8;j++){const row=document.createElement('div');row.innerHTML='<span>Обычная новость '+i+' '+j+'</span><a href="#item">Подробности события</a>';article.append(row)}
 }
 for(const [id,tag,text,klass] of [
 ['ad-text','aside','Скачайте Chrome — быстрее и безопаснее',''],
 ['ad-class','ins','Специальное предложение','advert-banner'],
 ['ad-marker','a','Купите диван со скидкой\nРЕКЛАМА\nмагазин.ру',''],
 ['keep-help','a','Как отключить рекламу в браузере — подробная инструкция со скриншотами',''],
 ]){const el=document.createElement(tag);el.id=id;el.textContent=text;el.className=klass;el.style.cssText='display:block;white-space:pre-line;width:260px;height:60px';root.append(el)}
 const huge=document.createElement('div');huge.id='keep-huge';huge.style.cssText='height:900px;width:1000px';huge.textContent='Скачайте Chrome';root.append(huge);
 const form=document.createElement('form');form.id='keep-form';form.innerHTML='<input value="Несохранённый текст"><button>Сохранить</button>';root.append(form);
}'''


def metric(cdp):
    return {v['name']: v['value'] for v in cdp.send('Performance.getMetrics')['metrics']}


def run(browser, js, initial=False):
    page = browser.new_page(viewport={'width':1280,'height':800})
    page.set_content('<!doctype html><style>body{margin:0}article{display:block}div,span,a{font:12px sans-serif}</style><body></body>')
    cdp = page.context.new_cdp_session(page)
    cdp.send('Performance.enable')
    page.evaluate(INSTRUMENT)
    if initial:page.evaluate(FIXTURE)
    before = metric(cdp)
    page.evaluate(js)
    if not initial:page.evaluate(FIXTURE)
    page.wait_for_function("!document.querySelector('#ad-text,#ad-class,#ad-marker')", timeout=15000)
    page.wait_for_timeout(300)
    result = page.evaluate('''() => ({...__cost, articles:document.querySelectorAll('article').length, rows:document.querySelectorAll('article div').length, help:!!document.querySelector('#keep-help'), huge:!!document.querySelector('#keep-huge'), draft:document.querySelector('#keep-form input')?.value, ads:document.querySelectorAll('#ad-text,#ad-class,#ad-marker').length})''')
    after = metric(cdp)
    result['cdp'] = {k:after.get(k,0)-before.get(k,0) for k in ['TaskDuration','LayoutDuration','RecalcStyleDuration','LayoutCount','RecalcStyleCount']}
    assert result['articles'] == 320 and result['rows'] == 2560, result
    assert result['help'] and result['huge'] and result['draft'] == 'Несохранённый текст' and result['ads'] == 0, result
    # New mutation wave, nested roots, content added after the first scan finished.
    page.evaluate("""() => {const outer=document.createElement('section');document.querySelector('#feed').append(outer);const child=document.createElement('aside');child.id='late-ad';child.textContent='Установите другой браузер';child.style.cssText='width:260px;height:60px';outer.append(child);const safe=document.createElement('span');safe.id='late-content';safe.textContent='Полезный материал';outer.append(safe);} """)
    page.wait_for_function("!document.querySelector('#late-ad')")
    result['late_nested_content_preserved'] = page.locator('#late-content').count() == 1
    # Re-injection is a no-op, and there is no periodic scanning after settling.
    page.evaluate(js)
    page.wait_for_timeout(300)
    idle = page.evaluate('({...__cost})')
    page.wait_for_timeout(250)
    assert page.evaluate('({...__cost})') == idle, 'Cosmetics did work without DOM mutations'
    result['late_nested_ad_removed'] = True
    result['idle_no_periodic_work'] = True
    page.close()
    return result


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--baseline',default='097a659')
    parser.add_argument('--output')
    args=parser.parse_args()
    old=subprocess.check_output(['git','show',f'{args.baseline}:{SOURCE}'],cwd=ROOT,text=True)
    new=(ROOT/SOURCE).read_text()
    with sync_playwright() as pw:
        browser=pw.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None,headless=True,args=['--no-sandbox'])
        try:
            baseline=run(browser,script(old));candidate=run(browser,script(new))
            initial_baseline=run(browser,script(old),initial=True);initial_candidate=run(browser,script(new),initial=True)
        finally:browser.close()
    assert candidate['rect'] <= baseline['rect'] * .05, (baseline,candidate)
    assert candidate['text'] <= baseline['text'] * .05, (baseline,candidate)
    assert candidate['slices'] > 1, candidate
    assert candidate['late_nested_content_preserved'], candidate
    assert initial_candidate['late_nested_content_preserved'], initial_candidate
    assert initial_candidate['rect'] <= initial_baseline['rect'] * .05, (initial_baseline,initial_candidate)
    result={'baseline_ref':args.baseline,'baseline':baseline,'candidate':candidate,'initial_baseline':initial_baseline,'initial_candidate':initial_candidate,'scope':'synthetic real Chromium cosmetic script only; not total browser CPU'}
    if args.output:pathlib.Path(args.output).write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps(result,ensure_ascii=False))

if __name__=='__main__':main()
