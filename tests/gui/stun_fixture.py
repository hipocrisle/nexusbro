"""Bounded loopback-only RFC 5389 Binding responder; no external traffic."""
import ipaddress
import socket
import struct
import threading
import time


COOKIE = 0x2112A442
MAPPED = ipaddress.IPv4Address('203.0.113.10')  # RFC 5737 documentation range.


def binding_reply(packet, source_port):
    """Reply only to well-formed modern Binding requests, preserving the transaction."""
    if len(packet) < 20:
        return None
    kind, length, cookie = struct.unpack('!HHI', packet[:8])
    if kind != 0x0001 or cookie != COOKIE or length % 4 or len(packet) != 20 + length:
        return None
    offset = 20
    while offset < len(packet):
        if offset + 4 > len(packet):
            return None
        _, size = struct.unpack('!HH', packet[offset:offset + 4])
        offset += 4 + ((size + 3) & ~3)
        if offset > len(packet):
            return None
    mapped = struct.pack('!BBHI', 0, 1, source_port ^ (COOKIE >> 16), int(MAPPED) ^ COOKIE)
    attribute = struct.pack('!HH', 0x0020, len(mapped)) + mapped
    return struct.pack('!HHI', 0x0101, len(attribute), COOKIE) + packet[8:20] + attribute


class StunFixture:
    def __init__(self):
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.socket.bind(('127.0.0.1', 0))
        self.socket.settimeout(.2)
        self.port = self.socket.getsockname()[1]
        self.lock = threading.Lock()
        self.packets = []
        self.stopped = threading.Event()
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _serve(self):
        while not self.stopped.is_set():
            try:
                packet, peer = self.socket.recvfrom(65535)
            except socket.timeout:
                continue
            except OSError:
                return
            reply = binding_reply(packet, peer[1])
            with self.lock:
                # Count every received UDP packet, including malformed probes.
                self.packets.append({'time': time.monotonic(), 'binding': reply is not None})
            if reply is not None:
                try:
                    self.socket.sendto(reply, peer)
                except OSError:
                    if self.stopped.is_set():
                        return

    def since(self, timestamp):
        with self.lock:
            return [dict(packet) for packet in self.packets if packet['time'] >= timestamp]

    def close(self):
        self.stopped.set()
        self.socket.close()
        self.thread.join(timeout=1)
