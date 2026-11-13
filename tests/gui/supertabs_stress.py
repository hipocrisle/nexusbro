"""Реальный GUI NexusBro: WebKit Inspector, React-события и команды Tauri.
Запуск после приложения с WEBKIT_INSPECTOR_HTTP_SERVER=127.0.0.1:9223.
Зависимость: websocket-client. Данные приложения должны быть изолированы.
"""
import argparse
import json
import pathlib
import re
import os
import signal
import shutil
import subprocess
import time
import websocket

parser = argparse.ArgumentParser()
parser.add_argument('--stress', action='store_true')
parser.add_argument('--target', default='page-7')
parser.add_argument('--data', default='bench-artifacts/final-data')
parser.add_argument('--display', default=':92')
parser.add_argument('--out', default='bench-artifacts/gui-results.json')
args = parser.parse_args()
from client import Client
client = Client(args.display)
ws = client.ws
args.target = client.target
seq = 0
results = []

def js(expression):
    global seq
    seq += 1
    request = seq
    ws.send(json.dumps({'id':request, 'method':'Target.sendMessageToTarget', 'params':{
        'targetId':args.target, 'message':json.dumps({'id':request, 'method':'Runtime.evaluate',
        'params':{'expression':expression, 'returnByValue':True}})}}))
    while True:
        message = json.loads(ws.recv())
        if message.get('method') != 'Target.dispatchMessageFromTarget':
            if message.get('error'):
                raise RuntimeError(message)
            continue
        response = json.loads(message['params']['message'])
        if response.get('id') != request:
            continue
        if response.get('error') or response.get('result', {}).get('wasThrown'):
            raise RuntimeError(response)
        return response['result']['result'].get('value')

def until(expression, timeout=8):
    start = time.monotonic()
    while time.monotonic()-start < timeout:
        value = js(expression)
        if value:
            return value
        time.sleep(.02)
    raise AssertionError(f'Таймаут: {expression}; DOM={js("document.body.innerText")}')

def command(name, **params):
    js('window.__itog=null; window.__TAURI_INTERNALS__.invoke('+json.dumps(name)+','+json.dumps(params)+''').then(r=>window.__itog={r},e=>window.__itog={error:String(e)}); true''')
    result = until('window.__itog', 30)
    assert 'error' not in result, result
    if isinstance(result['r'], dict) and 'ok' in result['r']:
        assert result['r']['ok'], result
    return result['r']

def click(selector):
    js('document.querySelector('+json.dumps(selector)+').click(); true')

def click_text(text):
    js('Array.from(document.querySelectorAll("button,.stroka-menyu")).find(x=>x.textContent.trim()==='+json.dumps(text)+').click(); true')

def input_value(index, value):
    js('''(()=>{let x=document.querySelectorAll('.dialog input:not([type=checkbox])')['''+str(index)+'''];
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(x,'''+json.dumps(value)+''');
    x.dispatchEvent(new Event('input',{bubbles:true}));return true})()''')

def measure(label, action, condition):
    started = time.monotonic()
    action()
    until(condition)
    result = {'action':label, 'ui_ms':round((time.monotonic()-started)*1000,1)}
    results.append(result)
    print(json.dumps(result,ensure_ascii=False), flush=True)

def processes():
    entries = []
    for entry in pathlib.Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            cmd = (entry/'cmdline').read_bytes().split(b'\0')
            if b'--nb-dvizhok' in cmd and any(str(pathlib.Path(args.data).resolve()).encode() in x for x in cmd):
                if not any(x.startswith(('--user-data-dir='+str(pathlib.Path(args.data).resolve())+'/').encode()) for x in cmd):
                    continue
                entries.append({'pid':int(entry.name), 'profile':pathlib.Path(next(x.decode().split('=',1)[1] for x in cmd if x.startswith(b'--user-data-dir='))).name})
        except (OSError,StopIteration):
            pass
    return entries

def native_states(hidden=False):
    active = command('aktivnaya')
    states = []
    for tab in command('spisok'):
        if not tab['zapushchena']:
            continue
        log = (pathlib.Path(args.data)/'NexusBro/zhurnal.log').read_text()
        handles = re.findall(re.escape(tab['id'])+r': запуск ок, окно (\d+)',log)
        assert handles, tab
        handle = handles[-1]
        info = subprocess.check_output(['xwininfo','-id',handle,'-all'],env={**os.environ,'DISPLAY':args.display},text=True)
        mapped = 'Map State: IsViewable' in info
        assert mapped == (tab['id']==active and not hidden), info
        assert re.search(r'1 child:',info), info
        states.append({'id':tab['id'],'mapped':mapped,'has_cef_child':True})
    return states

