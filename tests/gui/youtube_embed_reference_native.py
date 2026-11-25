#!/usr/bin/env python3
"""GHA-only official YouTube embed feasibility probe in ordinary NexusBro CEF.

Diagnostic only, never playback acceptance. Verifies the architectural
hypothesis that the supported IFrame Player API (official embed) can start
playback in a normal isolated Super tab without the anonymous extractor.
No yt-dlp, no native player private URL, no native IPC from the frame,
no nb-lite CSP/MAIN-grant changes, no owner profile/cookies/auth.

Fixture: own loopback HTTP origin serving clean HTML with the official
https://www.youtube.com/embed/<video> iframe (enablejsapi=1&origin=<fixture>)
and one contrast Play button. One physical trusted click -> the fixture's
own DOM handler commands playVideo through the documented postMessage
IFrame API (no remote iframe_api script is required for this). Only
allowlisted integers cross the page boundary: states {-1,0,1,2,3,5},
errors {2,5,100,101,150,153}; plus advancing currentTime seconds.
Any ad wall / unknown code / refused embed -> blocked, never playback.

Primary references (official docs):
- https://developers.google.com/youtube/player_parameters (enablejsapi, origin)
- https://support.google.com/youtube/answer/171780 (IFrame Player API,
  postMessage command protocol incl. playVideo without the api script)
"""
import argparse
import json
import os
import shutil
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from youtube_paired_native import (
    Device, switch_super, rows, route_file, OwnedWindowManager,
    close_for_privacy, super_record, call, click_control, Blocked, foreground,
)

VIDEO = 'aqz-KE-bpKQ'
ALLOWED_STATES = frozenset((-1, 0, 1, 2, 3, 5))
ALLOWED_ERRORS = frozenset((2, 5, 100, 101, 150, 153))
MIN_ADVANCED_SECONDS = 8
WORK_BUDGET_SECONDS = 180
CLEANUP_BUDGET_SECONDS = 90

FIXTURE_HTML = """<!doctype html>
<html><head><meta charset="utf-8"><title>embed probe fixture</title>
<style>
body{margin:0;background:#123;color:#fff;font:16px sans-serif}
#play{display:block;width:220px;height:64px;margin:16px;font-size:24px;background:#ffd400;color:#000;border:4px solid #000;cursor:pointer}
#yt{width:640px;height:360px;border:0;background:#000;margin:0 16px 16px}
</style></head>
<body>
<button id="play" type="button">PLAY</button>
<iframe id="yt" src="https://www.youtube.com/embed/__VIDEO__?enablejsapi=1&amp;origin=__ORIGIN__&amp;rel=0&amp;modestbranding=1"
        allow="autoplay; encrypted-media; picture-in-picture"></iframe>
<script>
(function(){
  var yt=document.getElementById('yt');
  var out={trusted_click:false,states:[],errors:[],times:[]};
  window.__embed=out;
  function send(payload){yt.contentWindow.postMessage(JSON.stringify(payload),'https://www.youtube.com');}
  yt.addEventListener('load',function(){send({event:'listening'});});
  setInterval(function(){send({event:'listening'});},1000);
  setInterval(function(){send({event:'command',func:'getCurrentTime',args:[]});},500);
  document.getElementById('play').addEventListener('click',function(e){
    if(!e.isTrusted)return;
    out.trusted_click=true;
    send({event:'command',func:'playVideo',args:[]});
  });
  window.addEventListener('message',function(e){
    if(e.origin!=='https://www.youtube.com')return;
    var d;try{d=JSON.parse(e.data);}catch(_){return;}
    if(d.event==='onStateChange')out.states.push(d.info|0);
    else if(d.event==='onError')out.errors.push(d.info|0);
    else if(d.event==='infoDelivery'&&d.info&&typeof d.info.currentTime==='number')out.times.push(d.info.currentTime);
  });
})();
</script>
</body></html>
"""


class Fixture:
    """Own loopback HTTP origin serving exactly one page; logs nothing."""

    def __init__(self, video):
        self.origin = None
        self._template = FIXTURE_HTML
        self._video = video
        self._server = None
        self._thread = None

    def start(self):
        template = self._template
        video = self._video

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path.split('?', 1)[0] != '/embed-probe.html':
                    self.send_response(404); self.end_headers(); return
                origin = f'http://127.0.0.1:{self.server.server_port}'
                payload = (template.replace('__VIDEO__', video)
                           .replace('__ORIGIN__', origin)).encode('utf-8')
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Content-Length', str(len(payload)))
                self.send_header('Cache-Control', 'no-store')
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                return

        self._server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.origin = f'http://127.0.0.1:{self._server.server_port}'
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self.origin + '/embed-probe.html'

    def close(self):
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None


