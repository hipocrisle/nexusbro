"""Запускать из корня репозитория после cargo build; нужен отдельный Xvfb.
Все страницы и данные локальные. Два процесса супер-вкладок, без внешней сети.
"""
import argparse
import http.server
import json
import os
import pathlib
import re
import signal
import socket
import subprocess
import threading
import time
import urllib.parse
from client import Client

parser=argparse.ArgumentParser()
parser.add_argument('--display',default=':94')
parser.add_argument('--port',type=int,default=9223)
parser.add_argument('--out',default='.dev-check/ordinary-results.json')
args=parser.parse_args()
root=pathlib.Path.cwd()
run=root/'.dev-check'/f'ordinary-{time.time_ns()}'
run.mkdir(parents=True)
results=[]
class Fixture(http.server.BaseHTTPRequestHandler):
    reloads=0
    def do_GET(self):
        path=urllib.parse.urlparse(self.path).path
        title={'/one':'One','/two':'Two','/popup':'Popup','/set':'CookieSet'}.get(path,'Unknown')
        if path=='/reload':
            type(self).reloads+=1
            title=f'Reload{type(self).reloads}'
        if path=='/read':title='Cookie-A' if 'owner=A' in self.headers.get('Cookie','') else 'Cookie-none'
        html=f'''<!doctype html><title>{title}</title><body style="background:#185541;color:white;font:20px sans-serif">
        <h1>{title}</h1><input style="position:absolute;left:20px;top:170px;width:200px;height:30px" oninput="document.title=\'Typed:\'+this.value"><a style="position:absolute;left:20px;top:90px;padding:12px;background:white" target="_blank" href="/popup">Popup</a></body>'''.encode()
        self.send_response(200)
        self.send_header('Content-Type','text/html; charset=utf-8')
        if path=='/set':self.send_header('Set-Cookie','owner=A; Path=/; Max-Age=3600')
        self.send_header('Content-Length',str(len(html)))
        self.end_headers();self.wfile.write(html)
    def log_message(self,*_args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Fixture)
threading.Thread(target=server.serve_forever,daemon=True).start()
base=f'http://127.0.0.1:{server.server_port}'
env={**os.environ,'DISPLAY':args.display,'CEF_PATH':str(root/'desktop/src-tauri/cef'),
    'XDG_DATA_HOME':str(run/'data'),'XDG_CONFIG_HOME':str(run/'config'),'XDG_CACHE_HOME':str(run/'cache'),
    'WEBKIT_INSPECTOR_HTTP_SERVER':f'127.0.0.1:{args.port}','WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS':'1'}
app=None;client=None;log=None

def start():
    global app,client,log
    with socket.socket() as probe:
        probe.settimeout(.2)
        assert probe.connect_ex(('127.0.0.1',args.port))!=0,'порт инспектора уже занят другим приложением'
    log=(run/f'app-{len(results)}.log').open('w')
    app=subprocess.Popen([str(root/'desktop/src-tauri/target/debug/nexusbro')],env=env,stdout=log,stderr=log)
    for _ in range(450):
        if app.poll() is not None:raise AssertionError(f"приложение завершилось: {app.returncode}")
        try:client=Client(args.display,args.port);break
        except (OSError,ConnectionError,websocket_error):time.sleep(.1)
    assert client
    wins=subprocess.check_output(['xdotool','search','--onlyvisible','--name','^NexusBro$'],env=env,text=True).splitlines()
    subprocess.run(['xdotool','windowfocus',wins[0]],env=env,check=True)
    subprocess.run(['xdotool','windowsize',wins[0],'1280','820'],env=env,check=True)
    client.wait('!!document.querySelector(".niz")')

def stop():
    try:client.js('window.__TAURI_INTERNALS__.invoke("plugin:process|exit",{code:0});true')
    except (websocket_error,KeyError):pass
    app.wait(timeout=8);assert app.returncode==0
    client.close();log.close()
    time.sleep(.3)

from websocket import WebSocketException as websocket_error

def sess(id):return client.command('obychnye_spisok',id=id)
def wait_sess(id,predicate):
    for _ in range(150):
        s=sess(id)
        if predicate(s):return s
        time.sleep(.03)
    raise AssertionError(s)
