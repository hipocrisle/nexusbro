import json
import os
import subprocess
import time
import re
import urllib.request
import websocket

class Client:
    def __init__(self, display=':94', port=9223):
        self.display = display
        index=urllib.request.urlopen(f'http://127.0.0.1:{port}',timeout=2).read().decode()
        route=re.search(r'/socket/\d+/\d+/WebPage',index)
        if not route:raise ConnectionError('WebKit ещё не создал страницу')
        self.ws = websocket.create_connection(f'ws://127.0.0.1:{port}'+route[0], timeout=5)
        self.target = 'page-7'
        self.seq = 0
        self.ws.settimeout(.3)
        try:
            event = json.loads(self.ws.recv())
            if event.get('method') == 'Target.targetCreated':
                self.target = event['params']['targetInfo']['targetId']
        except websocket.WebSocketTimeoutException:
            pass
        self.ws.settimeout(5)

    def js(self, expression):
        self.seq += 1
        request = self.seq
        self.ws.send(json.dumps({'id':request, 'method':'Target.sendMessageToTarget', 'params':{
            'targetId':self.target, 'message':json.dumps({'id':request, 'method':'Runtime.evaluate',
            'params':{'expression':expression, 'returnByValue':True}})}}))
        while True:
            message = json.loads(self.ws.recv())
            if message.get('error'):
                raise RuntimeError(message)
            if message.get('method') != 'Target.dispatchMessageFromTarget':
                continue
            response = json.loads(message['params']['message'])
            if response.get('id') != request:
                continue
            if response.get('error') or response.get('result', {}).get('wasThrown'):
                raise RuntimeError(response)
            return response['result']['result'].get('value')

    def wait(self, expression, timeout=10):
        start = time.monotonic()
        while time.monotonic()-start < timeout:
            result = self.js(expression)
            if result:
                return result
            time.sleep(.025)
        raise AssertionError(f'{expression}; DOM={self.js("document.body.innerText")}')

    def command(self, name, **params):
        self.js('window.__guiResult=null;window.__TAURI_INTERNALS__.invoke('+json.dumps(name)+','+json.dumps(params)+
                ').then(r=>window.__guiResult={r},e=>window.__guiResult={error:String(e)});true')
        r = self.wait('window.__guiResult')
        assert 'error' not in r, r
        if isinstance(r['r'],dict) and 'ok' in r['r']:
            assert r['r']['ok'],r
        return r['r']

    def mouse(self, x, y, button=1):
        subprocess.run(['xdotool','mousemove',str(x),str(y),'click',str(button)],
            env={**os.environ,'DISPLAY':self.display},check=True)

    def click(self, selector, button=1):
        p = self.js('(()=>{let e=document.querySelector('+json.dumps(selector)+');if(!e)return null;let r=e.getBoundingClientRect();return [Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)]})()')
        assert p,selector
        self.mouse(*p,button)

    def text_button(self, text):
        self.js('window.__guiButton=Array.from(document.querySelectorAll("button,.stroka-menyu")).find(x=>x.textContent.trim()==='+json.dumps(text)+');true')
        p = self.js('(()=>{let r=window.__guiButton.getBoundingClientRect();return [Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)]})()')
        self.mouse(*p)

    def fill(self, selector, text, enter=False):
        self.click(selector)
        env={**os.environ,'DISPLAY':self.display}
        subprocess.run(['xdotool','key','ctrl+a'],env=env,check=True)
        subprocess.run(['xdotool','type','--clearmodifiers','--delay','8','--',text],env=env,check=True)
        self.wait('document.querySelector('+json.dumps(selector)+').value==='+json.dumps(text),timeout=3)
        if enter:subprocess.run(['xdotool','key','Return'],env=env,check=True)

    def close(self):
        self.ws.close()