try:
    js('window.__lagi=[]; window.__last=performance.now(); window.__timer=setInterval(()=>{let n=performance.now();window.__lagi.push(n-window.__last);window.__last=n},20); true')
    # Создание через форму, затем сохранение/открытие через меню.
    measure('открыть форму',lambda:click('.niz button'), '!!document.querySelector(".dialog input")')
    input_value(0,'Bench GUI A')
    input_value(2,'data:text/html,<body style="background:%231e5f45;color:white"><h1>Bench GUI A</h1></body>')
    # Последняя галка — стартовый адрес вместо восстановления сессии.
    js('document.querySelectorAll(".dialog input[type=checkbox]")[1].click(); true')
    measure('создать A',lambda:click_text('Создать'), '!document.querySelector(".dialog") && document.querySelectorAll(".vkladka").length===1 && !document.querySelector(".tochka.spit")')
    a = command('spisok')[0]['id']
    until('document.querySelector(".vkladka .tochka") && !document.querySelector(".vkladka .tochka").classList.contains("spit")')
    time.sleep(.5)
    results.append({'native_after_creation':native_states()})
    before = processes()
    results.append({'engines_before_close':before})
    # Отмена и клик по фону должны убирать вопрос.
    for answer in ['Отмена','фон']:
        click('.vkladka .krest')
        until('!!document.querySelector(".zatemnenie .knopki")')
        native_states(hidden=True)
        measure('ответ '+answer, lambda: click_text(answer) if answer!='фон' else click('.zatemnenie'), '!document.querySelector(".zatemnenie")')
        assert len(command('spisok'))==1
    click('.vkladka .krest')
    until('!!document.querySelector(".zatemnenie .knopki")')
    measure('сохранить и закрыть A',lambda:click_text('Сохранить'), '!document.querySelector(".zatemnenie") && !document.querySelector(".vkladka")')
    time.sleep(.3)
    after = processes()
    assert not after, after
    results.append({'engines_after_close':after})
    click('.niz button:nth-child(2)')
    until('!!document.querySelector(".stroka-menyu")')
    measure('открыть сохранённую A',lambda:click('.stroka-menyu'), '!!document.querySelector(".vkladka.aktivna") && !document.querySelector(".vypadajka") && !document.querySelector(".tochka.spit")')
    until('!document.querySelector(".vkladka .tochka").classList.contains("spit")')
    results.append({'native_after_restore':native_states()})
    # Правка работающей вкладки через контекстное меню.
    js('document.querySelector(".vkladka").dispatchEvent(new MouseEvent("contextmenu",{bubbles:true,clientX:50,clientY:65}));true')
    until('!!document.querySelector(".stroka-menyu")')
    native_states(hidden=True)
    click_text('Настройки')
    until('!!document.querySelector(".dialog input")')
    input_value(0,'Bench GUI edited')
    measure('правка работающей A',lambda:click_text('Сохранить'), '!document.querySelector(".dialog") && document.querySelector(".imya").textContent==="Bench GUI edited" && !document.querySelector(".tochka.spit")')
    until('!document.querySelector(".vkladka .tochka").classList.contains("spit")')
    results.append({'native_after_edit':native_states()})
    # Вторая вкладка, переключение и сохранность соседнего движка.
    click('.niz button')
    until('!!document.querySelector(".dialog input")')
    input_value(0,'Bench GUI B')
    measure('создать B',lambda:click_text('Создать'), 'document.querySelectorAll(".vkladka").length===2 && !document.querySelector(".dialog") && !document.querySelector(".tochka.spit")')
    until('Array.from(document.querySelectorAll(".vkladka .tochka")).every(x=>!x.classList.contains("spit"))')
    measure('переключить на A',lambda:click('.vkladka'), 'document.querySelector(".aktivnaya-imya").textContent==="Bench GUI edited"')
    measure('переключить на B',lambda:click('.vkladka:nth-child(2)'), 'document.querySelector(".aktivnaya-imya").textContent==="Bench GUI B"')
    results.append({'native_after_switch':native_states()})
    click('.vkladka:nth-child(2) .krest')
    until('!!document.querySelector(".zatemnenie .knopki")')
    measure('удалить B без сохранения',lambda:click_text('Не сохранять'), 'document.querySelectorAll(".vkladka").length===1 && !document.querySelector(".zatemnenie")')
    assert len(processes())==1, processes()
    assert command('aktivnaya')==a
    assert js('document.querySelector(".aktivnaya-imya").textContent')=='Bench GUI edited'
    # Повторное открытие профиля после удаления ранее оставляло живой CEF.
    for i in range(3):
        measure(f'закрыть A, цикл {i+1}',lambda:click('.vkladka .krest'), '!document.querySelector(".vkladka") && !document.querySelector(".zatemnenie")')
        assert not processes(), processes()
        click('.niz button:nth-child(2)'); until('!!document.querySelector(".stroka-menyu")')
        measure(f'открыть A, цикл {i+1}',lambda:click('.stroka-menyu'), '!!document.querySelector(".vkladka.aktivna") && !document.querySelector(".vypadajka") && !document.querySelector(".tochka.spit")')
        until('!document.querySelector(".vkladka .tochka").classList.contains("spit")')
    if args.stress:
        nast = lambda id: dict(id=id,nazvanie=id,proxy='',adres='about:blank',pustaya=True,rezat_reklamu=True)
        for i in range(5):
            id = f'bench-pending-{i}'
            command('otkryt',nastrojki=nast(id))
            pid = next(x['pid'] for x in processes() if x['profile']==id)
            os.kill(pid,signal.SIGSTOP)
            start = time.monotonic()
            command('zakryt',id=id,sohranit=False)
            elapsed = round((time.monotonic()-start)*1000,1)
            assert not pathlib.Path(f'/proc/{pid}').exists()
            assert not (pathlib.Path(args.data)/'NexusBro/profili'/id).exists()
            assert len(processes())==1, processes()
            assert elapsed<1000
            results.append({'close_stopped_startup':i+1,'ms':elapsed,'pid_reaped':True})
        id = 'bench-pending-edit'
        command('otkryt',nastrojki=nast(id))
        old_pid = next(x['pid'] for x in processes() if x['profile']==id)
        os.kill(old_pid,signal.SIGSTOP)
        n = nast(id); n['nazvanie']='edited during startup'
        command('izmenit',nastrojki=n)
        until('Array.from(document.querySelectorAll(".vkladka .tochka")).every(x=>!x.classList.contains("spit"))')
        assert not pathlib.Path(f'/proc/{old_pid}').exists()
        assert len([x for x in processes() if x['profile']==id])==1
        native_states()
        command('zakryt',id=id,sohranit=False)
        results.append({'edit_during_stopped_startup':True,'old_pid_reaped':True})
        id = 'bench-delayed'
        command('otkryt',nastrojki=nast(id))
        pid = next(x['pid'] for x in processes() if x['profile']==id)
        os.kill(pid,signal.SIGSTOP)
        js('window.__duplicates=null;Promise.all(Array.from({length:20},()=>window.__TAURI_INTERNALS__.invoke("pereklyuchit",{id:"bench-delayed"}))).then(r=>window.__duplicates=r);true')
        until('window.__duplicates')
        assert len([x for x in processes() if x['profile']==id])==1
        command('pereklyuchit',id=a)
        os.kill(pid,signal.SIGCONT)
        until('Array.from(document.querySelectorAll(".vkladka .tochka")).every(x=>!x.classList.contains("spit"))')
        assert command('aktivnaya')==a
        native_states()
        command('zakryt',id=id,sohranit=True)
        shutil.rmtree(pathlib.Path(args.data)/'NexusBro/profili'/id)
        start=time.monotonic()
        command('udalit_sohranennuyu',id=id)
        elapsed=round((time.monotonic()-start)*1000,1)
        assert elapsed<1000
        results.append({'duplicate_switches':20,'single_engine':True,'late_start_did_not_steal_focus':True,'delete_missing_profile_ms':elapsed})
        time.sleep(.5)
        assert len(processes())==1
        assert command('aktivnaya')==a
        assert len(command('spisok'))==1
        results.append({'no_ghost_tabs_after_cancel':True})
    results.append({'native_final':native_states()})
    results.append({'max_timer_interval_ms':round(js('Math.max(...window.__lagi)'),1),'engine_count_final':len(processes()),'overlay_final':js('!!document.querySelector(".zatemnenie")')})
    assert js('Math.max(...window.__lagi)')<1000
    assert all(r.get('ui_ms',0)<1000 for r in results if not r.get('action','').startswith('создать'))
    assert not js('!!document.querySelector(".zatemnenie")')
    results.append({'success':True})
finally:
    pathlib.Path(args.out).write_text(json.dumps(results,ensure_ascii=False,indent=2)+'\n')
    ws.close()