def filter_allowlisted(values, allowed):
    if not isinstance(values, list):
        raise Blocked('embed_receipt_invalid')
    seen = []
    for value in values:
        if type(value) is not int or value not in allowed:
            raise Blocked('embed_unexpected_code')
        if value not in seen:
            seen.append(value)
    return seen


def summarize(raw):
    """Allowlist boundary: only documented integers and advancing clock leave the page."""
    if not isinstance(raw, dict) or type(raw.get('trusted_click')) is not bool:
        raise Blocked('embed_receipt_invalid')
    trusted = raw.get('trusted_click')
    states = filter_allowlisted(raw.get('states'), ALLOWED_STATES)
    errors = filter_allowlisted(raw.get('errors'), ALLOWED_ERRORS)
    times = raw.get('times')
    if type(times) is not list or any(type(t) not in (int, float) or t < 0 or t > 10 ** 7 for t in times):
        raise Blocked('embed_receipt_invalid')
    advanced = round(max(times) - min(times), 1) if len(times) >= 2 else 0.0
    return {'trusted_play_click': trusted is True, 'states_seen': states, 'errors_seen': errors,
            'api_clock_advanced_seconds': advanced,
            'playing_observed': 1 in states and advanced >= MIN_ADVANCED_SECONDS}


def initial_report():
    return {'schema': 1, 'diagnostic_only': True, 'feasibility_probe': True,
            'completed': False, 'video': VIDEO,
            'trusted_play_click': False, 'states_seen': [], 'errors_seen': [],
            'api_clock_advanced_seconds': 0.0, 'playing_observed': False,
            'full_playback_acceptance': False, 'paired': False,
            'official_api_docs': ['https://developers.google.com/youtube/player_parameters',
                                  'https://support.google.com/youtube/answer/171780'],
            'budgets': {'work_seconds': WORK_BUDGET_SECONDS, 'cleanup_seconds': CLEANUP_BUDGET_SECONDS},
            'stage': 'setup', 'cleanup_completed': False}


def run(args):
    report = initial_report(); device = wm = None; root = None; client = None
    fixture = Fixture(VIDEO); closed = False; close_attempted = False
    out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    started = time.monotonic(); work_deadline = started + WORK_BUDGET_SECONDS
    def remaining(cap):
        left = work_deadline - time.monotonic()
        if left <= 0: raise Blocked('embed_work_deadline')
        return min(cap, left)
    try:
        if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'linux':
            raise Blocked('gha_linux_only')
        root = Path(tempfile.mkdtemp(prefix='nb-embed-')).resolve(strict=True)
        exe = Path(args.exe).resolve(strict=True); cef = Path(args.cef).resolve(strict=True)
        url = fixture.start()
        device = Device('embed-probe', root / 'device', exe, cef)
        identifier = 'embed-probe'
        record = super_record(identifier, 'Official embed reference', url, proxy='')
        folder = device.data / 'profili' / identifier; folder.mkdir(parents=True)
        (folder / 'obychnye.json').write_text(json.dumps({'vkladki': [{'id': 1, 'adres': url, 'zagolovok': 'Embed probe', 'otlozhena': True}], 'aktivnaya': 1, 'sleduyushchaya': 2}))
        for name in ('vkladki.json', 'sohranennye.json'):
            (device.data / name).write_text(json.dumps([record]))
        (device.data / 'nastrojki.json').write_text(json.dumps({'avtoobnovleniya': False, 'apparatnoe_uskorenie': False, 'memory_saver': False}))
        report['stage'] = 'startup'; client = device.start()
        if time.monotonic() - started > 45: raise Blocked('embed_startup_deadline')
        wm = OwnedWindowManager(device, root / 'wm'); device.player_wm = wm
        switch_super(client, identifier, timeout=remaining(10))
        if client.js('!!document.querySelector(".pusto button")'):
            click_control(client, '.pusto button', remaining(10))
        report['stage'] = 'fixture-load'
        deadline = time.monotonic() + remaining(60); tab = None
        while time.monotonic() < deadline:
            session = rows(client, identifier, timeout=remaining(3)); tab = session.get('aktivnaya')
            if any(v.get('id') == tab and v.get('browser_id', 0) > 0 for v in session.get('vkladki', [])):
                break
            time.sleep(.5)
        else:
            raise Blocked('embed_tab_not_live')
        # Real paint of the contrast button in the owned viewport.
        deadline = time.monotonic() + remaining(20); painted = False
        while time.monotonic() < deadline and not painted:
            painted = client.js('(()=>{const b=document.getElementById("play");return !!b&&b.getBoundingClientRect().width>0&&document.readyState==="complete"})()') is True
            time.sleep(.25)
        if not painted: raise Blocked('embed_button_not_painted')
        report['stage'] = 'trusted-play-click'
        foreground(device)
        click_control(client, '#play', remaining(10))  # one physical trusted click
        report['stage'] = 'observation'
        deadline = time.monotonic() + remaining(120); summary = None
        while time.monotonic() < deadline:
            raw = client.js('window.__embed||null')
            if raw:
                summary = summarize(raw)
                report.update(summary)
                if summary['errors_seen']:
                    raise Blocked('embed_error_' + '_'.join(str(e) for e in summary['errors_seen']))
                if summary['playing_observed']:
                    break
            time.sleep(.5)
        if summary is None: raise Blocked('embed_receipt_unavailable')
        if not summary['trusted_play_click']: raise Blocked('trusted_click_missing')
        if not summary['playing_observed']: raise Blocked('embed_playing_not_observed')
        report['stage'] = 'normal-owned-exit'; close_attempted = True
        close_for_privacy(client, device, report); closed = True
        report['completed'] = True; report['stage'] = 'complete'
    except Blocked as error:
        report['blocked_reason'] = str(error)
    except Exception:
        report['blocked_reason'] = 'embed_probe_unavailable'
    finally:
        if client is not None and not close_attempted and device is not None and device.app is not None:
            prior = report['stage']
            try:
                close_attempted = True; close_for_privacy(client, device, report)
            except Exception:
                report['normal_exit_incomplete'] = True
            report['stage'] = prior
        cleanup = True
        fixture.close()
        for obj in (device, wm):
            if obj:
                try:
                    obj.close()
                except Exception:
                    cleanup = False
        if root:
            try:
                shutil.rmtree(root)
            except OSError:
                cleanup = False
        receipt = report.get('app_shutdown_receipt')
        if isinstance(receipt, dict):
            report['app_shutdown_receipt'] = {key: receipt[key] for key in
                ('schema', 'complete', 'owners', 'clean', 'forced', 'unacknowledged', 'unclean', 'pending_starts')
                if key in receipt}
        report['cleanup_completed'] = cleanup
        if not cleanup: report['completed'] = False; report['blocked_reason'] = 'cleanup_failed'
        if not report.get('owned_exit', {}).get('clean_shutdown_receipt_verified'): report['completed'] = False
        report['wall_seconds'] = round(time.monotonic() - started, 3)
        (out / 'result.json').write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    return 0 if report['completed'] else 1


