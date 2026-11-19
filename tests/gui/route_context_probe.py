"""Offline CEF capability measurement, not a product routing acceptance test."""
import argparse
import http.client
import http.server
import json
import os
import pathlib
import select
import signal
import subprocess
import tempfile
import threading
import time
import urllib.parse


def scenario(exe, workspace, kind, display):
    profile = workspace / kind
    lock = threading.Condition()
    reports, navigation, denied = {}, {}, []
    first_report = threading.Event()

    class Origin(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            url = urllib.parse.urlsplit(self.path)
            if url.path == '/favicon.ico':
                self.send_response(204); self.end_headers(); return
            context = urllib.parse.parse_qs(url.query).get('context', [''])[0]
            if url.path not in ('/first', '/second') or context not in ('A', 'B'):
                self.send_error(404); return
            with lock:
                navigation[context] = {'start': time.monotonic(), 'route': self.headers.get('X-Route-Probe', 'DIRECT')}
                lock.notify_all()
                lock.wait_for(lambda: len(navigation) == 2, timeout=3)
            if context == 'B' and not first_report.wait(12):
                self.send_error(504); return
            time.sleep(.2)
            script = '''const context=CONTEXT;const before={storage:localStorage.getItem('route-probe-seed'),cookie:document.cookie};
if(context==='A'){localStorage.setItem('route-probe-seed','A');document.cookie='route_probe_seed=A; Path=/; SameSite=Lax';}
fetch('/report?context='+context,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(before)});'''.replace('CONTEXT', json.dumps(context))
            body = ('<!doctype html><title>Route context fixture</title><script>'+script+'</script>').encode()
            self.send_response(200); self.send_header('Content-Type', 'text/html'); self.send_header('Content-Length', str(len(body))); self.end_headers()
            self.wfile.write(body)
            with lock:
                navigation[context]['end'] = time.monotonic()

        def do_POST(self):
            context = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query).get('context', [''])[0]
            size = int(self.headers.get('Content-Length', '0'))
            if self.path.split('?')[0] != '/report' or context not in ('A', 'B') or not 0 < size < 4096:
                self.send_error(400); return
            value = json.loads(self.rfile.read(size))
            with lock:
                reports[context] = {'storage': value.get('storage'), 'cookie': value.get('cookie', ''), 'route': self.headers.get('X-Route-Probe', 'DIRECT')}
                if context == 'A': first_report.set()
                if len(reports) == 2: (profile/'done').touch()
            self.send_response(204); self.end_headers()

    origin = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Origin)
    servers = [origin]
    def proxy(label):
        class Proxy(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass
            def do_CONNECT(self):
                denied.append(True); self.send_error(403)
            def forward(self):
                url = urllib.parse.urlsplit(self.path)
                if url.scheme != 'http' or url.hostname != '127.0.0.1' or url.port != origin.server_port:
                    denied.append(True); self.send_error(403); return
                size = int(self.headers.get('Content-Length', '0'))
                if size > 4096:
                    self.send_error(413); return
                headers = {k: v for k, v in self.headers.items() if k.lower() not in ('connection', 'proxy-connection', 'transfer-encoding', 'x-route-probe')}
                headers['X-Route-Probe'] = label
                connection = http.client.HTTPConnection('127.0.0.1', origin.server_port, timeout=20)
                try:
                    connection.request(self.command, urllib.parse.urlunsplit(('', '', url.path, url.query, '')), self.rfile.read(size) if size else None, headers)
                    response = connection.getresponse(); body = response.read()
                    self.send_response(response.status)
                    for key, value in response.getheaders():
                        if key.lower() not in ('connection', 'transfer-encoding', 'content-length'): self.send_header(key, value)
                    self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
                finally: connection.close()
            do_GET = forward
            do_POST = forward
        instance = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Proxy)
        servers.append(instance)
        return 'http://127.0.0.1:'+str(instance.server_port)
    proxy_a, proxy_b = proxy('A'), proxy('B')
    for server in servers:
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
    env = {**os.environ, 'DISPLAY': display}
    command = [str(exe), '--probe-root', str(profile), '--scenario', kind, '--proxy-a', proxy_a, '--proxy-b', proxy_b, '--target-base', 'http://127.0.0.1:'+str(origin.server_port)]
    process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    try:
        stdout, stderr = process.communicate(timeout=55)
        (workspace/(kind+'-native.jsonl')).write_text(stdout)
        (workspace/(kind+'-native.log')).write_text(stderr)
        api = [json.loads(line) for line in stdout.splitlines() if line.startswith('{')]
        if process.returncode or set(reports) != {'A', 'B'}:
            raise AssertionError('Native '+kind+' probe did not complete; inspect its bounded diagnostic output')
        expected = {'independent': {'A':'A','B':'B'}, 'shared': {'A':'B','B':'B'}, 'cli': {'A':'A','B':'A'}}[kind]
        actual = {key: row['route'] for key, row in reports.items()}
        navigation_routes = {key: row['route'] for key, row in navigation.items()}
        shared = reports['B']['storage'] == 'A' and 'route_probe_seed=A' in reports['B']['cookie']
        storage_expected = shared if kind == 'shared' else reports['B']['storage'] is None and not reports['B']['cookie']
        overlap = max(row['start'] for row in navigation.values()) < min(row['end'] for row in navigation.values())
        matches = actual == expected and navigation_routes == expected and storage_expected and overlap
        return {'scenario':kind,'actual_fetch_routes':actual,'actual_navigation_routes':navigation_routes,'expected_routes':expected,'storage_shared':shared,'storage_matches_expected':storage_expected,'navigation_overlapped':overlap,'native_api':api,'blocked_nonfixture_proxy_requests':len(denied),'matches_expected_matrix':matches,'feature_ready':False}
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try: process.wait(timeout=3)
            except subprocess.TimeoutExpired: os.killpg(process.pid, signal.SIGKILL); process.wait()
        else:
            try: os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError: pass
        for server in servers: server.shutdown(); server.server_close()


