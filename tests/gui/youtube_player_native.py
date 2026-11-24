#!/usr/bin/env python3
"""GHA-only real CEF controlled dual-WebM playback, NOT YouTube extraction.

Only packaged native commands admit the fixed fixture document. Playback actions
are physical mouse clicks. Public output contains numeric receipts, never URLs,
headers, certificate material, process arguments or raw native errors.
"""
import argparse
import datetime
import http.server
import json
import os
import pathlib
import select
import shutil
import socket
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

from security_native import Device, call, js_wait, painted, require, super_record, wait_for
from tab_routes import WindowsDevice, switch_super, rows, click_control
from resource_soak import foreground
from hibernation_cookie_probe import WindowsFixtureTrust, tls_proxy_preflight

HOST = 'fixture.googlevideo.com'


def certificates(root):
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import NameOID, ExtendedKeyUsageOID
    root.mkdir()
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    leafkey = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'Disposable player fixture CA')])
    now = datetime.datetime.now(datetime.timezone.utc)
    def builder(subject, public):
        return (x509.CertificateBuilder().subject_name(subject).issuer_name(name).public_key(public)
                .serial_number(x509.random_serial_number()).not_valid_before(now-datetime.timedelta(minutes=2))
                .not_valid_after(now+datetime.timedelta(days=1)))
    ca = (builder(name, key.public_key()).add_extension(x509.BasicConstraints(ca=True, path_length=0), True)
          .add_extension(x509.KeyUsage(False,False,False,False,False,True,True,False,False),True)
          .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()),False).sign(key, hashes.SHA256()))
    leaf = (builder(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, HOST)]), leafkey.public_key())
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), True)
            .add_extension(x509.SubjectAlternativeName([x509.DNSName(HOST)]), False)
            .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), False)
            .add_extension(x509.KeyUsage(True,False,True,False,False,False,False,False,False),True)
            .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(key.public_key()),False).sign(key, hashes.SHA256()))
    for filename, data in [('ca.crt', ca.public_bytes(serialization.Encoding.PEM)),
                           ('server.crt', leaf.public_bytes(serialization.Encoding.PEM)),
                           ('server.key', leafkey.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))]:
        target = root / filename
        with target.open('xb') as stream: stream.write(data)
        target.chmod(0o600)
    return ca.fingerprint(hashes.SHA1()).hex().upper()


class Trust:
    def __init__(self, root, cert, thumb):
        self.root, self.cert, self.thumb = root, cert, thumb
        self.oldhome = os.environ.get('HOME'); self.installed = False; self.windows = None
    def install(self):
        if sys.platform == 'win32':
            self.windows = WindowsFixtureTrust(self.root,self.cert,self.cert.parent/'server.crt')
            self.windows.install()
        else:
            home = self.root / 'home'; database = home / '.pki' / 'nssdb'; database.mkdir(parents=True)
            for args in (['-N','--empty-password'], ['-A','-n','Disposable player CA','-t','C,,','-i',str(self.cert)]):
                subprocess.run(['certutil','-d','sql:'+str(database),*args],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=15)
            os.environ['HOME'] = str(home)
        self.installed = True
    def close(self):
        if self.windows:
            self.windows.close()
        elif self.oldhome is None: os.environ.pop('HOME',None)
        else: os.environ['HOME'] = self.oldhome


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = False