def self_test():
    assert summarize({'trusted_click': True, 'states': [3, 1, 1, 2], 'errors': [], 'times': [0.0, 4.2, 9.1]}) == {
        'trusted_play_click': True, 'states_seen': [3, 1, 2], 'errors_seen': [],
        'api_clock_advanced_seconds': 9.1, 'playing_observed': True}
    assert summarize({'trusted_click': False, 'states': [-1], 'errors': [], 'times': [1.0]})['playing_observed'] is False
    for bad in ({'trusted_click': True, 'states': [4], 'errors': [], 'times': []},
                {'trusted_click': True, 'states': [], 'errors': [7], 'times': []},
                {'trusted_click': True, 'states': [], 'errors': [], 'times': ['x']},
                {'states': [], 'errors': [], 'times': []}):
        try:
            summarize(bad)
        except Blocked:
            pass
        else:
            raise AssertionError('invalid_embed_receipt_accepted')
    assert FIXTURE_HTML.count('https://www.youtube.com/embed/') == 1
    assert 'enablejsapi=1' in FIXTURE_HTML and '__ORIGIN__' in FIXTURE_HTML
    assert 'iframe_api' not in FIXTURE_HTML  # postMessage protocol needs no remote script
    import re as _re
    remote = [u for u in _re.findall(r'https://[^\s"\'<>\\)]+', FIXTURE_HTML) if '127.0.0.1' not in u]
    assert remote and all(u == 'https://www.youtube.com' or u.startswith('https://www.youtube.com/')
                          for u in remote), 'fixture must reference only the official youtube origin'
    probe = Fixture(VIDEO); url = probe.start(); probe.close()
    assert url.startswith('http://127.0.0.1:') and url.endswith('/embed-probe.html')
    report = initial_report()
    assert report['full_playback_acceptance'] is False and report['paired'] is False
    return {'self_test': True, 'source_only': True, 'native_execution': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('exe', 'cef', 'out'):
        parser.add_argument('--' + key)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        print(json.dumps(self_test())); raise SystemExit(0)
    if not all(getattr(args, key) for key in ('exe', 'cef', 'out')):
        parser.error('Required arguments missing: --exe --cef --out')
    raise SystemExit(run(args))
