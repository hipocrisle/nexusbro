"""Real Chromium checks of the exact Rust bootstrap template, without native CEF claims."""
import re,json,sys,threading,os,argparse
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parent))
from memory_safety_browser import Fixture,ThreadingHTTPServer
from playwright.sync_api import sync_playwright
root=Path(__file__).resolve().parents[1]
rust=(root/'desktop/src-tauri/src/cef_memory_renderer.rs').read_text()
# Exercise the actual Rust format literal, including JSON escapes and brace expansion.
literal=re.search(r'format!\(("(?:[^"\\]|\\.)*")',rust[rust.index('pub fn new_document_script'):]).group(1)
template=json.loads(literal)
safety=(root/'desktop/src-tauri/scripts/memory_safety.js').read_text()
parser=argparse.ArgumentParser();parser.add_argument('--output');args=parser.parse_args()
server=ThreadingHTTPServer(('127.0.0.1',0),Fixture);threading.Thread(target=server.serve_forever,daemon=True).start()
reports=[]
with sync_playwright() as pw:
 b=pw.chromium.launch(channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL','chrome') or None,headless=True,args=['--no-sandbox'])
 for name in ['nb_memory_0123456789abcdef','quotes"\\\n__SAFETY_SOURCE__']:
  nonce='fedcba98765432100123456789abcdef'
  source=template.format(json.dumps(name),json.dumps(nonce),safety)
  c=b.new_context();c.add_init_script('Object.defineProperty(window,'+json.dumps(name)+',{configurable:true,value:function(n,p){window.fixture={nonceOK:n==='+json.dumps(nonce)+',probe:p,deleted:!Object.hasOwn(window,'+json.dumps(name)+')};return true;}});'+source)
  p=c.new_page();p.goto(f'http://127.0.0.1:{server.server_port}/');v=p.evaluate('({nonceOK:fixture.nonceOK,deleted:fixture.deleted,result:JSON.parse(fixture.probe())})')
  assert v['nonceOK'] and v['deleted'] and v['result']['known'] and v['result']['reason'] is None,v
  reports.append(v);c.close()
 c=b.new_context();c.add_init_script(template.format(json.dumps('absent'),json.dumps('nonce'),safety));p=c.new_page();errors=[];p.on('pageerror',lambda e:errors.append(str(e)));p.goto(f'http://127.0.0.1:{server.server_port}/');assert not errors,errors;reports.append({'case':'absent_binding','page_errors':errors});c.close();b.close()
server.shutdown()
result={'cases':3,'results':reports}
if args.output:
 output=Path(args.output);output.parent.mkdir(parents=True,exist_ok=True);output.write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result))