class Fixture:
    def __init__(self, cert, key, video, audio):
        self.lock = threading.Lock(); self.peers = {}; self.receipts = []; self.connects = []; self.direct = 0
        self.servers = []; self.sockets = set(); self.threads = []
        self.payload = {'video': video.read_bytes(), 'audio': audio.read_bytes()}
        require(all(0 < len(v) <= 32*1024*1024 for v in self.payload.values()), 'Media fixture size invalid')
        owner = self
        class Origin(http.server.BaseHTTPRequestHandler):
            def log_message(self,*_): pass
            def do_GET(self):
                path = urllib.parse.urlsplit(self.path); track = urllib.parse.parse_qs(path.query).get('track',[''])[0]
                if path.path != '/videoplayback' or track not in owner.payload:
                    self.send_error(404); return
                body = owner.payload[track]; start = 0; end = len(body)-1; ranged = False
                header = self.headers.get('Range')
                if header:
                    import re
                    match = re.fullmatch(r'bytes=(\d+)-(\d*)',header)
                    if not match: self.send_error(416); return
                    start = int(match[1]); end = min(int(match[2]) if match[2] else end,end); ranged=True
                    if start > end: self.send_error(416); return
                with owner.lock:
                    owner.receipts.append({'lane':owner.peers.get(self.client_address[1],'Direct'),'track':track,'range':ranged,'bytes':end-start+1})
                self.send_response(206 if ranged else 200)
                self.send_header('Content-Type','video/webm' if track=='video' else 'audio/webm')
                self.send_header('Accept-Ranges','bytes'); self.send_header('Cache-Control','no-store')
                self.send_header('Content-Length',str(end-start+1))
                if ranged:self.send_header('Content-Range',f'bytes {start}-{end}/{len(body)}')
                self.end_headers()
                try:self.wfile.write(body[start:end+1])
                except (OSError,ssl.SSLError):pass
        origin = http.server.ThreadingHTTPServer(('127.0.0.1',0),Origin); origin.daemon_threads=True
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); tls.load_cert_chain(cert,key)
        origin.socket=tls.wrap_socket(origin.socket,server_side=True); self.origin_port=origin.server_port; self.start(origin)
        self.proxies={}
        for lane in ('A','B'):
            class Proxy(socketserver.StreamRequestHandler):
                def handle(self):
                    self.connection.settimeout(10)
                    try:
                        first=self.rfile.readline(1025)
                        if first != ('CONNECT '+HOST+':443 HTTP/1.1\r\n').encode():return
                        total=0
                        while True:
                            line=self.rfile.readline(2049); total+=len(line)
                            if not line or total>8192:return
                            if line==b'\r\n':break
                        upstream=socket.create_connection(('127.0.0.1',origin.server_port),5)
                        with owner.lock:
                            owner.peers[upstream.getsockname()[1]]=self.server.lane
                            owner.connects.append(self.server.lane);owner.sockets.update((upstream,self.connection))
                        try:
                            self.wfile.write(b'HTTP/1.1 200 Connection Established\r\n\r\n');self.wfile.flush()
                            while True:
                                ready,_,_=select.select([self.connection,upstream],[],[],1)
                                for source in ready:
                                    data=source.recv(65536)
                                    if not data:return
                                    (upstream if source is self.connection else self.connection).sendall(data)
                        finally:
                            with owner.lock:owner.peers.pop(upstream.getsockname()[1],None);owner.sockets.discard(upstream);owner.sockets.discard(self.connection)
                            upstream.close()
                    except OSError:pass
            server=Server(('127.0.0.1',0),Proxy);server.lane=lane;self.proxies[lane]=server.server_address[1];self.start(server)
        class Trap(socketserver.BaseRequestHandler):
            def handle(self):
                with owner.lock:owner.direct+=1
        # Never take another process' port or modify host networking policy.
        trap=Server(('127.0.0.1',443),Trap);self.start(trap)
    def start(self,server):
        self.servers.append(server);thread=threading.Thread(target=server.serve_forever,daemon=True);self.threads.append(thread);thread.start()
    def close(self):
        with self.lock:
            for sock in list(self.sockets):
                try:sock.shutdown(socket.SHUT_RDWR)
                except OSError:pass
        for server in self.servers:server.shutdown();server.server_close()
        for thread in self.threads:thread.join(timeout=3)


def linux_fixture_ack(client, identifier, tab, diagnostics):
    """Admission receipt only; configured and physical playback remain mandatory."""
    receipt = {'received': False, 'accepted': False}
    diagnostics['fixture_admission'] = receipt
    expression = ('window.__playerFixtureResults.find(x=>x.super_id==='
                  + json.dumps(identifier) + '&&x.data?.id===' + str(tab) + ')')
    value = wait_for(lambda: client.js(expression), 'Native fixture admission acknowledgment missing', 10)
    error = value.get('data', {}).get('error')
    receipt.update(received=True, accepted=error is None)
    # Unknown native values are never published (including raw URLs/errors).
    errors = {
        'Вкладка недоступна': 'browser_missing',
        'Диагностика доступна только в CI': 'ci_only',
        'Диагностика недоступна': 'fixture_invalid',
        'Диагностические дорожки отклонены': 'tracks_rejected',
        'Нет подходящих WebM дорожек для лёгкого плеера': 'tracks_unsupported',
        'Контекст вкладки недоступен': 'context_missing',
        'Не удалось подготовить внутренний плеер': 'scheme_install_failed',
        'Не удалось открыть внутренний плеер': 'grant_failed',
        'Плеер занят': 'grant_busy', 'Плеер закрыт': 'grant_closed',
        'Плеер уже закрыт': 'grant_revoked',
        'Не удалось подготовить видео': 'config_failed',
        'Документ вкладки недоступен': 'frame_missing',
    }
    receipt['code'] = ('accepted' if error is None else
                       errors.get(error, 'unknown_native_error') if isinstance(error, str)
                       else 'invalid_native_error')
    current = rows(client, identifier)
    selected = next((r for r in current.get('vkladki', []) if r.get('id') == tab), {})
    receipt['tab'] = {
        'present': bool(selected), 'selected': current.get('aktivnaya') == tab,
        'native_browser': isinstance(selected.get('browser_id'), int) and selected['browser_id'] > 0,
        'loading': selected.get('zagruzka') is True,
        'deferred': selected.get('otlozhena') is True,
    }
    require(error is None, 'Native fixture admission refused')


