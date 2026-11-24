#!/usr/bin/env python3
"""Real two-hour, runner-only resource experiment; never a release publisher.

Only saved fixtures are seeded. Native UI wakes pages; lifecycle IPC is read
only. A short diagnostic pilot can complete but can NEVER pass acceptance.
"""
import argparse
import collections
import datetime
import hashlib
import http.server
import io
import json
import math
import os
import pathlib
import platform
import re
import statistics
import struct
import subprocess
import sys
import threading
import time
import urllib.parse
import wave

import psutil
from security_native import Device, call, js_wait, painted, require, super_record, wait_for
from tab_routes import WindowsDevice, click_control, switch_super, rows, painted_interaction_point

BASELINE_SOURCE = '35ee6500c134d7bcec42bfca9b3f6127a8208a1b'
PHASES = {'Running', 'Preparing', 'Stopping', 'Sleeping', 'Starting'}


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def lifecycle(value):
    """Validate future readonly API strictly; never infer ACKs from a process."""
    require(isinstance(value, dict) and value.get('schema') == 1, 'Lifecycle schema is not 1')
    engines = value.get('engines')
    require(isinstance(engines, list) and len(engines) <= 8, 'Lifecycle engines list outside fixture bounds')
    seen = set()
    clean = []
    for row in engines:
        require(isinstance(row, dict), 'Lifecycle engine row invalid')
        identifier = row.get('id')
        require(identifier in {'soak-' + str(i) for i in range(8)} and identifier not in seen,
                'Lifecycle owner missing, duplicated or outside fixture')
        seen.add(identifier)
        require(row.get('phase') in PHASES, 'Lifecycle phase invalid')
        for key in ('generation', 'attempt', 'tab_count', 'live_count'):
            require(type(row.get(key)) is int and row[key] >= 0, 'Lifecycle numeric field invalid')
        require(row['tab_count'] == 125 and row['live_count'] <= 3, 'Lifecycle tab counts outside workload')
        require(type(row.get('profile_reserved')) is bool and type(row.get('lease_confirmed')) is bool,
                'Lifecycle lease fields invalid')
        pid = row.get('pid')
        require(pid is None or type(pid) is int and pid > 0, 'Lifecycle PID invalid')
        require(row.get('reason') is None or isinstance(row['reason'], str), 'Lifecycle reason invalid')
        if row['phase'] == 'Sleeping':
            require(pid is None and not row['profile_reserved'] and not row['lease_confirmed'],
                    'Sleeping row retains an OS/profile lease')
        if row['phase'] in ('Running', 'Preparing', 'Stopping'):
            require(row['profile_reserved'] and row['lease_confirmed'] and pid is not None,
                    'Live lifecycle phase has no confirmed lease')
        clean.append({key: row[key] for key in ('id', 'phase', 'generation', 'attempt',
                                               'profile_reserved', 'lease_confirmed', 'pid', 'tab_count', 'live_count')})
    return clean


