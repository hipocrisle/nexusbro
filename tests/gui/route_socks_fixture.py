"""Authenticated, loopback-only SOCKS5 witness for native route acceptance.

Only owner.ports (its HTTP and HTTPS origins) are reachable. TLS bytes pass
through unchanged; origin receipts identify the lane from the upstream peer.
No credentials, request payloads or cookies are retained in diagnostic events.
"""
import secrets
import select
import socket
import socketserver
import threading
import time


class SocksFixture:
    def __init__(self, owner):
        self.owner = owner
        self.events = []
        self._lock = threading.Lock()
        self._sockets = set()
        self._peer_ports = set()
        self._closed = False
        self._username = secrets.token_hex(12).encode('ascii')
        self._password = secrets.token_hex(20).encode('ascii')
        self._ports = frozenset(owner.ports)
        if not self._ports or any(not isinstance(port, int) or not 0 < port < 65536 for port in self._ports):
            raise ValueError('SOCKS fixture requires explicit loopback origin ports')
        with owner.lock:
            owner.tokens['Socks'] = secrets.token_hex(20)
        fixture = self

        class Server(socketserver.ThreadingTCPServer):
            daemon_threads = True
            block_on_close = False
            allow_reuse_address = True

            def get_request(self):
                connection, address = super().get_request()
                fixture._track(connection)
                return connection, address

            def handle_error(self, request, client_address):
                # All failures are bounded fixture protocol failures. Do not
                # let a traceback accidentally print a credential buffer.
                pass

        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                fixture._handle(self.request)

        self._server = Server(('127.0.0.1', 0), Handler)
        self.port = self._server.server_address[1]
        self.url = 'socks5://' + self._username.decode('ascii') + ':' + self._password.decode('ascii') + '@127.0.0.1:' + str(self.port)
        self._thread = threading.Thread(target=lambda: self._server.serve_forever(poll_interval=.1), daemon=True)
        self._thread.start()

    def _track(self, connection):
        with self._lock:
            if not self._closed:
                self._sockets.add(connection)
                return
        connection.close()
        raise OSError('SOCKS fixture is closed')

    def _untrack(self, connection):
        with self._lock:
            self._sockets.discard(connection)

    def _record(self, authenticated, atyp, host, port):
        # Record only complete, allowlisted CONNECT attempts. An unauthenticated
        # handshake never reaches target parsing and produces no origin event.
        if atyp != 3 or host not in ('127.0.0.1', 'localhost') or port not in self._ports:
            return
        with self._lock:
            self.events.append({'authenticated': bool(authenticated), 'atyp': atyp,
                                'host': host, 'port': port})

    @staticmethod
    def _read(connection, count):
        value = bytearray()
        while len(value) < count:
            chunk = connection.recv(count - len(value))
            if not chunk:
                raise OSError('Incomplete SOCKS fixture request')
            value.extend(chunk)
        return bytes(value)

    @staticmethod
    def _reply(connection, status, bound_port=0):
        connection.sendall(bytes([5, status, 0, 1, 127, 0, 0, 1]) + int(bound_port).to_bytes(2, 'big'))

    def _handle(self, client):
        upstream = None
        peer_port = None
        authenticated = False
        atyp = host = target_port = None
        recorded = False
        try:
            client.settimeout(8)
            version, count = self._read(client, 2)
            if version != 5 or count == 0:
                return
            methods = self._read(client, count)
            if 2 not in methods:
                client.sendall(bytes([5, 255]))
                return
            client.sendall(bytes([5, 2]))  # RFC 1929 required; never anonymous.
            version, length = self._read(client, 2)
            if version != 1 or length == 0:
                client.sendall(bytes([1, 1]))
                return
            username = self._read(client, length)
            length = self._read(client, 1)[0]
            if length == 0:
                client.sendall(bytes([1, 1]))
                return
            password = self._read(client, length)
            authenticated = (secrets.compare_digest(username, self._username)
                             & secrets.compare_digest(password, self._password))
            client.sendall(bytes([1, 0 if authenticated else 1]))
            if not authenticated:
                return
            version, command, reserved, atyp = self._read(client, 4)
            if version != 5 or reserved != 0 or command != 1:
                self._reply(client, 7)
                return
            if atyp != 3:
                self._reply(client, 8)
                return
            length = self._read(client, 1)[0]
            if length == 0:
                self._reply(client, 8)
                return
            try:
                host = self._read(client, length).decode('ascii')
            except UnicodeDecodeError:
                self._reply(client, 8)
                return
            target_port = int.from_bytes(self._read(client, 2), 'big')
            if host not in ('127.0.0.1', 'localhost') or target_port not in self._ports:
                self._reply(client, 2)
                return
            # Even "localhost" never invokes DNS here. The origin allowlist is
            # fixed at construction; the fixture cannot become an open proxy.
            upstream = socket.create_connection(('127.0.0.1', target_port), timeout=8)
            self._track(upstream)
            peer_port = upstream.getsockname()[1]
            # Serialize registration with close(): no late worker may add a
            # lane mapping after teardown has removed all owned peers.
            with self._lock:
                if self._closed:
                    raise OSError('SOCKS fixture is closed')
                with self.owner.lock:
                    self.owner.peers[peer_port] = 'Socks'
                self._peer_ports.add(peer_port)
            self._record(True, atyp, host, target_port)
            recorded = True
            self._reply(client, 0, peer_port)
            # No HTTP parsing, TLS termination or auth-header fabrication.
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                readable, _, _ = select.select([client, upstream], [], [], .5)
                for source in readable:
                    chunk = source.recv(65536)
                    if not chunk:
                        return
                    destination = upstream if source is client else client
                    destination.sendall(chunk)
        except (OSError, ValueError):
            pass
        finally:
            if not recorded:
                self._record(authenticated, atyp, host, target_port)
            if peer_port is not None:
                with self._lock:
                    with self.owner.lock:
                        self.owner.peers.pop(peer_port, None)
                    self._peer_ports.discard(peer_port)
            for connection in (upstream, client):
                if connection is not None:
                    self._untrack(connection)
                    try:
                        connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    connection.close()

    def close(self):
        with self._lock:
            if self._closed:
                return
            self._closed = True
            sockets = list(self._sockets)
            self._sockets.clear()
            with self.owner.lock:
                for peer_port in self._peer_ports:
                    self.owner.peers.pop(peer_port, None)
            self._peer_ports.clear()
        # Existing tunnels must die as well as the listener; a stale live socket
        # must not make the stopped-route/no-Direct-fallback check pass falsely.
        for connection in sockets:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=3)
        if self._thread.is_alive():
            raise RuntimeError('Owned SOCKS fixture listener did not stop')