def observe(client, identifier, tab, *, admitting=False, diagnostics=None):
    client.js('window.__playerObserved=[];true')
    call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_snapshot','id':tab})
    expression='window.__playerObserved.find(x=>x.super_id==='+json.dumps(identifier)+'&&x.data?.id==='+str(tab)+')'
    value=wait_for(lambda:client.js(expression),'Native player observation missing',10)
    if diagnostics is not None:diagnostics['native_probe_unavailable']='state' not in value.get('data',{})
    if diagnostics is not None and sys.platform.startswith('linux'):
        code = value.get('data', {}).get('code')
        allowed = {'probe_wrong_thread', 'browser_invalid', 'frame_absent',
                   'document_not_admitted', 'message_capacity', 'observer_unavailable',
                   'dispatch_failed', 'timeout', 'document_changed', 'cdp_method_failed',
                   'cdp_result_absent', 'snapshot_value_unavailable', 'cdp_agent_detached'}
        if code is not None:
            diagnostics['native_probe_code'] = code if isinstance(code, str) and code in allowed else 'unknown_probe_code'

    if admitting and isinstance(value.get('data',{}).get('error'),str) and 'state' not in value['data']:
        return {}  # only pre-admission: old document may still be navigating
    require(isinstance(value.get('data',{}).get('state'),dict),'Native observation refused')
    state=value['data']['state']
    if diagnostics is not None:
        fields={'configured','disposed','playing','separate','position','audio_position','duration','skew','decoded','dropped','video_width','video_height'}
        diagnostics['last_snapshot']={key:v for key,v in state.items() if key in fields and type(v) in (bool,int,float)}
    return state


def physical(client, device, kind, diagnostics):
    foreground(device);painted(client)
    # Actual CEF controls are laid out by shipped CSS: main16px padding,
    # final24px status +12px gap +button-height39px. No DOM CEF mutation.
    point=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height}})()')
    require(point['w']>=700 and point['h']>=250,'Native player viewport too small')
    x=point['x']+(65 if kind=='play' else point['w']*.45)
    y=point['y']+point['h']-65
    from PIL import ImageGrab
    label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    origin=call(client,'plugin:window|inner_position',label=label)
    scale=client.js('devicePixelRatio')
    # Read the button's left padding (away from text), at the same row as
    # the one physical click. Shell RAF alone is not CEF compositor proof.
    pixel=(round(origin['x']+(point['x']+23)*scale),round(origin['y']+y*scale))
    diagnostics['control']={'kind':kind,'viewport_width':round(point['w']),'viewport_height':round(point['h']),
                            'relative_click_x':round(x-point['x']),'relative_click_y':round(y-point['y']),
                            'scale':scale,'pixel_in_screen':False,'pixel_matches':False,'clicked':False}
    def controls_painted():
        with ImageGrab.grab(**({'xdisplay':device.display} if sys.platform!='win32' else {})) as image:
            inside=0<=pixel[0]<image.width and 0<=pixel[1]<image.height
            rgb=list(image.getpixel(pixel)[:3]) if inside else []
            matches=tuple(rgb) in ((41,43,59),(54,56,77))
            diagnostics['control'].update(pixel_in_screen=inside,pixel_rgb=rgb,pixel_matches=matches)
            return inside and matches
    wait_for(controls_painted,'Native player controls not physically painted',15)
    client.mouse(x,y)
    diagnostics['control']['clicked']=True


