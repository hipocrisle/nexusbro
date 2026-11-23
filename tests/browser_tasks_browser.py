"""Real Chromium/React task manager acceptance with synthetic OS-only IPC.

Intervals use a controllable clock preserving the production 2000ms schedule;
this is not native-process or actual-desktop visibility acceptance.
"""
import ast
import functools
import http.server
import json
import os
import pathlib
import threading
from playwright.sync_api import expect, sync_playwright

ROOT = pathlib.Path(__file__).resolve().parents[1]
base_module = ast.parse((ROOT / 'tests/frontend_controls.py').read_text())
BASE = next(ast.literal_eval(n.value) for n in base_module.body if isinstance(n, ast.Assign)
            and any(isinstance(t, ast.Name) and t.id == 'mock' for t in n.targets))
MOCK = BASE + r'''
const originalInvoke=window.__TAURI_INTERNALS__.invoke;
window.__taskRequests=0;window.__inflight=0;window.__maxInflight=0;
window.__taskHold=false;window.__taskReplies=[];window.__taskError=false;
window.__taskHidden=false;window.__taskIntervals=new Map();window.__taskIntervalId=900000;
Object.defineProperty(document,'visibilityState',{configurable:true,get:()=>window.__taskHidden?'hidden':'visible'});
const originalInterval=window.setInterval.bind(window),originalClear=window.clearInterval.bind(window);
window.setInterval=(fn,delay,...args)=>{if(delay!==2000)return originalInterval(fn,delay,...args);const id=++window.__taskIntervalId;window.__taskIntervals.set(id,{delay,fn:()=>fn(...args)});return id;};
window.clearInterval=id=>{window.__taskIntervals.delete(id);return originalClear(id);};
window.__taskTick=()=>{for(const timer of [...window.__taskIntervals.values()])timer.fn();};
window.__taskSnapshot=(ms,samples,working=10485760,privateBytes=5242880)=>{
 const group={id:'super:fixture',super_id:'fixture',name:'Ресурсная Super',processes:samples.length,
  working_set_bytes:working,private_bytes:privateBytes,
  cpu_samples:samples.map(([identity,cpu_time_ms])=>({identity,cpu_time_ms}))};
 return {monotonic_ms:ms,logical_cpus:4,groups:[group],totals:{...group,id:'all',name:'All'},complete:true,memory_note:'Synthetic OS snapshot'};
};
window.__taskNext=window.__taskSnapshot(1000,[['10:1',100],['11:1',200]]);
window.__TAURI_INTERNALS__.invoke=async(cmd,args)=>{
 if(cmd!=='browser_tasks_snapshot')return originalInvoke(cmd,args);
 window.__taskRequests++;window.__inflight++;window.__maxInflight=Math.max(window.__maxInflight,window.__inflight);
 try{if(window.__taskHold)await new Promise(resolve=>window.__taskReplies.push(resolve));
  if(window.__taskError)throw Error('Synthetic OS access failed');
  return structuredClone(window.__taskNext);
 }finally{window.__inflight--;}
};
window.__releaseTask=()=>{window.__taskHold=false;for(const done of window.__taskReplies.splice(0))done();};
'''

class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_):
        pass