class Fixture:
    def __init__(self):
        self.lock = threading.Lock()
        self.hits = collections.Counter()
        self.media = collections.deque(maxlen=1500)
        audio = io.BytesIO()
        with wave.open(audio, 'wb') as wav:
            wav.setnchannels(1); wav.setsampwidth(2); wav.setframerate(8000)
            wav.writeframes(b''.join(struct.pack('<h', int(8 * math.sin(i / 30))) for i in range(16000)))
        sound = audio.getvalue()
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                parsed = urllib.parse.urlsplit(self.path)
                match = re.fullmatch(r'/page/([0-7])/(\d+)', parsed.path)
                if parsed.path == '/media-proof':
                    query = urllib.parse.parse_qs(parsed.query)
                    try:
                        position = float(query['position'][0]); duration = float(query['duration'][0])
                        elapsed = float(query['played'][0]); paused = query['paused'][0] == 'true'
                        require(all(math.isfinite(v) and v >= 0 for v in (position, duration, elapsed)), 'Media number invalid')
                        with owner.lock:
                            owner.media.append({'at': time.monotonic(), 'position': position,
                                                'duration': duration, 'played': elapsed, 'paused': paused,
                                                'trusted': query.get('trusted') == ['true']})
                        body = b'ok'
                    except (KeyError, ValueError, AssertionError):
                        self.send_error(400); return
                    content_type = 'text/plain'
                elif parsed.path == '/audio.wav':
                    body, content_type = sound, 'audio/wav'
                elif match and int(match[2]) < 125:
                    s, t = map(int, match.groups())
                    with owner.lock:
                        owner.hits[(s, t)] += 1
                    if (s, t) == (0, 0):
                        body = b'''<!doctype html><meta charset="utf-8"><style>body{margin:0;background:#185541}button{position:fixed;left:0;top:0;width:130px;height:70px;background:rgb(123,31,162);border:0;color:white}button span{position:absolute;left:5px;bottom:3px;font:12px sans-serif}</style><button id="start"><span>Start media</span></button><script>
let a=null,trusted=false,played=0,last=0;function report(){if(!a)return;let n=a.currentTime;if(!a.paused){let d=n-last;if(d<0)d+=a.duration;if(d>=0&&d<4)played+=d;}last=n;fetch('/media-proof?'+new URLSearchParams({position:n,duration:Number.isFinite(a.duration)?a.duration:0,played,paused:a.paused,trusted}),{cache:'no-store'}).catch(()=>{});}
start.onclick=async e=>{if(!e.isTrusted||a)return;trusted=true;a=new Audio('/audio.wav');a.loop=true;a.volume=.01;await a.play();setInterval(report,1000);report();};</script>'''
                    else:
                        body = b'<!doctype html><meta charset="utf-8"><style>body{height:5000px;background:#185541;color:white}</style><h1>Quiet resource fixture</h1>'
                    content_type = 'text/html'
                else:
                    self.send_error(404); return
                self.send_response(200)
                self.send_header('Content-Type', content_type)
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                try:
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown(); self.server.server_close(); self.thread.join(timeout=5)


def foreground(device):
    """Foreground the exact inspector owner, not whichever restored view is top."""
    client = device.client
    label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    require(label == 'main', 'Soak inspector is not main shell')
    title = call(client, 'plugin:window|title', label=label)
    owned = {device.app.pid} | {p.pid for p in psutil.Process(device.app.pid).children(recursive=True)}
    if sys.platform == 'win32':
        from pywinauto import Desktop
        windows = [w for w in Desktop(backend='uia').windows(visible_only=True)
                   if w.process_id() in owned and w.window_text() == title]
        require(len(windows) == 1, 'Soak native shell is ambiguous')
        windows[0].restore(); windows[0].set_focus()
        owner_pid = windows[0].process_id()
    else:
        env = {**os.environ, 'DISPLAY': device.display}
        native = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--name', '^' + re.escape(title) + '$'], env=env, text=True).splitlines()
        require(len(native) == 1, 'Soak native shell is ambiguous')
        pid = int(subprocess.check_output(['xdotool', 'getwindowpid', native[0]], env=env, text=True))
        require(pid in owned, 'Soak shell outside owned process tree')
        owner_pid = pid
        subprocess.run(['xdotool', 'windowraise', native[0], 'windowfocus', native[0]], env=env, check=True, timeout=5)
    painted(client)
    return owner_pid


class Sampler:
    def __init__(self, root_pid, destination, memory_cap):
        self.root = psutil.Process(root_pid)
        self.identity = (self.root.pid, self.root.create_time())
        self.destination, self.cap = destination, memory_cap
        self.previous = {}
        self.samples = []
        self.stop = threading.Event()
        self.error = None
        self.topology = None
        self.guard = threading.Lock()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def sample(self):
        require((self.root.pid, self.root.create_time()) == self.identity and self.root.is_running(), 'Soak app root exited')
        totals = collections.Counter()
        parent = None
        for process in [self.root] + self.root.children(recursive=True):
            try:
                identity = (process.pid, process.create_time())
                memory = process.memory_info()
                cpu = process.cpu_times()
                private = getattr(memory, 'private', None)
                if private is None:
                    private = process.memory_full_info().uss
                row = {'cpu_seconds': cpu.user + cpu.system, 'rss_bytes': memory.rss, 'private_bytes': private}
                previous = self.previous.get(identity, row['cpu_seconds'])
                self.previous[identity] = row['cpu_seconds']
                totals.update({'processes': 1, 'cpu_seconds_delta': max(0, row['cpu_seconds'] - previous),
                               'rss_bytes': row['rss_bytes'], 'private_bytes': row['private_bytes']})
                if identity == self.identity:
                    parent = row
            except psutil.NoSuchProcess:
                continue
        require(parent is not None, 'Soak app root not measurable')
        require(totals['processes'] <= 128 and totals['rss_bytes'] <= self.cap, 'Soak load guard exceeded')
        with self.guard:
            topology = self.topology
        return {'elapsed_seconds': round(time.monotonic() - self.started, 3),
                'owned': dict(totals), 'parent': parent, 'topology': topology}

    def run(self):
        self.started = time.monotonic()
        try:
            with self.destination.open('w') as stream:
                while not self.stop.is_set():
                    row = self.sample(); self.samples.append(row)
                    stream.write(json.dumps(row, separators=(',', ':')) + '\n'); stream.flush()
                    self.stop.wait(5)
        except Exception as error:
            self.error = type(error).__name__
            self.stop.set()

    def finish(self):
        self.stop.set(); self.thread.join(timeout=15)
        require(not self.thread.is_alive(), 'Sampler failed to stop')