def video_paint(client, device):
    from PIL import ImageGrab
    foreground(device)
    rect=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return [r.x+r.width*.25,r.y+r.height*.3,r.x+r.width*.75,r.y+r.height*.6,devicePixelRatio]})()')
    label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    origin=call(client,'plugin:window|inner_position',label=label)
    box=tuple(round((origin['x'] if i%2==0 else origin['y'])+rect[i]*rect[4]) for i in range(4))
    with ImageGrab.grab(**({'xdisplay':device.display} if sys.platform!='win32' else {})) as image:
        require(box[0]>=0 and box[1]>=0 and box[2]<=image.width and box[3]<=image.height,'Physical media paint outside screen')
        return image.crop(box).resize((32,18)).convert('RGB').tobytes()


def run(args):
    require(os.environ.get('GITHUB_ACTIONS')=='true','Disposable GHA runner required')
    out=pathlib.Path(args.out).resolve();out.mkdir(parents=True,exist_ok=False)
    report={'schema':1,'completed':False,'extractor_acceptance':False,'checks':[],'stage':'setup','player_diagnostics':{}}
    code=1;device=fixture=trust=None;private_root=None
    old=os.environ.get('NEXUSBRO_TEST_YOUTUBE_DNS_GUARD')
    try:
        private_root=pathlib.Path(tempfile.mkdtemp(prefix='nb-player-'))
        if private_root:
            root=private_root;thumb=certificates(root/'tls')
            trust=Trust(root,root/'tls'/'ca.crt',thumb);trust.install()
            fixture=Fixture(root/'tls'/'server.crt',root/'tls'/'server.key',pathlib.Path(args.video_webm),pathlib.Path(args.audio_webm))
            report['tls_handshake_preflight']=tls_proxy_preflight(fixture.proxies['A'],HOST,443,root/'tls'/'ca.crt',root/'tls'/'server.crt')
            exe,cef=pathlib.Path(args.exe).resolve(),pathlib.Path(args.cef).resolve()
            device=WindowsDevice(root/'device',exe,cef) if sys.platform=='win32' else Device('player',root/'device',exe,cef)
            definitions=[]
            for lane in ('A','B'):
                identifier='player-'+lane.lower();item=super_record(identifier,'Player '+lane,'about:blank',proxy='http://127.0.0.1:'+str(fixture.proxies[lane]));item['poryadok']=len(definitions);definitions.append(item)
                folder=device.data/'profili'/identifier;folder.mkdir(parents=True)
                (folder/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':'about:blank','zagolovok':'Fixture','otlozhena':True}],'aktivnaya':1,'sleduyushchaya':2}))
            for filename in ('vkladki.json','sohranennye.json'):(device.data/filename).write_text(json.dumps(definitions))
            (device.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False}))
            os.environ['NEXUSBRO_TEST_YOUTUBE_DNS_GUARD']='1'
            client=device.start();foreground(device)
            client.js('window.__playerObserved=[];window.__playerListenerReady=false;window.__playerHandler=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-state")window.__playerObserved.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:window.__playerHandler}).then(()=>window.__playerListenerReady=true);true')
            js_wait(client,'window.__playerListenerReady','Actual native event listener missing')
            if sys.platform.startswith('linux'):
                client.js('window.__playerFixtureResults=[];window.__playerFixtureListenerReady=false;window.__playerFixtureHandler=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-result")window.__playerFixtureResults.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:window.__playerFixtureHandler}).then(()=>window.__playerFixtureListenerReady=true);true')
                js_wait(client,'window.__playerFixtureListenerReady','Actual native fixture listener missing')
            for lane in ('A','B'):
                identifier='player-'+lane.lower();report['stage']='load-'+lane
                foreground(device);switch_super(client,identifier)
                if client.js('!!document.querySelector(".pusto button")'):click_control(client,'.pusto button',20)
                session=wait_for(lambda:rows(client,identifier) if any(r.get('browser_id',0)>0 and not r.get('zagruzka') for r in rows(client,identifier)['vkladki']) else None,'Actual native browser not loaded',40)
                tab=session['aktivnaya']
                if sys.platform.startswith('linux'):client.js('window.__playerFixtureResults=[];true')
                call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_fixture','id':tab})
                if sys.platform.startswith('linux'):linux_fixture_ack(client,identifier,tab,report['player_diagnostics'])
                initial=wait_for(lambda:(lambda s:s if s.get('configured') and not s.get('disposed') else None)(observe(client,identifier,tab,admitting=True,diagnostics=report['player_diagnostics'])),'Player was not configured',30)
                report['stage']='physical-play-'+lane;physical(client,device,'play',report['player_diagnostics'])
                first=wait_for(lambda:(lambda s:s if s.get('playing') and s.get('decoded',0)>=3 else None)(observe(client,identifier,tab,diagnostics=report['player_diagnostics'])),'Physical play did not decode actual media',25)
                require(first['video_width']==320 and first['video_height']==180 and first['separate'],'Actual VP9/Opus geometry differs')
                paint_before=video_paint(client,device)
                time.sleep(3);second=observe(client,identifier,tab,diagnostics=report['player_diagnostics'])
                paint_after=video_paint(client,device)
                require(sum(a!=b for a,b in zip(paint_before,paint_after))>30,'Actual OS video paint did not change')
                require(second['position']>first['position']+1 and second['audio_position']>first['audio_position']+1 and second['skew']<1,'Dual media clocks failed')
                report['stage']='physical-pause-'+lane;physical(client,device,'play',report['player_diagnostics']);time.sleep(.5);paused=observe(client,identifier,tab,diagnostics=report['player_diagnostics']);time.sleep(1);still=observe(client,identifier,tab,diagnostics=report['player_diagnostics'])
                require(not paused['playing'] and abs(still['position']-paused['position'])<.35,'Physical pause failed')
                report['stage']='physical-seek-'+lane;physical(client,device,'seek',report['player_diagnostics']);time.sleep(1);seek=observe(client,identifier,tab,diagnostics=report['player_diagnostics'])
                require(abs(seek['position']-still['position'])>5,'Physical seek did not move actual native media')
                with fixture.lock:
                    tracks={r['track'] for r in fixture.receipts if r['lane']==lane};direct=fixture.direct
                    range_count=sum(r['range'] for r in fixture.receipts if r['lane']==lane)
                require(tracks=={'video','audio'} and direct==0,'Media bypassed actual selected route')
                report['checks'].append({'name':'actual-dual-webm-'+lane,'decoded':second['decoded'],'width':second['video_width'],'height':second['video_height'],'skew':second['skew'],'range_requests':range_count,'physical_pause':True,'physical_seek':True,'actual_os_paint_changed':True,'direct_connections':direct})
            report['completed']=True;report['stage']='complete';code=0
    except Exception as error:
        report['failure_class']=type(error).__name__
        codes={'Native fixture admission acknowledgment missing':'fixture_ack_missing',
               'Native fixture admission refused':'fixture_admission_refused',
               'Actual native fixture listener missing':'fixture_listener_missing',
               'Native player controls not physically painted':'controls_not_painted',
               'Physical play did not decode actual media':'playback_not_decoding',
               'Native player observation missing':'snapshot_missing','Native observation refused':'snapshot_refused',
               'Player was not configured':'not_configured','Dual media clocks failed':'clocks_not_advancing',
               'Physical pause failed':'pause_failed','Physical seek did not move actual native media':'seek_failed',
               'Actual VP9/Opus geometry differs':'media_geometry','Actual OS video paint did not change':'video_paint_static',
               'Media bypassed actual selected route':'route_witness_missing','Native player viewport too small':'viewport_too_small',
               'windows_fixture_chain_not_trusted':'windows_chain_untrusted',
               'windows_chain_operation_failed':'windows_chain_operation_failed'}
        report['failure_code']=codes.get(str(error),'unclassified')
    finally:
        if trust and trust.windows:report['windows_chain_preflight']=trust.windows.receipts
        cleanup_ok=True
        for owned in (device,fixture,trust):
            if owned:
                try:owned.close()
                except Exception:cleanup_ok=False
        report['cleanup_completed']=cleanup_ok
        if not cleanup_ok:report['completed']=False;code=1
        if private_root:shutil.rmtree(private_root,ignore_errors=True)
        if old is None:os.environ.pop('NEXUSBRO_TEST_YOUTUBE_DNS_GUARD',None)
        else:os.environ['NEXUSBRO_TEST_YOUTUBE_DNS_GUARD']=old
        (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    return code


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('exe','cef','out','video-webm','audio-webm'):parser.add_argument('--'+name,required=True)
    sys.exit(run(parser.parse_args()))
