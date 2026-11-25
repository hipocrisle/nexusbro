"""GitHub-only Linux recovery acceptance against the actual packaged application.

No IPC/CEF mocks: preseeded committed sessions, physical tab click, abrupt owned
process-tree kill, actual restart and explicit-close/graceful-exit behavior.
All URLs and browsing data belong to one loopback fixture.
"""
import argparse
import ctypes
import ctypes.util
import http.server
import json
import os
import pathlib
import signal
import socket
import subprocess
import threading
import time
import urllib.parse

import psutil
from websocket import WebSocketException
from client import Client


def wait(check, message, timeout=30):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            last = check()
            if last:
                return last
        except (ConnectionError, OSError, RuntimeError, AssertionError, WebSocketException):
            pass
        time.sleep(.1)
    raise AssertionError((message, last))


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def close_owned_window(client, app, exe, display, expected_label):
    """Send the same WM_DELETE_WINDOW client message a window manager sends.

    Xvfb intentionally has no window manager. Do not widen application IPC ACLs
    or use XDestroyWindow: the app must receive its actual close-request event.
    """
    label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    assert valid_close_label(expected_label) and label==expected_label, 'Refusing to close an unexpected window'
    assert app.poll() is None, 'Owned app already exited'
    client.command('fokus_obolochke')
    xenv = {**os.environ, 'DISPLAY': display}
    focused = int(subprocess.check_output(['xdotool', 'getwindowfocus'], env=xenv, text=True).strip())
    assert focused not in (0, 1), 'Native secondary shell did not obtain X11 focus'
    held=psutil.Process(app.pid)
    owned_shells=owned_shell_identities([held,*held.children(recursive=True)],exe)

    atom = ctypes.c_ulong
    window = ctypes.c_ulong
    pointer = ctypes.c_void_p
    x11 = ctypes.CDLL(ctypes.util.find_library('X11') or 'libX11.so.6')
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = pointer
    x11.XCloseDisplay.argtypes = [pointer]
    x11.XInternAtom.argtypes = [pointer, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = atom
    x11.XGetWMProtocols.argtypes = [pointer, window, ctypes.POINTER(ctypes.POINTER(atom)), ctypes.POINTER(ctypes.c_int)]
    x11.XGetWMProtocols.restype = ctypes.c_int
    x11.XQueryTree.argtypes = [pointer, window, ctypes.POINTER(window), ctypes.POINTER(window), ctypes.POINTER(ctypes.POINTER(window)), ctypes.POINTER(ctypes.c_uint)]
    x11.XQueryTree.restype = ctypes.c_int
    x11.XFree.argtypes = [pointer]
    x11.XFlush.argtypes = [pointer]
    x11.XSync.argtypes = [pointer, ctypes.c_int]

    class Data(ctypes.Union):
        _fields_ = [('b', ctypes.c_char * 20), ('s', ctypes.c_short * 10), ('l', ctypes.c_long * 5)]

    class Message(ctypes.Structure):
        _fields_ = [('type', ctypes.c_int), ('serial', ctypes.c_ulong), ('send_event', ctypes.c_int),
                    ('display', pointer), ('window', window), ('message_type', atom), ('format', ctypes.c_int), ('data', Data)]

    class Event(ctypes.Union):
        _fields_ = [('client', Message), ('pad', ctypes.c_long * 24)]

    x11.XSendEvent.argtypes = [pointer, window, ctypes.c_int, ctypes.c_long, ctypes.POINTER(Event)]
    x11.XSendEvent.restype = ctypes.c_int
    connection = x11.XOpenDisplay(display.encode())
    assert connection, 'Could not open the owned Xvfb display'
    try:
        protocols_atom = x11.XInternAtom(connection, b'WM_PROTOCOLS', False)
        delete_atom = x11.XInternAtom(connection, b'WM_DELETE_WINDOW', False)
        candidate = focused
        verified = None
        for _ in range(16):
            protocols = ctypes.POINTER(atom)()
            count = ctypes.c_int()
            supports_delete = False
            if x11.XGetWMProtocols(connection, candidate, ctypes.byref(protocols), ctypes.byref(count)):
                try:
                    supports_delete = delete_atom in [protocols[i] for i in range(count.value)]
                finally:
                    if protocols:
                        x11.XFree(protocols)
            if supports_delete:
                pid = int(subprocess.check_output(['xdotool', 'getwindowpid', str(candidate)], env=xenv, text=True).strip())
                assert pid in owned_shells, 'Focused X11 client does not belong to the isolated app shell'
                title = subprocess.check_output(['xdotool', 'getwindowname', str(candidate)], env=xenv, text=True).strip()
                assert title == ('NexusBro' if expected_label=='main' else 'NexusBro — Recovery'), 'Focused X11 client is not the expected shell'
                raw = subprocess.check_output(['xdotool', 'getwindowgeometry', '--shell', str(candidate)], env=xenv, text=True)
                geometry = dict(line.split('=', 1) for line in raw.splitlines() if '=' in line)
                viewport = client.js('({width:innerWidth,height:innerHeight,scale:devicePixelRatio||1})')
                width, height = int(geometry['WIDTH']), int(geometry['HEIGHT'])
                assert width >= 300 and height >= 200, 'Secondary native window has invalid geometry'
                assert abs(width - viewport['width'] * viewport['scale']) <= 64 and abs(height - viewport['height'] * viewport['scale']) <= 96, 'Focused X11 geometry does not match the inspected secondary renderer'
                verified = candidate
                break
            root, parent = window(), window()
            children = ctypes.POINTER(window)()
            nchildren = ctypes.c_uint()
            assert x11.XQueryTree(connection, candidate, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(nchildren)), 'Could not resolve focused X11 shell ancestry'
            if children:
                x11.XFree(children)
            if parent.value in (0, root.value, candidate):
                break
            candidate = parent.value
        assert verified is not None, 'Owned secondary shell does not advertise WM_DELETE_WINDOW'
        # Recheck the same live OS process identity immediately before close.
        pid=int(subprocess.check_output(['xdotool','getwindowpid',str(verified)],env=xenv,text=True).strip())
        process=psutil.Process(pid)
        assert pid in owned_shells and process.create_time()==owned_shells[pid] and pathlib.Path(process.exe())==exe and app.poll() is None, 'Owned shell identity changed before close'
        assert client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')==expected_label, 'Inspected window identity changed before close'
        event = Event()
        event.client.type = 33  # ClientMessage
        event.client.send_event = True
        event.client.display = connection
        event.client.window = verified
        event.client.message_type = protocols_atom
        event.client.format = 32
        event.client.data.l[0] = delete_atom
        event.client.data.l[1] = 0  # CurrentTime
        assert x11.XSendEvent(connection, verified, False, 0, ctypes.byref(event)), 'WM_DELETE_WINDOW was not sent'
        x11.XFlush(connection)
        x11.XSync(connection, False)
    finally:
        x11.XCloseDisplay(connection)


def owned_shell_identities(processes,exe):
    owned={}
    for process in processes:
        try:
            args=process.cmdline()
            if pathlib.Path(process.exe())==exe and '--nb-dvizhok' not in args and not any(a.startswith('--type=') for a in args):
                owned[process.pid]=process.create_time()
        except psutil.NoSuchProcess:
            pass
    assert len(owned)==1,'Isolated app shell PID is ambiguous'
    return owned


def valid_close_label(label):
    return isinstance(label,str) and (label=='main' or label.startswith('browser-'))


def close_owned_secondary(client,app,exe,display):
    label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    assert isinstance(label,str) and label.startswith('browser-'),'Refusing to close a non-secondary window'
    close_owned_window(client,app,exe,display,label)


def normal_close_order(clients,close,hidden):
    assert 'main' in clients and len(clients)<=2 and all(valid_close_label(label) for label in clients),'Unexpected owned window set'
    secondary=[label for label in clients if label!='main']
    close(clients['main'],'main')
    if secondary:
        label=secondary[0]
        hidden(clients[label])
        close(clients[label],label)


def known_execution_gone(process,birth):
    try:
        return not process.is_running() or process.create_time()!=birth or process.status()==psutil.STATUS_ZOMBIE
    except psutil.NoSuchProcess:
        return True


def self_test():
    events=[];clients={'main':'m','browser-fixture':'s'}
    normal_close_order(clients,lambda client,label:events.append(('close',label)),lambda client:events.append(('hidden',client)))
    assert events==[('close','main'),('hidden','s'),('close','browser-fixture')]
    events=[];normal_close_order({'main':'m'},lambda client,label:events.append(label),lambda _:events.append('unexpected'))
    assert events==['main']
    for windows in ({'main':'m','foreign':'f'}, {'browser-fixture':'s'}):
        events=[]
        try:normal_close_order(windows,lambda *_:events.append('close'),lambda _:None)
        except AssertionError:pass
        else:raise AssertionError('Foreign window set accepted')
        assert not events
    events=[]
    def rejected(*_):raise RuntimeError('Synthetic native close rejection')
    try:normal_close_order(clients,rejected,lambda _:events.append('hidden'))
    except RuntimeError:pass
    else:raise AssertionError('Close failure swallowed')
    assert not events
    events=[]
    try:normal_close_order(clients,lambda client,label:events.append(label),rejected)
    except RuntimeError:pass
    else:raise AssertionError('Visibility rejection swallowed')
    assert events==['main']
    class Process:
        def is_running(self):return True
        def create_time(self):return 5
        def status(self):return psutil.STATUS_RUNNING
    assert not known_execution_gone(Process(),5)
    assert known_execution_gone(Process(),4)
    class Shell:
        def __init__(self,pid,path,args):self.pid=pid;self.path=path;self.args=args
        def exe(self):return self.path
        def cmdline(self):return self.args
        def create_time(self):return self.pid+0.5
    path=pathlib.Path('/synthetic/nexusbro')
    direct=Shell(1,str(path),[]);cef=Shell(2,str(path),['--nb-dvizhok'])
    wrapper=Shell(3,'/synthetic/dbus-run-session',[])
    assert owned_shell_identities([direct,cef],path)=={1:1.5}
    assert owned_shell_identities([wrapper,direct,cef],path)=={1:1.5}
    try:owned_shell_identities([direct,Shell(4,str(path),[])],path)
    except AssertionError:pass
    else:raise AssertionError('Ambiguous app owners accepted')
    print(json.dumps({'self_test':True,'native_execution':False}))


def run(exe, cef, out, display=':97'):
    assert os.environ.get('GITHUB_ACTIONS') == 'true', 'Native recovery must run on a disposable CI runner'
    out.mkdir(parents=True, exist_ok=False)
    data = out / 'data'
    data.mkdir()
    hits = []
    checks = []
    terminal_success = False

    class Fixture(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            path = urllib.parse.urlsplit(self.path).path
            hits.append(path)
            title = {'/one': 'RecoveredOne', '/two': 'RecoveredTwo', '/cold-main': 'ColdMain', '/blocked': 'MustNeverLoad'}.get(path, 'Other')
            body = ('<!doctype html><title>' + title + '</title><style>body{background:#185541}</style><h1>' + title + '</h1>').encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{server.server_port}'
    inspector = free_port()
    env = {**os.environ, 'DISPLAY': display, 'CEF_PATH': str(cef), 'LD_LIBRARY_PATH': str(cef),
           'NEXUSBRO_TEST_DATA': str(data), 'NEXUSBRO_PROBA': '0',
           'WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS': '1',
           'WEBKIT_INSPECTOR_HTTP_SERVER': f'127.0.0.1:{inspector}'}
    main_id = 'recovery-main'

    def tab(ident, path, title, route=None):
        return {'id': ident, 'adres': path if path == 'about:blank' else base + path,
                'zagolovok': title, 'route': route or {'kind': 'inherit'}}

    def session(tabs, active):
        return {'vkladki': tabs, 'aktivnaya': active, 'sleduyushchaya': 999, 'zakrytye': []}

    def write(path, obj):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(obj))

    definitions = [{'id': main_id, 'nazvanie': 'Recovery', 'proxy': '', 'adres': 'about:blank', 'rezat_reklamu': False, 'pustaya': False},
                   {'id': 'legacy-private', 'nazvanie': 'Private', 'proxy': '', 'adres': 'about:blank', 'rezat_reklamu': False, 'pustaya': False, 'privatnaya': True}]
    write(data / 'vkladki.json', definitions)
    write(data / 'aktivnaya.json', main_id)
    write(data / 'nastrojki.json', {'avtoobnovleniya': False})
    profile = data / 'profili' / main_id
    write(profile / 'obychnye.backup.json', session([tab(1, 'about:blank', 'Blank'), tab(2, '/cold-main', 'ColdMain')], 1))
    (profile / 'obychnye.json').write_text('{truncated')
    tabs = [tab(201, '/one', 'RecoveryOne'), tab(202, '/two', 'RecoveryTwo', {'kind': 'direct'}),
            tab(203, '/blocked', 'Blocked', {'kind': 'connection', 'id': 'removed-connection'})]
    windows = [{'key': 'recovery-window', 'active_super': main_id, 'views': [{'super_id': main_id, 'session': session(tabs, 201)}]},
               {'key': 'private-window', 'active_super': 'legacy-private', 'views': [{'super_id': 'legacy-private', 'session': session([tab(1, '/private', 'Private')], 1)}]}]
    write(data / 'windows-recovery.json', {'schema': 1, 'windows': windows})
    app = None
    clients = {}
    x = None
    launches = 0
    expected_windows = 2

    def discover():
        nonlocal clients
        found = {}
        for index in range(5):
            try:
                c = Client(display=display, port=inspector, page_index=index)
                label = c.js('window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')
                if label == 'main' or (isinstance(label, str) and label.startswith('browser-')):
                    found[label] = c
                else:
                    c.close()
            except (ConnectionError, OSError, RuntimeError, WebSocketException):
                break
        if 'main' not in found or len(found) != expected_windows:
            for c in found.values():
                c.close()
            return False
        clients = found
        return True

    def start(expected=2):
        nonlocal app, launches, expected_windows
        expected_windows = expected
        launches += 1
        app = subprocess.Popen(['dbus-run-session', '--', str(exe)], env=env, start_new_session=True,
                               stdout=(out / f'app-{launches}.log').open('w'), stderr=subprocess.STDOUT)
        wait(discover, 'Main and one ordinary recovered window were not created', 45)
        main = clients['main']
        secondary = next((c for label, c in clients.items() if label != 'main'), None)
        main.wait('document.querySelectorAll(".obychnaya").length===2', timeout=40)
        if secondary:
            secondary.wait('document.querySelectorAll(".obychnaya").length===3', timeout=40)
            assert secondary.command('aktivnaya') == main_id
        assert main.command('aktivnaya') == main_id
        return main, secondary

    def stop(crash):
        nonlocal app, clients
        owned = psutil.Process(app.pid).children(recursive=True)
        if crash:
            # Kill the actual parent app first: no final UI/CEF callbacks may
            # turn this into a graceful-stop test. Everything killed is owned.
            parents = []
            for child in owned:
                try:
                    args = child.cmdline()
                    if '--nb-dvizhok' not in args and not any(a.startswith('--type=') for a in args) and pathlib.Path(child.exe()) == exe:
                        parents.append(child)
                except psutil.NoSuchProcess:
                    pass
            assert len(parents) == 1, 'Could not identify the isolated actual app process'
            parents[0].kill()
        else:
            identities=[]
            for process in owned:
                try:identities.append((process,process.create_time()))
                except psutil.NoSuchProcess:pass
            def main_hidden(secondary):
                deadline=time.monotonic()+10
                while time.monotonic()<deadline:
                    # This allowed read-only RPC is awaited; real rejection or
                    # premature renderer disconnect propagates, never ignored.
                    visible=secondary.command('plugin:window|is_visible',label='main')
                    assert type(visible) is bool,'Main visibility receipt invalid'
                    if not visible:return
                    time.sleep(.1)
                raise AssertionError('Main native close did not hide the owned window')
            normal_close_order(clients,lambda client,label:close_owned_window(client,app,exe,display,label),main_hidden)
            app.wait(timeout=15)
            assert app.returncode == 0
            wait(lambda:all(known_execution_gone(process,birth) for process,birth in identities),
                 'Normal exit left an owned native execution alive',5)
        for c in clients.values():
            try:
                c.close()
            except Exception:
                pass
        clients = {}
        if crash:
            try:
                os.killpg(app.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            app.wait(timeout=10)
            for child in owned:
                try:
                    child.kill()
                except psutil.NoSuchProcess:
                    pass
            psutil.wait_procs(owned, timeout=5)
        app = None
        time.sleep(.4)

    try:
        x = subprocess.Popen(['Xvfb', display, '-screen', '0', '1280x900x24', '-nolisten', 'tcp'],
                             stdout=(out / 'xvfb.log').open('w'), stderr=subprocess.STDOUT)
        time.sleep(.4)
        main, secondary = start()
        time.sleep(2)
        assert not any(path in hits for path in ['/one', '/two', '/cold-main', '/blocked', '/private']), hits
        assert len(secondary.command('obychnye_spisok', id=main_id)['vkladki']) == 3
        checks.append({'corrupt_main_uses_backup_ordinary_windows_restored_private_excluded_and_no_cold_page_requests': True})
        secondary.command('fokus_obolochke')
        secondary.click('.obychnaya[data-id="202"]')
        live = wait(lambda: (lambda s: s if any(t['zagolovok'] == 'RecoveredTwo' and t['id'] == s['aktivnaya'] and not t['zagruzka'] for t in s['vkladki']) else None)(secondary.command('obychnye_spisok', id=main_id)), 'Physical click failed to wake exact restored row')
        chosen = next(t for t in live['vkladki'] if t['id'] == live['aktivnaya'])
        assert chosen['route'] == {'kind': 'direct'} and chosen['id'] != 202
        assert '/two' in hits and '/one' not in hits and '/cold-main' not in hits, hits
        checks.append({'physical_restored_tab_click_waits_native_ack_remaps_id_and_keeps_explicit_route': True})
        wait(lambda: any(w['key'] == 'recovery-window' and w['views'][0]['session']['aktivnaya'] == live['aktivnaya'] for w in json.loads((data / 'windows-recovery.json').read_text())['windows']), 'Acknowledged secondary session was not committed')
        stop(crash=True)
        before = len(hits)
        main, secondary = start()
        time.sleep(2)
        assert '/two' not in hits[before:], hits[before:]
        recovered = secondary.command('obychnye_spisok', id=main_id)
        assert len(recovered['vkladki']) == 3
        assert next(t for t in recovered['vkladki'] if t['id'] == recovered['aktivnaya'])['route'] == {'kind': 'direct'}
        checks.append({'actual_sigkill_restart_restores_independent_window_tabs_routes_lazily': True})
        # Application-wide quit keeps open windows, unlike explicit close.
        stop(crash=False)
        assert len(json.loads((data / 'windows-recovery.json').read_text())['windows']) == 1
        checks.append({'graceful_app_exit_retains_open_window_recovery_snapshot': True})
        main, secondary = start()
        closed_label=next(label for label,client in clients.items() if client is secondary)
        close_owned_secondary(secondary, app, exe, display)
        wait(lambda: json.loads((data / 'windows-recovery.json').read_text())['windows'] == [], 'Explicit window close did not remove its recovery record')
        # Native closure/removal is proved above. Retire only the inspector
        # bookkeeping, so later main-only quit never queries a dead renderer.
        retired=clients.pop(closed_label)
        assert retired is secondary
        try:retired.close()
        except Exception:pass
        checks.append({'explicit_secondary_window_close_commits_removal': True})
        stop(crash=False)
        main, secondary = start(expected=1)
        assert secondary is None
        time.sleep(2)
        assert json.loads((data / 'windows-recovery.json').read_text())['windows'] == []
        checks.append({'closed_window_does_not_resurrect_on_next_start': True})
        stop(crash=False)
        terminal_success = True
    finally:
        if app:
            try:
                stop(crash=True)
            except Exception:
                try:
                    os.killpg(app.pid, signal.SIGKILL)
                except (ProcessLookupError, AttributeError):
                    pass
        if x:
            x.terminate()
            x.wait(timeout=10)
        server.shutdown()
        (out / 'results.json').write_text(json.dumps({'completed': terminal_success and len(checks) == 6, 'checks': checks}, indent=2) + '\n')


if __name__ == '__main__':
    import sys
    if sys.argv[1:]==['--self-test']:
        self_test();raise SystemExit(0)
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True)
    parser.add_argument('--cef', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--display', default=':97')
    args = parser.parse_args()
    run(pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve(), pathlib.Path(args.out).resolve(), args.display)