def aggregates(samples):
    buckets = collections.defaultdict(list)
    for row in samples:
        buckets[int(row['elapsed_seconds'] // 60)].append(row)
    result = []
    for minute, rows_ in sorted(buckets.items()):
        seconds = rows_[-1]['elapsed_seconds'] - rows_[0]['elapsed_seconds']
        result.append({'minute': minute, 'sample_count': len(rows_),
                       'cpu_core_percent': round(100 * sum(r['owned']['cpu_seconds_delta'] for r in rows_[1:]) / seconds, 3) if seconds > 0 else None,
                       'peak_owned_rss_bytes': max(r['owned']['rss_bytes'] for r in rows_),
                       'peak_owned_private_bytes': max(r['owned']['private_bytes'] for r in rows_),
                       'peak_processes': max(r['owned']['processes'] for r in rows_),
                       'median_parent_private_bytes': statistics.median(r['parent']['private_bytes'] for r in rows_)})
    return result


def drift(samples):
    groups = collections.defaultdict(list)
    for row in samples:
        if row['topology'] is not None:
            groups[tuple(row['topology']) + (row['owned']['processes'],)].append(row)
    eligible = [rows_ for rows_ in groups.values() if len(rows_) >= 120 and rows_[-1]['elapsed_seconds'] - rows_[0]['elapsed_seconds'] >= 1800]
    if not eligible:
        return {'comparable': False}
    group = max(eligible, key=len)
    first, last = group[:60], group[-60:]
    initial = statistics.median(r['parent']['private_bytes'] for r in first)
    final = statistics.median(r['parent']['private_bytes'] for r in last)
    hours = (statistics.median(r['elapsed_seconds'] for r in last) - statistics.median(r['elapsed_seconds'] for r in first)) / 3600
    return {'comparable': True, 'matching_topology_samples': len(group), 'span_hours': round(hours, 4),
            'parent_private_delta_bytes': final - initial,
            'parent_private_slope_bytes_per_hour': round((final - initial) / hours),
            'first_parent_private_median_bytes': initial, 'last_parent_private_median_bytes': final}


def run(args):
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Soak runs only on disposable GitHub runners')
    require(not os.environ.get('NEXUSBRO_TEST_MEMORY_IDLE_SECONDS'), 'Accelerated idle environment is forbidden in resource soak')
    require(re.fullmatch('[0-9a-f]{40}', args.source) is not None, 'Expected immutable source SHA required')
    require(re.fullmatch('[0-9a-f]{40}', args.test_source) is not None, 'Test source SHA required')
    if args.baseline:
        require(args.source == BASELINE_SOURCE and args.build_run == '45371512784', 'Only explicit 0.2.26 baseline is supported')
    out = pathlib.Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    report = {'schema': 1, 'completed': False, 'acceptance_passed': False, 'baseline': args.baseline,
              'diagnostic_pilot': args.diagnostic_pilot, 'hibernation_acceptance': False,
              'source': args.source, 'test_source': args.test_source, 'build_run': args.build_run,
              'platform': sys.platform, 'runner_system': platform.system(), 'logical_cpus': psutil.cpu_count(),
              'private_metric': 'windows_private_commit' if sys.platform == 'win32' else 'linux_uss',
              'workload': {'supers': 8, 'saved_per_super': 125, 'saved_total': 1000, 'visited_limit': 24},
              'coverage': {'writer_overlap': 'readonly_lease_plus_sampled_OS_identity_not_exhaustive', 'session_cookie_roundtrip': 'separate_fast_native_gate_required'},
              'drift_limits': {'parent_delta_mb': args.parent_delta_mb, 'parent_slope_mb_per_hour': args.parent_slope_mb_hour},
              'checks': [], 'requested_seconds': args.seconds if args.diagnostic_pilot else 7200}
    fixture = device = sampler = None
    code = 1
    stage = 'setup'
    try:
        exe, cef = pathlib.Path(args.exe).resolve(), pathlib.Path(args.cef).resolve()
        require(exe.is_file() and cef.is_dir(), 'Native executable/CEF unavailable')
        report['exe_sha256'] = sha256(exe)
        report['package_sha256'] = args.package_sha256
        require(re.fullmatch('[0-9a-f]{64}', args.package_sha256) is not None, 'Package SHA256 required')
        fixture = Fixture()
        base = 'http://127.0.0.1:' + str(fixture.server.server_port)
        owned_root = out / 'private-device'
        if sys.platform == 'win32':
            device = WindowsDevice(owned_root, exe, cef)
        else:
            device = Device('soak', owned_root, exe, cef)
        definitions = []
        for index in range(8):
            identifier = 'soak-' + str(index)
            record = super_record(identifier, 'SoakSuper' + str(index), base + '/page/' + str(index) + '/0')
            record['poryadok'] = index; definitions.append(record)
            profile = device.data / 'profili' / identifier; profile.mkdir(parents=True)
            tabs = [{'id': tab + 1, 'adres': base + '/page/' + str(index) + '/' + str(tab),
                     'otlozhena': True, 'zagolovok': 'Soak ' + str(index) + '-' + str(tab)} for tab in range(125)]
            (profile / 'obychnye.json').write_text(json.dumps({'vkladki': tabs, 'aktivnaya': 1, 'sleduyushchaya': 126}), encoding='utf-8')
        (device.data / 'vkladki.json').write_text(json.dumps(definitions), encoding='utf-8')
        (device.data / 'nastrojki.json').write_text(json.dumps({'avtoobnovleniya': False, 'apparatnoe_uskorenie': False}), encoding='utf-8')
        client = device.start()
        # WindowsDevice returns the inspector; both implementations also retain it.
        require(client is not None, 'Owned app inspector unavailable')
        time.sleep(2)
        policy = call(client, 'nastrojki_poluchit')
        require(policy.get('memory_saver', True) is True and policy.get('memory_saver_minutes', 30) == 30, 'Normal automatic30min policy differs')
        report['normal_idle_policy_minutes'] = 30
        stage = 'native-ui-warmup'
        initial_available = psutil.virtual_memory().available
        memory_cap = min(args.max_memory_mb * 1024**2, int(initial_available * .75))
        def warmup_load_guard():
            require(device.app.poll() is None, 'Owned app exited during warmup')
            processes = [psutil.Process(device.app.pid)]
            processes += processes[0].children(recursive=True)
            rss = 0
            for process in processes:
                try:
                    rss += process.memory_info().rss
                except psutil.NoSuchProcess:
                    pass
            require(len(processes) <= 128 and rss <= memory_cap, 'Warmup load guard exceeded')
        require(not fixture.hits, 'Cold saved fixtures caused unsolicited navigation')
        if not args.baseline:
            lifecycle(call(client, 'super_lifecycle_snapshot'))
        for index in range(8):
            warmup_load_guard()
            foreground(device); switch_super(client, 'soak-' + str(index))
            for tab in range(3):
                warmup_load_guard()
                foreground(device)
                click_control(client, '.obychnaya[data-id="' + str(tab + 1) + '"]', 30)
                wait_for(lambda: fixture.hits[(index, tab)] > 0, 'Native physical tab never reached fixture', 30)
                def selected_loaded():
                    session = rows(client, 'soak-' + str(index))
                    target = [r for r in session['vkladki'] if r['id'] == tab + 1]
                    return (session['aktivnaya'] == tab + 1 and len(target) == 1
                            and target[0]['adres'] == base + '/page/' + str(index) + '/' + str(tab)
                            and not target[0].get('otlozhena') and not target[0].get('zagruzka'))
                wait_for(selected_loaded, 'Actual selected native page did not settle', 30)
                if (index, tab) == (0, 0):
                    device.client.mouse(*painted_interaction_point(client))
                    wait_for(lambda: fixture.media and fixture.media[-1]['trusted'] and not fixture.media[-1]['paused'], 'Trusted native media did not start', 20)
        foreground(device); switch_super(client, 'soak-1')
        expected = {(s, t) for s in range(8) for t in range(3)}
        require(set(fixture.hits) == expected, 'Visited/deferred fixture set differs from 24/976')
        initial_sessions = [rows(client, 'soak-' + str(i)) for i in range(8)]
        require(sum(len(r['vkladki']) for r in initial_sessions) == 1000 and
                sum(not t.get('otlozhena') for r in initial_sessions for t in r['vkladki']) == 24,
                'Native rows disagree with the 24-live/976-deferred warmup')
        warmup_load_guard()
        report['checks'].append('native_ui_24_visited_1000_saved_976_deferred_with_real_media')
        # Setup is excluded. Full acceptance measures two actual hours below.
        measured = time.monotonic(); duration = args.seconds if args.diagnostic_pilot else 7200
        report['started_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        report['load_guard'] = {'max_processes': 128, 'max_owned_rss_bytes': memory_cap}
        parent_pid = foreground(device)
        sampler = Sampler(parent_pid, out / 'samples.jsonl', memory_cap); sampler.thread.start()
        sleepy = set(); cycles = 0; previous = {}; last_generation = {}; event_count = 0
        writer_identities = {}
        next_poll = measured; next_cycle = measured + 3000
        while time.monotonic() - measured < duration:
            now = time.monotonic(); elapsed = now - measured
            stage = 'owned-app-and-load-guard'
            require(device.app.poll() is None, 'Owned application exited during soak')
            require(sampler.error is None, 'Sampler/load guard failed')
            stage = 'continuous-media-and-deferred-network'
            with fixture.lock:
                media = list(fixture.media); visited = set(fixture.hits)
            require(visited == expected, 'A deferred page unexpectedly performed network I/O')
            require(media and now - media[-1]['at'] < 20 and media[-1]['trusted'] and not media[-1]['paused'], 'Continuous media receipt stopped')
            recent = [r for r in media if now - r['at'] < 15]
            require(len(recent) < 3 or recent[-1]['played'] - recent[0]['played'] >= .25, 'Media clock stopped advancing')
            if now >= next_poll:
                stage = 'readonly-lifecycle-sampling'
                if args.baseline:
                    live = call(client, 'spisok')
                    # Existing obychnye_spisok returns parent RAM sessions; it
                    # neither sends an engine command nor updates idle time.
                    sessions = [rows(client, 'soak-' + str(i)) for i in range(8)]
                    saved_count = sum(len(r['vkladki']) for r in sessions)
                    live_tabs = sum(not t.get('otlozhena', False) for r in sessions for t in r['vkladki'])
                    require(saved_count == 1000 and live_tabs <= 24, 'Baseline saved/live topology escaped bounds')
                    topology = (sum(bool(r.get('zapushchena')) for r in live), live_tabs, saved_count)
                else:
                    engines = lifecycle(call(client, 'super_lifecycle_snapshot'))
                    require(len(engines) == 8, 'Lifecycle omitted a seeded Super')
                    counts = collections.Counter(r['phase'] for r in engines)
                    topology = tuple(counts[p] for p in sorted(PHASES)) + (sum(r['live_count'] for r in engines), 1000)
                    pids = [r['pid'] for r in engines if r['pid'] is not None]
                    require(len(set(pids)) == len(pids), 'One native PID claimed by multiple profile owners')
                    for row in engines:
                        key = row['id']; state = (row['phase'], row['generation'], row['attempt'])
                        require(row['generation'] >= last_generation.get(key, 0), 'Lifecycle generation regressed')
                        last_generation[key] = row['generation']
                        if previous.get(key) != state: event_count += 1
                        previous[key] = state
                        old_writer = writer_identities.get(key)
                        if row['pid'] is not None:
                            writer = psutil.Process(row['pid'])
                            identity = (writer.pid, writer.create_time())
                            owned_pids = {sampler.root.pid} | {p.pid for p in sampler.root.children(recursive=True)}
                            require(writer.pid in owned_pids, 'Lifecycle writer outside owned app tree')
                            if old_writer and old_writer != identity:
                                require(not psutil.pid_exists(old_writer[0]) or psutil.Process(old_writer[0]).create_time() != old_writer[1], 'Replacement writer started before old writer exit')
                            writer_identities[key] = identity
                        if row['phase'] == 'Sleeping':
                            if old_writer:
                                require(not psutil.pid_exists(old_writer[0]) or psutil.Process(old_writer[0]).create_time() != old_writer[1], 'Sleeping profile still has old OS writer')
                            if key in ('soak-6', 'soak-7'): sleepy.add(key)
                with sampler.guard: sampler.topology = topology
                next_poll = now + 5
            # Quiet6/7 remain untouched through the real default30min interval.
            # Bounded repeated wake/current selection between50..80min only.
            if 3000 <= elapsed < 4800 and now >= next_cycle:
                stage = 'native-ui-wake-cycle'
                foreground(device); switch_super(client, 'soak-' + str(2 + cycles % 4))
                foreground(device); switch_super(client, 'soak-1')
                cycles += 1; next_cycle = now + 120
            time.sleep(.2)
        elapsed = time.monotonic() - measured
        sampler.finish()
        require(sampler.error is None and len(sampler.samples) >= duration / 5 - 3, 'Sampling duration incomplete')
        report.update(wall_seconds=round(elapsed, 3), ended_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      samples_count=len(sampler.samples), minute_aggregates=aggregates(sampler.samples),
                      drift=drift(sampler.samples), native_ui_cycles=cycles, lifecycle_transitions=event_count,
                      quiet_supers_observed_sleeping=len(sleepy), media_receipt_count=len(fixture.media),
                      media_last_played_seconds=fixture.media[-1]['played'])
        stage = 'two-hour-and-drift-acceptance'
        if not args.diagnostic_pilot:
            require(elapsed >= 7200 and cycles >= 10, 'Two-hour/cycle acceptance incomplete')
            require(report['drift']['comparable'], 'No sufficiently long comparable topology for parent drift')
            require(report['drift']['parent_private_delta_bytes'] <= args.parent_delta_mb * 1024**2 and
                    report['drift']['parent_private_slope_bytes_per_hour'] <= args.parent_slope_mb_hour * 1024**2,
                    'Predeclared parent drift budget exceeded')
            if not args.baseline:
                require(len(sleepy) == 2, 'Two quiet Supers never reached real Sleeping')
                report['hibernation_acceptance'] = True
            report['acceptance_passed'] = True
        report['completed'] = True
        code = 0
    except Exception as error:
        # Exception strings, arbitrary IPC replies and process command lines may
        # include local paths or secrets; publish only fixed failure class.
        report['failure_class'] = type(error).__name__
        report['failure_stage'] = stage
    finally:
        for name, cleanup in [('sampler', sampler.finish if sampler else None),
                              ('device', device.close if device else None),
                              ('fixture', fixture.close if fixture else None)]:
            if cleanup:
                try:
                    cleanup()
                except Exception as error:
                    report.setdefault('cleanup_failure_classes', {})[name] = type(error).__name__
                    report['acceptance_passed'] = False; report['hibernation_acceptance'] = False
                    code = 1
        if sampler and 'minute_aggregates' not in report:
            report['minute_aggregates'] = aggregates(sampler.samples)
        (out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({k: report[k] for k in ('completed', 'acceptance_passed', 'baseline', 'diagnostic_pilot', 'hibernation_acceptance')}))
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('exe', 'cef', 'out', 'source', 'test-source', 'build-run', 'package-sha256'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--baseline', action='store_true')
    parser.add_argument('--diagnostic-pilot', action='store_true')
    parser.add_argument('--seconds', type=int, default=120, help='Only used by explicit diagnostic pilot; acceptance always7200')
    parser.add_argument('--max-memory-mb', type=int, default=8192)
    parser.add_argument('--parent-delta-mb', type=int, default=64)
    parser.add_argument('--parent-slope-mb-hour', type=int, default=32)
    args = parser.parse_args()
    if not 120 <= args.seconds <= 900 or not 1024 <= args.max_memory_mb <= 8192:
        parser.error('Pilot duration or memory cap outside bounded range')
    if not 1 <= args.parent_delta_mb <= 256 or not 1 <= args.parent_slope_mb_hour <= 128:
        parser.error('Drift limits outside reviewable bounded range')
    sys.exit(run(args))


if __name__ == '__main__':
    main()
