"""Loopback-only network witnesses for native per-tab route acceptance.

Receipts originate from the HTTP server, never from test-created success flags.
TLS is terminated only at the origin; CONNECT proxies forward encrypted bytes.
"""
import http.client
import http.server
import json
import secrets
import select
import socket
import ssl
import threading
import time
import urllib.parse


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True


class Fixture:
    def __init__(self, cert, key, stun_uri=None):
        self.lock = threading.Lock()
        self.receipts, self.tunnels, self.peers = [], [], {}
        self.tokens = {lane: secrets.token_hex(20) for lane in ('A', 'B', 'Direct', 'Historical', 'Neighbor')}
        self.worker_token = secrets.token_hex(24)
        self.stun_uri = stun_uri
        self.servers = []
        self.loading_started = threading.Event()
        self._loading_release = threading.Event()
        self.proxy_servers, self.proxy_sockets, self.stopped_proxies = {}, {}, set()
        owner = self

        class Origin(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                parsed = urllib.parse.urlsplit(self.path)
                q = urllib.parse.parse_qs(parsed.query)
                lane = self.headers.get('X-Fixture-Route')
                if not lane:
                    with owner.lock:
                        lane = owner.peers.get(self.client_address[1], 'Direct')
                cookie = self.headers.get('Cookie', '')
                # Never retain or serialize the authentication token itself.
                auth = next((name for name, token in owner.tokens.items()
                             if 'route_auth=' + token in cookie), None)
                with owner.lock:
                    owner.receipts.append({'path': parsed.path, 'case': q.get('case', [None])[0], 'lane': lane,
                                           'auth': auth, 'storage': q.get('storage', [None])[0],
                                           'rtc_event': q.get('event', [None])[0],
                                           'candidate_type': q.get('type', [None])[0],
                                           'candidate_address': q.get('address', [None])[0],
                                           'worker_proof': secrets.compare_digest(self.headers.get('X-Fixture-Worker', ''), owner.worker_token),
                                           'worker_existing': q.get('existing', [None])[0],
                                           'worker_event': q.get('event', [None])[0],
                                           'secure': q.get('secure', [None])[0],
                                           'time': time.monotonic(),
                                           'tls': isinstance(self.connection, ssl.SSLSocket)})
                if parsed.path == '/route-pending' and q.get('case') == ['route-loading']:
                    # Receipt (including actual lane/auth) is recorded above,
                    # before this real subresource keeps CEF's load unfinished.
                    owner.loading_started.set()
                    owner._loading_release.wait(30)
                    body = b'<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>'
                    try:
                        self.send_response(200)
                        self.send_header('Content-Type', 'image/svg+xml')
                        self.send_header('Cache-Control', 'no-store')
                        self.send_header('Content-Length', str(len(body)))
                        self.end_headers()
                        self.wfile.write(body)
                    except (BrokenPipeError, ConnectionResetError):
                        # A manual route restart legitimately aborts the old
                        # browser's still-pending request before release.
                        pass
                    return
                if parsed.path.startswith('/worker/'):
                    if parsed.path == '/worker/sw.js':
                        body = owner.worker_script().encode()
                        kind = 'application/javascript'
                    elif parsed.path == '/worker/page':
                        body = ("<!doctype html><title>Worker route fixture</title><p>Owned worker route</p><script>"
                                "(async()=>{const q=new URLSearchParams(location.search);"
                                "let reg=await navigator.serviceWorker.getRegistration('/worker/');const existing=!!reg?.active;"
                                "if(!reg)reg=await navigator.serviceWorker.register('/worker/sw.js',{scope:'/worker/'});"
                                "const ready=await navigator.serviceWorker.ready;"
                                "await fetch('/worker/page-status?'+new URLSearchParams({case:q.get('case'),existing:String(existing),secure:String(isSecureContext)}),{cache:'no-store'});"
                                "ready.active.postMessage({case:q.get('case')});"
                                "})().catch(()=>fetch('/worker/page-error?'+new URLSearchParams(location.search),{cache:'no-store'}));</script>").encode()
                        kind = 'text/html; charset=utf-8'
                    elif parsed.path in ('/worker/receipt', '/worker/page-status', '/worker/page-error'):
                        body, kind = b'worker receipt', 'text/plain'
                    else:
                        self.send_error(404)
                        return
                    self.send_response(200)
                    self.send_header('Content-Type', kind)
                    self.send_header('Cache-Control', 'no-store')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                seed = q.get('seed', [None])[0]
                if seed and seed not in owner.tokens:
                    self.send_error(400)
                    return
                body = owner.page_html(q.get('case', [None])[0])
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Cache-Control', 'no-store')
                if seed:
                    self.send_header('Set-Cookie', 'route_auth=' + owner.tokens[seed] + '; HttpOnly; SameSite=Lax; Path=/'
                                     + ('; Max-Age=86400' if q.get('persist') == ['1'] else ''))
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        plain = Server(('127.0.0.1', 0), Origin)
        tls = Server(('127.0.0.1', 0), Origin)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        tls.socket = context.wrap_socket(tls.socket, server_side=True)
        self.ports = {plain.server_port, tls.server_port}
        self.http, self.https = plain.server_port, tls.server_port
        self.servers.extend((plain, tls))
        self.proxies = {}
        for lane in ('A', 'B'):
            self.proxies[lane] = self._proxy(lane)
        for server in self.servers:
            threading.Thread(target=server.serve_forever, daemon=True).start()

    def _proxy(self, lane):
        owner = self
        self.proxy_sockets[lane] = set()

        class ProxyServer(Server):
            def get_request(self):
                connection, address = super().get_request()
                owner._track(lane, connection)
                return connection, address

        class Proxy(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def finish(self):
                try:
                    super().finish()
                finally:
                    owner._untrack(lane, self.connection)

            def target(self, address):
                parsed = urllib.parse.urlsplit(address if '://' in address else '//' + address)
                if parsed.hostname != '127.0.0.1' or parsed.port not in owner.ports:
                    raise ValueError('Fixture proxy rejects all non-fixture destinations')
                return parsed

            def do_GET(self):
                conn = upstream = None
                try:
                    p = self.target(self.path)
                    pending = p.path == '/route-pending' and urllib.parse.parse_qs(p.query).get('case') == ['route-loading']
                    conn = http.client.HTTPConnection(p.hostname, p.port, timeout=32 if pending else 8)
                    conn.connect()
                    upstream = conn.sock
                    owner._track(lane, upstream)
                    headers = {k: v for k, v in self.headers.items()
                               if k.lower() not in ('proxy-connection', 'connection', 'x-fixture-route')}
                    headers['X-Fixture-Route'] = lane
                    conn.request('GET', urllib.parse.urlunsplit(('', '', p.path, p.query, '')), headers=headers)
                    response = conn.getresponse()
                    body = response.read()
                    self.send_response(response.status)
                    for k, v in response.getheaders():
                        if k.lower() not in ('connection', 'transfer-encoding', 'server', 'date'):
                            self.send_header(k, v)
                    self.end_headers()
                    self.wfile.write(body)
                except (ValueError, OSError, http.client.HTTPException):
                    try:
                        self.send_error(502)
                    except OSError:
                        pass
                finally:
                    if upstream:
                        owner._untrack(lane, upstream)
                    if conn:
                        conn.close()

            def do_CONNECT(self):
                upstream = None
                peer_port = None
                try:
                    p = self.target(self.path)
                    upstream = socket.create_connection((p.hostname, p.port), timeout=8)
                    owner._track(lane, upstream)
                    peer_port = upstream.getsockname()[1]
                    with owner.lock:
                        owner.peers[peer_port] = lane
                        owner.tunnels.append({'lane': lane, 'port': p.port})
                    self.send_response(200, 'Connection established')
                    self.end_headers()
                    upstream.setblocking(False)
                    self.connection.setblocking(False)
                    deadline = time.monotonic() + 90
                    while time.monotonic() < deadline:
                        ready, _, _ = select.select([self.connection, upstream], [], [], 1)
                        for source in ready:
                            data = source.recv(65536)
                            if not data:
                                return
                            destination = upstream if source is self.connection else self.connection
                            destination.settimeout(8)
                            destination.sendall(data)
                            destination.setblocking(False)
                except (ValueError, OSError):
                    pass
                finally:
                    if upstream:
                        with owner.lock:
                            owner.peers.pop(peer_port, None)
                        owner._untrack(lane, upstream)
                        upstream.close()

        server = ProxyServer(('127.0.0.1', 0), Proxy)
        self.proxy_servers[lane] = server
        self.servers.append(server)
        return server.server_port

    def _track(self, lane, connection):
        with self.lock:
            if lane in self.stopped_proxies:
                connection.close()
                raise ConnectionAbortedError('Fixture proxy has been stopped')
            self.proxy_sockets[lane].add(connection)

    def _untrack(self, lane, connection):
        with self.lock:
            self.proxy_sockets[lane].discard(connection)

    def stop_proxy(self, lane):
        """Close this owned listener AND all accepted HTTP/CONNECT transports."""
        with self.lock:
            if lane in self.stopped_proxies:
                return
            self.stopped_proxies.add(lane)
            connections = list(self.proxy_sockets[lane])
            self.proxy_sockets[lane].clear()
        for connection in connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()
        server = self.proxy_servers[lane]
        server.shutdown()
        server.server_close()

    def rtc_script(self):
        # Only an actual CEF ICE event supplies candidate witnesses. No remote
        # description/candidate is installed, so the reserved mapped IP is
        # never used as an outgoing ICE peer destination.
        return ("if(q.has('rtc')){(async()=>{let pc,timer,done=false;"
                "const send=(event,extra={})=>fetch('/rtc?'+new URLSearchParams({case:q.get('case'),event,...extra}),{cache:'no-store'});"
                "try{await send('started');pc=new RTCPeerConnection({iceServers:[{urls:"
                + json.dumps(self.stun_uri) + "}]});"
                "const finish=()=>{if(done||pc.iceGatheringState!=='complete')return;done=true;clearTimeout(timer);send('gathering-done').finally(()=>{pc.close();void send('closed')})};"
                "pc.onicecandidate=e=>{if(e.candidate){const parts=e.candidate.candidate.split(/\\s+/),i=parts.indexOf('typ');"
                "if(i>=0&&parts[i+1]==='srflx')void send('candidate',{type:parts[i+1],address:parts[4]})}else finish()};"
                "pc.onicegatheringstatechange=finish;pc.createDataChannel('fixture');"
                "timer=setTimeout(()=>{if(done)return;done=true;void send('timeout');pc.close()},14000);"
                "await pc.setLocalDescription(await pc.createOffer());finish();"
                "}catch(e){clearTimeout(timer);if(pc)pc.close();void send('error')}})()}" )

    def page_html(self, case):
        # Draft has an actual beforeunload refusal. Interactive deliberately
        # uses ordinary page APIs without dirty input; other cases stay plain.
        controls = handlers = ''
        if case == 'draft':
            controls = "<input id='draft' style='position:absolute;left:20px;top:20px'>"
            handlers = ("document.querySelector('#draft').oninput=()=>fetch('/receipt/draft-typed');"
                        "addEventListener('beforeunload',event=>{if(document.querySelector('#draft').value){"
                        "event.preventDefault();event.returnValue='';}});")
        elif case == 'interactive':
            controls = ("<button id='route-interaction' style='position:absolute;left:20px;top:20px;width:140px;height:40px;border:0;padding:0;background:#7b1fa2;color:white'>Interact</button>"
                        "<input id='route-search' type='search' style='position:absolute;left:20px;top:70px'>"
                        "<canvas id='route-canvas' width='120' height='60' style='position:absolute;left:20px;top:110px'></canvas>")
            handlers = ("sessionStorage.setItem('route-interactive','ordinary-api-use');"
                        "const ctx=document.querySelector('#route-canvas').getContext('2d');"
                        "ctx.fillStyle='#185541';ctx.fillRect(0,0,120,60);"
                        "document.querySelector('#route-interaction').onclick=event=>{if(event.isTrusted)"
                        "fetch('/receipt/route-interaction?'+new URLSearchParams({case:q.get('case'),storage:localStorage.getItem('route-store')||'empty'}),{cache:'no-store'})};")
        elif case == 'route-loading':
            controls = "<img id='route-pending' alt='' src='/route-pending?case=route-loading'>"
        elif case == 'media':
            controls = "<button id='media' style='position:absolute;left:20px;top:70px;width:140px;height:40px;border:0;padding:0;background:#185541;color:white'>Start media</button><audio id='audio' loop></audio>"
            handlers = ("document.querySelector('#media').onclick=async event=>{if(!event.isTrusted)return;"
                        "const mark=name=>fetch('/receipt/'+name,{cache:'no-store'});void mark('media-trusted-click');"
                        "try{const c=new AudioContext();window.fixtureAudio=c;void mark('media-context-created');"
                        "await c.resume();void mark('media-resume-resolved');"
                        "const o=c.createOscillator(),g=c.createGain();g.gain.value=.02;o.connect(g);g.connect(c.destination);o.start();"
                        "if(c.state==='running')void mark('media-running')}catch{void mark('media-start-error')}};")
        return ("<!doctype html><title>Route fixture</title><p>Owned route fixture</p>" + controls
                + "<script>const q=new URLSearchParams(location.search);"
                "if(q.has('seed'))localStorage.setItem('route-store',q.get('seed'));"
                "function beat(){fetch('/receipt/'+q.get('case')+'?storage='+encodeURIComponent(localStorage.getItem('route-store')||'empty'),{cache:'no-store'})}"
                "if(!location.pathname.startsWith('/receipt/')){beat();setInterval(beat,700)}"
                + handlers + self.rtc_script() + "</script>").encode()

    def worker_script(self):
        # The page never receives this nonce or fabricates worker receipts.
        # Only the actual installed worker adds the proof header to its fetch.
        return ("const proof=" + json.dumps(self.worker_token) + ";"
                "const receipt=(name,event)=>fetch('/worker/receipt?'+new URLSearchParams({case:name,event}),"
                "{headers:{'X-Fixture-Worker':proof},credentials:'include',cache:'no-store'});"
                "self.addEventListener('install',e=>e.waitUntil(self.skipWaiting()));"
                "self.addEventListener('activate',e=>e.waitUntil(self.clients.claim().then(()=>receipt('worker-activate','activate'))));"
                "self.addEventListener('message',e=>{const name=e.data?.case;"
                "if(typeof name==='string'&&name.startsWith('worker-')&&name.length<100)e.waitUntil(receipt(name,'message'));});")

    def worker_url(self, case):
        return 'http://127.0.0.1:' + str(self.http) + '/worker/page?' + urllib.parse.urlencode({'case': case})

    def url(self, case, seed=None, tls=False, rtc=False, persist=False):
        query = {'case': case}
        if rtc:
            query['rtc'] = '1'
        if seed:
            query['seed'] = seed
        if persist:
            if not seed:
                raise ValueError('Persistent fixture cookie requires an explicit seed case')
            query['persist'] = '1'
        return ('https' if tls else 'http') + '://127.0.0.1:' + str(self.https if tls else self.http) + '/page?' + urllib.parse.urlencode(query)

    def records(self, case):
        with self.lock:
            return [dict(item) for item in self.receipts if item['path'] == '/receipt/' + case]

    def requests(self, case):
        with self.lock:
            return [dict(item) for item in self.receipts
                    if item.get('case') == case or item['path'] == '/receipt/' + case]

    def release_loading(self):
        """Release the actual unfinished origin response, never synthesize a load ACK."""
        self._loading_release.set()

    def close(self):
        self.release_loading()
        for lane in self.proxy_servers:
            self.stop_proxy(lane)
        for server in self.servers:
            if server in self.proxy_servers.values():
                continue
            server.shutdown()
            server.server_close()