def main():
    parser = argparse.ArgumentParser(); parser.add_argument('--exe', required=True, type=pathlib.Path); parser.add_argument('--out', required=True, type=pathlib.Path)
    args = parser.parse_args(); args.out.mkdir(parents=True, exist_ok=True)
    read_fd, write_fd = os.pipe()
    xlog = (args.out/'xvfb.log').open('wb')
    xserver = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1280x900x24', '-nolisten', 'tcp'], pass_fds=(write_fd,), stdout=xlog, stderr=subprocess.STDOUT, start_new_session=True)
    os.close(write_fd)
    rows=[]
    try:
        ready, _, _ = select.select([read_fd], [], [], 25)
        assert ready, 'Xvfb display allocation timed out'
        number = os.read(read_fd, 32).decode().strip(); assert number.isdigit()
        with tempfile.TemporaryDirectory(prefix='cef-route-probe-') as temporary:
            workspace=pathlib.Path(temporary)
            for kind in ('independent','shared','cli'):
                try:
                    row=scenario(args.exe.resolve(),workspace,kind,':'+number); rows.append(row)
                finally:
                    for path in workspace.glob(kind+'-native.*'):
                        (args.out/path.name).write_bytes(path.read_bytes())
        assert all(row['matches_expected_matrix'] for row in rows), 'Actual routes or storage do not match the proposed capability matrix'
        (args.out/'results.json').write_text(json.dumps({'measurements':rows,'capability_measurement_passed':True,'product_feature_ready':False},indent=2)+'\n')
        print(json.dumps({'capability_measurement_passed':True,'product_feature_ready':False}))
    except Exception as error:
        (args.out/'results.json').write_text(json.dumps({'measurements':rows,'capability_measurement_passed':False,'product_feature_ready':False,'failure':str(error)[:1000]},indent=2)+'\n')
        raise
    finally:
        os.close(read_fd); xserver.terminate()
        try:xserver.wait(timeout=3)
        except subprocess.TimeoutExpired:xserver.kill();xserver.wait()
        xlog.close()

if __name__=='__main__':main()