def main():
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0),
        functools.partial(Quiet, directory=str(ROOT / 'desktop/dist')))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(
                channel=os.environ.get('NEXUSBRO_PLAYWRIGHT_CHANNEL', 'chrome') or None,
                headless=True, args=['--no-sandbox'])
            page = browser.new_page(viewport={'width':1280,'height':820})
            page.add_init_script(MOCK)
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.goto(f'http://127.0.0.1:{server.server_port}')
            expect(page.get_by_title('Настройки NexusBro', exact=True)).to_be_visible()
            baseline_intervals = page.evaluate('window.__taskIntervals.size')
            page.evaluate('window.__taskTick()')
            assert page.evaluate('window.__taskRequests') == 0, 'Closed task manager polled OS'
            page.keyboard.press('Shift+Escape')
            dialog = page.get_by_role('dialog', name='Диспетчер задач NexusBro', exact=True)
            expect(dialog).to_be_visible()
            group = dialog.get_by_role('row').filter(has=page.get_by_role('rowheader', name='Ресурсная Super', exact=True))
            expect(group).to_be_visible()
            expect(group.locator('td').nth(0)).to_have_text('…')
            for width in [1280,900]:
                page.set_viewport_size({'width':width,'height':820})
                page.evaluate('()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')
                fit=dialog.locator('table').evaluate('t=>{const p=t.parentElement,r=p.getBoundingClientRect();return {overflow:p.scrollWidth>p.clientWidth+1,cells:[...t.querySelectorAll("th,td")].every(c=>{const q=c.getBoundingClientRect();return q.left>=r.left-1&&q.right<=r.right+1;})};}')
                assert not fit['overflow'] and fit['cells'],fit
            page.set_viewport_size({'width':1280,'height':820})

            expect(dialog.get_by_role('button',name='Закрыть диспетчер задач')).to_be_focused()
            page.keyboard.press('Tab')
            expect(dialog.get_by_role('button',name='Закрыть диспетчер задач')).to_be_focused()
            assert page.evaluate('window.__taskIntervals.size') == baseline_intervals + 1
            assert all(delay == 2000 for delay in page.evaluate('[...window.__taskIntervals.values()].map(t=>t.delay)'))
            assert page.evaluate('window.__taskRequests') == 1
            page.evaluate("window.__taskNext=window.__taskSnapshot(3000,[['10:1',1100],['11:1',600]]);window.__taskTick()")
            expect(group.locator('td').nth(0)).to_have_text('17.5%')
            # Dead process disappears; huge cumulative time in a new identity is
            # not treated as CPU consumed during this sampling interval.
            page.evaluate("window.__taskNext=window.__taskSnapshot(5000,[['10:1',1300],['12:1',100000]]);window.__taskTick()")
            expect(group.locator('td').nth(0)).to_have_text('2.5%')
            # PID reused with a different creation identity: no giant CPU spike.
            page.evaluate("window.__taskNext=window.__taskSnapshot(7000,[['10:2',900000]],null,null);window.__taskTick()")
            expect(group.locator('td').nth(0)).to_have_text('0.0%')
            expect(group.locator('td').nth(1)).to_have_text('Недоступно')
            expect(group.locator('td').nth(2)).to_have_text('Недоступно')
            page.evaluate("window.__taskNext=window.__taskSnapshot(1,[['10:2',1]]);window.__taskNext.complete=false;window.__taskTick()")
            expect(group.locator('td').nth(0)).to_have_text('…')
            expect(dialog.get_by_role('status')).to_contain_text('Часть процессов недоступна')
            before = page.evaluate('window.__taskRequests')
            page.evaluate('window.__taskHidden=true;window.__taskTick();window.__taskTick()')
            assert page.evaluate('window.__taskRequests') == before, 'Hidden task manager kept polling'
            page.evaluate('window.__taskHidden=false;window.__taskError=true;window.__taskTick()')
            expect(dialog.get_by_role('alert')).to_have_text('Error: Synthetic OS access failed')
            page.evaluate("window.__taskError=false;window.__taskNext=window.__taskSnapshot(9000,[['10:2',201]]);window.__taskTick()")
            expect(dialog.get_by_role('alert')).to_have_count(0)
            # Slow native query cannot overlap subsequent interval ticks.
            before = page.evaluate('window.__taskRequests')
            page.evaluate('window.__taskHold=true;window.__taskTick();window.__taskTick();window.__taskTick()')
            assert page.evaluate('window.__taskRequests') == before + 1
            assert page.evaluate('window.__maxInflight') == 1
            page.keyboard.press('Escape')
            expect(dialog).to_have_count(0)
            assert page.evaluate('window.__taskIntervals.size') == baseline_intervals
            page.evaluate('window.__releaseTask();window.__taskTick()')
            expect(dialog).to_have_count(0)
            assert page.evaluate('window.__taskRequests') == before + 1
            page.keyboard.press('Shift+Escape')
            expect(dialog).to_be_visible()
            expect(dialog.get_by_role('rowheader',name='Ресурсная Super',exact=True)).to_be_visible()
            # A newly mounted manager starts a new delta baseline.
            reopened = dialog.get_by_role('row').filter(has=page.get_by_role('rowheader',name='Ресурсная Super',exact=True))
            expect(reopened.locator('td').nth(0)).to_have_text('…')
            dialog.get_by_role('button',name='Закрыть диспетчер задач').click()
            expect(dialog).to_have_count(0)
            page.evaluate('window.__taskTick()')
            assert page.evaluate('window.__taskIntervals.size') == baseline_intervals
            assert not errors, errors
            browser.close()
            print(json.dumps({'real_chromium':True,'os_ipc':'synthetic fixture',
                'closed_no_poll':True,'interval_2000ms_only_while_open':True,
                'singleflight_slow_query':True,'hidden_no_poll_fixture':True,
                'cpu_matching_identity_deltas_machine_percent':True,'process_exit_and_pid_reuse_no_spike':True,
                'unknown_memory_not_zero':True,'counter_reset_and_reopen_baseline':True,
                'error_recovery':True,'close_ignores_late_reply':True,'shift_escape_and_focus_trap':True}))
    finally:
        server.shutdown()
        server.server_close()


if __name__ == '__main__':
    main()