def active(s):return next(t for t in s['vkladki'] if t['id']==s['aktivnaya'])
def navigate(id,path,title):
    client.wait('!!document.querySelector(".obychnaya[aria-selected=true]") && !document.querySelector(".navigaciya input").disabled')
    client.fill('.navigaciya input',base+path,enter=True)
    return wait_sess(id,lambda s:active(s)['zagolovok']==title and not active(s)['zagruzka'])
def engines():
    found=[]
    for p in pathlib.Path('/proc').iterdir():
        if not p.name.isdigit():continue
        try:
            cmd=(p/'cmdline').read_bytes().split(b'\0')
            if b'--nb-dvizhok' in cmd and any(str(run).encode() in x for x in cmd):found.append(int(p.name))
        except OSError:pass
    return found

def supertab(name):
    client.click('.niz button')
    client.wait('!!document.querySelector(".dialog input")')
    client.fill('.dialog input',name)
    client.text_button('Создать')
    client.wait('!document.querySelector(".dialog")')
    client.wait('!document.querySelector(".vkladka .tochka.spit")')
    return next(v['id'] for v in client.command('spisok') if v['nazvanie']==name)

def native(id,count,width=1280,height=820):
    journal=(run/'data/NexusBro/zhurnal.log').read_text()
    handles=re.findall(re.escape(id)+r': запуск ок, окно (\d+)',journal)
    assert handles
    info=subprocess.check_output(['xwininfo','-id',handles[-1],'-all'],env=env,text=True,stderr=subprocess.DEVNULL)
    assert 'Map State: IsViewable' in info,info
    x=int(re.search(r'Absolute upper-left X:\s*(\d+)',info)[1]);y=int(re.search(r'Absolute upper-left Y:\s*(\d+)',info)[1])
    w=int(re.search(r'Width:\s*(\d+)',info)[1]);h=int(re.search(r'Height:\s*(\d+)',info)[1])
    assert w+x==width and h+y==height,info
    tree=re.findall(r'^( +)(0x[0-9a-f]+) ',info,re.M)
    depth=min(len(indent) for indent,_ in tree)
    children=[handle for indent,handle in tree if len(indent)==depth]
    assert len(children)==count,info
    visible=0
    for child in children:
        c=subprocess.check_output(['xwininfo','-id',child,'-stats'],env=env,text=True,stderr=subprocess.DEVNULL)
        assert int(re.search(r'Width:\s*(\d+)',c)[1])==w and int(re.search(r'Height:\s*(\d+)',c)[1])==h,c
        visible+=('Map State: IsViewable' in c)
    assert visible==1,info

def wait_native(id,count,width=1280,height=820):
    error=None
    for _ in range(100):
        try:native(id,count,width,height);return
        except (AssertionError,subprocess.CalledProcessError) as e:error=e;time.sleep(.05)
    raise AssertionError(str(error))

