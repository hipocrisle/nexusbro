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
    def __init__(self, cert, key):
        self.lock = threading.Lock()
        self.receipts, self.tunnels, self.peers = [], [], {}
        self.tokens = {lane: secrets.token_hex(20) for lane in ('A', 'B', 'Direct')}
        self.servers = []
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
                                           'time': time.monotonic(),
                                           'tls': isinstance(self.connection, ssl.SSLSocket)})
                seed = q.get('seed', [None])[0]
                if seed and seed not in owner.tokens:
                    self.send_error(400)
                    return
                body = ("<!doctype html><title>Route fixture</title><input id='draft' style='position:absolute;left:20px;top:20px'>"
                        "<button id='media' style='position:absolute;left:20px;top:70px'>Start media</button><audio id='audio' loop></audio>"
                        "<script>const q=new URLSearchParams(location.search);"
                        "if(q.has('seed'))localStorage.setItem('route-store',q.get('seed'));"
                        "function beat(){fetch('/receipt/'+q.get('case')+'?storage='+encodeURIComponent(localStorage.getItem('route-store')||'empty'),{cache:'no-store'})}"
                        "if(!location.pathname.startsWith('/receipt/')){beat();setInterval(beat,700)}"
                        "document.querySelector('#draft').oninput=()=>fetch('/receipt/draft-typed');"
                        "document.querySelector('#media').onclick=async()=>{const c=new AudioContext();await c.resume();"
                        "const o=c.createOscillator(),g=c.createGain();g.gain.value=.02;o.connect(g);g.connect(c.destination);o.start();window.fixtureAudio=c;if(c.state==='running')fetch('/receipt/media-running')};"
                        "</script>").encode()
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Cache-Control', 'no-store')
                if seed:
                    self.send_header('Set-Cookie', 'route_auth=' + owner.tokens[seed] + '; HttpOnly; SameSite=Lax; Path=/')
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
                    conn = http.client.HTTPConnection(p.hostname, p.port, timeout=8)
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

    def url(self, case, seed=None, tls=False):
        query = {'case': case}
        if seed:
            query['seed'] = seed
        return ('https' if tls else 'http') + '://127.0.0.1:' + str(self.https if tls else self.http) + '/page?' + urllib.parse.urlencode(query)

    def records(self, case):
        with self.lock:
            return [dict(item) for item in self.receipts if item['path'] == '/receipt/' + case]

    def requests(self, case):
        with self.lock:
            return [dict(item) for item in self.receipts
                    if item.get('case') == case or item['path'] == '/receipt/' + case]

    def close(self):
        for lane in self.proxy_servers:
            self.stop_proxy(lane)
        for server in self.servers:
            if server in self.proxy_servers.values():
                continue
            server.shutdown()
            server.server_close()