try:
    start()
    a=supertab('Work A')
    navigate(a,'/set','CookieSet')
    s=navigate(a,'/one','One');one=s['aktivnaya']
    p=client.js('(()=>{let r=document.querySelector(".pole").getBoundingClientRect();return [r.x+60,r.y+185]})()')
    client.mouse(*map(int,p))
    subprocess.run(['xdotool','type','--delay','8','cef123'],env=env,check=True)
    wait_sess(a,lambda s:active(s)['zagolovok']=='Typed:cef123')
    navigate(a,'/one','One')
    client.click('.novaya-obychnaya')
    wait_sess(a,lambda s:len(s['vkladki'])==2)
    navigate(a,'/read','Cookie-A')
    s=navigate(a,'/two','Two');two=s['aktivnaya']
    client.click('.obychnaya[aria-selected="true"] .zakrepit')
    wait_sess(a,lambda s:s['vkladki'][0]['id']==two and s['vkladki'][0]['zakreplena'])
    client.click('.obychnaya[aria-selected="true"] .zakrepit')
    wait_sess(a,lambda s:not next(v for v in s['vkladki'] if v['id']==two)['zakreplena'])
    subprocess.run(['xdotool','key','Return'],env=env,check=True)
    wait_sess(a,lambda s:s['vkladki'][0]['id']==two and s['vkladki'][0]['zakreplena'])
    client.click(f'.obychnaya[data-id="{one}"] .obychnaya-imya')
    wait_sess(a,lambda s:s['aktivnaya']==one)
    p=client.js('(()=>{let r=document.querySelector(".pole").getBoundingClientRect();return [r.x+70,r.y+108]})()')
    client.mouse(*map(int,p))
    s=wait_sess(a,lambda s:len(s['vkladki'])==3 and active(s)['zagolovok']=='Popup');popup=s['aktivnaya']
    assert len(engines())==1
    wait_native(a,3)
    win=subprocess.check_output(['xdotool','search','--onlyvisible','--name','^NexusBro$'],env=env,text=True).splitlines()[0]
    subprocess.run(['xdotool','windowsize',win,'1100','700'],env=env,check=True)
    wait_native(a,3,1100,700)
    subprocess.run(['xdotool','windowsize',win,'1280','820'],env=env,check=True)
    wait_native(a,3)
    navigate(a,'/two','Two');navigate(a,'/one','One')
    client.click('.navigaciya button[title="Назад"]')
    wait_sess(a,lambda s:active(s)['zagolovok']=='Two' and active(s)['vpered'])
    client.click('.navigaciya button[title="Вперёд"]')
    wait_sess(a,lambda s:active(s)['zagolovok']=='One')
    navigate(a,'/reload','Reload1')
    client.click('.navigaciya button[title="Обновить страницу"]')
    wait_sess(a,lambda s:active(s)['zagolovok']=='Reload2')
    client.click(f'.obychnaya[data-id="{popup}"] .zakryt-obychnuyu')
    wait_sess(a,lambda s:len(s['vkladki'])==2)
    client.click(f'.obychnaya[data-id="{one}"] .obychnaya-imya')
    wait_sess(a,lambda s:s['aktivnaya']==one)
    results.append({'ordinary_create_navigate_popup_back_forward_pin_close':True,'cef_processes_for_three_browsers':1,'shared_cookie_in_supertab':True,'keyboard_in_cef_and_address_bar':True,'reload_and_unpin':True,'pin_button_keyboard':True,'native_children':3,'one_native_visible':True,'resize_all_browsers':True})
    # Новая пустая вкладка: реальный ввод БЕЗ Ctrl+A должен заменить about:blank.
    client.click('.novaya-obychnaya')
    s=wait_sess(a,lambda s:len(s['vkladki'])==3 and active(s)['adres']=='about:blank')
    blank=s['aktivnaya']
    client.wait('(()=>{let e=document.querySelector(".navigaciya input");return document.activeElement===e && e.selectionStart===0 && e.selectionEnd===e.value.length && e.value==="about:blank"})()')
    subprocess.run(['xdotool','type','--clearmodifiers','--delay','8','--',base+'/one'],env=env,check=True)
    subprocess.run(['xdotool','key','Return'],env=env,check=True)
    wait_sess(a,lambda s:active(s)['zagolovok']=='One')
    def tabmenu(tab,text):
        client.click(f'.obychnaya[data-id="{tab}"]',3)
        client.wait('!!document.querySelector(".menu-obychnyh")')
        client.text_button(text)
        client.wait('!document.querySelector(".menu-obychnyh")')
    tabmenu(blank,'Закрыть вкладку')
    wait_sess(a,lambda s:len(s['vkladki'])==2)
    tabmenu(one,'Дублировать')
    s=wait_sess(a,lambda s:len(s['vkladki'])==3 and active(s)['zagolovok']=='One')
    duplicate=s['aktivnaya']
    tabmenu(duplicate,'Закрепить')
    wait_sess(a,lambda s:next(v for v in s['vkladki'] if v['id']==duplicate)['zakreplena'])
    tabmenu(duplicate,'Открепить')
    wait_sess(a,lambda s:not next(v for v in s['vkladki'] if v['id']==duplicate)['zakreplena'])
    tabmenu(duplicate,'Закрыть вкладку')
    wait_sess(a,lambda s:len(s['vkladki'])==2)
    tabmenu(one,'Дублировать')
    wait_sess(a,lambda s:len(s['vkladki'])==3)
    tabmenu(one,'Закрыть вкладки справа')
    wait_sess(a,lambda s:len(s['vkladki'])==2 and any(v['id']==two and v['zakreplena'] for v in s['vkladki']))
    tabmenu(one,'Дублировать')
    wait_sess(a,lambda s:len(s['vkladki'])==3)
    tabmenu(one,'Закрыть другие вкладки')
    wait_sess(a,lambda s:len(s['vkladki'])==2 and any(v['id']==two and v['zakreplena'] for v in s['vkladki']))
    client.click(f'.obychnaya[data-id="{one}"] .obychnaya-imya')
    wait_sess(a,lambda s:s['aktivnaya']==one)
    client.click(f'.obychnaya[data-id="{one}"]',3)
    client.wait('!!document.querySelector(".menu-obychnyh")')
    subprocess.run(['xdotool','key','Escape'],env=env,check=True)
    client.wait('!document.querySelector(".menu-obychnyh")')
    navigate(a,'/reload','Reload3');tabmenu(one,'Обновить')
    wait_sess(a,lambda s:active(s)['zagolovok']=='Reload4')
    navigate(a,'/one','One')
    # Сворачивание и физическое перетягивание разделителя, включая нативный CEF.
    client.click('.svernut-panel')
    client.wait('document.querySelector(".reika").getBoundingClientRect().width===52')
    wait_native(a,2)
    client.click('.svernut-panel')
    client.wait('!!document.querySelector(".razdelitel-paneli")')
    point=client.js('(()=>{let r=document.querySelector(".razdelitel-paneli").getBoundingClientRect();return [Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)]})()')
    subprocess.run(['xdotool','mousemove',str(point[0]),str(point[1]),'mousedown','1','mousemove','310',str(point[1]),'sleep','0.2','mouseup','1'],env=env,check=True)
    client.wait('document.querySelector(".reika").getBoundingClientRect().width===310')
    wait_native(a,2)
    # Настройки сохраняются на диске и управляют новой вкладкой.
    client.click('button[title="Настройки NexusBro"]')
    client.wait('!!document.querySelector(".nastrojki-okno")')
    client.click('.nastrojki-okno select')
    subprocess.run(['xdotool','key','Home','Down','Return'],env=env,check=True)
    client.fill('.nastrojki-okno input:not([type])','javascript:alert(1)')
    client.text_button('Сохранить')
    client.wait('!!document.querySelector(".nastrojki-okno [role=alert]")')
    assert json.loads((run/'data/NexusBro/nastrojki.json').read_text())['novaya_stranica']=='about:blank'
    client.fill('.nastrojki-okno input:not([type])',base+'/one')
    client.click('.nastrojki-okno input[type=checkbox]')
    client.text_button('Сохранить')
    client.wait('!document.querySelector(".nastrojki-okno") && document.documentElement.dataset.tema==="svetlaya"')
    config=run/'data/NexusBro/nastrojki.json'
    settings=json.loads(config.read_text())
    assert settings['panel_shirina']==310 and settings['tema']=='svetlaya' and not settings['avtoobnovleniya'],settings
    client.click('.novaya-obychnaya')
    s=wait_sess(a,lambda s:len(s['vkladki'])==3 and active(s)['zagolovok']=='One')
    tabmenu(s['aktivnaya'],'Закрыть вкладку')
    wait_sess(a,lambda s:len(s['vkladki'])==2)
    client.click(f'.obychnaya[data-id="{one}"] .obychnaya-imya')
    wait_sess(a,lambda s:s['aktivnaya']==one)
    wait_native(a,2)
    # Изображение страницы, а не только заголовок: проверка пикселя нативного CEF.
    shot=run/'paint.png'
    subprocess.run(['ffmpeg','-y','-f','x11grab','-video_size','1280x900','-i',args.display,'-frames:v','1',str(shot)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True)
    from PIL import Image
    point=client.js('(()=>{let r=document.querySelector(".pole").getBoundingClientRect();return [Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)]})()')
    pixel=Image.open(shot).convert('RGB').getpixel(tuple(point))
    assert pixel==(24,85,65),pixel
    results.append({'blank_address_selected_and_replaced_without_ctrl_a':True,'tab_context_duplicate_pin_unpin_reload_close_other_right':True,'bulk_close_preserves_pinned':True,'context_escape_restores_cef':True,'sidebar_collapsed_width':52,'sidebar_drag_width':310,'settings_saved_theme_home_update_checks':True,'new_tab_uses_configured_home':True,'native_page_pixel_rgb':pixel})
    b=supertab('Work B');navigate(b,'/read','Cookie-none')
    assert len(engines())==2
    client.click('.obychnaya .zakryt-obychnuyu')
    wait_sess(b,lambda s:len(s['vkladki'])==1 and active(s)['adres']=='about:blank')
    client.click('.vkladka:first-child')
    client.wait('document.querySelector(".aktivnaya-imya").textContent==="Work A"')
    s=wait_sess(a,lambda s:len(s['vkladki'])==2 and s['aktivnaya']==one)
    client.click('.vkladka:first-child',3)
    client.wait('!!document.querySelector(".stroka-menyu")')
    client.text_button('Сохранить и закрыть')
    client.wait('document.querySelectorAll(".vkladka").length===1')
    assert len(engines())==1
    client.click('.niz button:nth-child(2)');client.wait('!!document.querySelector(".stroka-menyu")')
    client.click('.stroka-menyu');client.wait('document.querySelectorAll(".vkladka").length===2')
    s=wait_sess(a,lambda s:len(s['vkladki'])==2 and s['aktivnaya']==one and s['vkladki'][0]['id']==two and s['vkladki'][0]['zakreplena'])
    client.wait('document.querySelectorAll(".obychnaya").length===2')
    results.append({'supertab_profile_isolation':True,'last_closed_tab_becomes_blank':True,'restore_saved_supertab_with_two_tabs':True})
    # Проверяем очередь команд при замороженном CEF без создания новых браузеров.
    profile=run/'data/NexusBro/profili'/a
    pid=next(p for p in engines() if str(profile).encode() in pathlib.Path(f'/proc/{p}/cmdline').read_bytes())
    os.kill(pid,signal.SIGSTOP)
    client.js('window.__lags=[];window.__last=performance.now();window.__clock=setInterval(()=>{let n=performance.now();window.__lags.push(n-window.__last);window.__last=n},20);true')
    started=time.monotonic()
    for i in range(25):client.command('obychnaya_komanda',id=a,komanda={'deystvie':'vybrat','id':two if i%2 else one})
    assert client.command('spisok')
    max_lag=client.js('Math.max(...window.__lags)')
    assert max_lag<1000,max_lag
    os.kill(pid,signal.SIGCONT)
    wait_sess(a,lambda s:s['aktivnaya']==one)
    results.append({'commands_while_cef_stopped':25,'max_ui_interval_ms':max_lag,'no_extra_engine':len(engines())==2})
    subprocess.run(['ffmpeg','-y','-f','x11grab','-video_size','1280x900','-i',args.display,'-frames:v','1',str(run/'ordinary.png')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True)
    stop();assert not engines()
    start()
    client.wait('document.querySelectorAll(".obychnaya").length===2')
    s=wait_sess(a,lambda s:len(s['vkladki'])==2 and s['aktivnaya']==one and active(s)['zagolovok']=='One')
    assert s['vkladki'][0]['id']==two and s['vkladki'][0]['zakreplena']
    assert len(engines())==1,engines()
    wait_native(a,2)
    client.wait('document.documentElement.dataset.tema==="svetlaya" && document.querySelector(".reika").getBoundingClientRect().width===310')
    results.append({'settings_and_sidebar_survive_restart':True})
    assert len(sess(b)['vkladki'])==1
    results.append({'app_restart_restores_urls_order_pin_active':True,'only_active_supertab_autostarts':True})
    stop();assert not engines()
    results.append({'success':True,'live_engines_after_exit':0})
finally:
    if app and app.poll() is None and client:
        try:
            (run/'dom.txt').write_text(client.js('document.body.innerText'))
            subprocess.run(['ffmpeg','-y','-f','x11grab','-video_size','1280x900','-i',args.display,'-frames:v','1',str(run/'last.png')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        except Exception:pass
    if app and app.poll() is None:
        app.terminate();app.wait(timeout=5)
    server.shutdown()
    pathlib.Path(args.out).write_text(json.dumps(results,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps(results,ensure_ascii=False,indent=2),flush=True)
    print('Локальные журналы:',run.name,flush=True)
