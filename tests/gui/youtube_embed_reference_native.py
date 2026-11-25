#!/usr/bin/env python3
"""GHA-only official YouTube embed feasibility probe in ordinary NexusBro CEF.

Diagnostic only, never playback acceptance. Verifies the architectural
hypothesis that the official IFrame Player API (youtube iframe_api + YT.Player)
can start playback in a normal isolated Super tab without the anonymous
extractor. No yt-dlp, no native player private URL, no native IPC from the
frame, no nb-lite CSP/MAIN-grant changes, no owner profile/cookies/auth.

CRITICAL (review KIMI-EMBED-REVIEW): client.js evaluates the TAURI SHELL DOM,
never the CEF page. Therefore:
- the fixture page reports ONLY through HTTP POST /receipt to the own
  loopback fixture server (typed allowlisted payload, <=4096 bytes);
- the trusted click is ONE physical client.mouse at .pole-relative (30,30),
  where the fixture button is absolutely positioned at (16,16) 220x64 solid
  yellow, so the point lands inside the button;
- real OS paint is proven by a yellow pixel observed via ImageGrab in the
  owned Xvfb at that exact screen point, not by DOM geometry.

Official API only: https://www.youtube.com/iframe_api script + YT.Player with
onReady/onStateChange/onError and player.getCurrentTime() on a timer. The
trusted DOM click handler calls player.playVideo() exactly once after onReady.

References:
- https://developers.google.com/youtube/player_parameters (enablejsapi, origin)
- https://support.google.com/youtube/answer/171780 (IFrame Player API)
"""
import argparse
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from origin_request import canonicalize
from youtube_paired_native import (
    Device, switch_super, rows, OwnedWindowManager,
    close_for_privacy, super_record, click_control, Blocked, foreground,
)

# Official IFrame API reference sample; the preceding Big Buck Bunny
# probe returned embed error150. Keep that evidence, do not call it playback.
VIDEO = 'M7lc1UVf-VE'
ALLOWED_STATES = frozenset((-1, 0, 1, 2, 3, 5))
ALLOWED_ERRORS = frozenset((2, 5, 100, 101, 150, 153))
MAX_SAMPLES = 512
MAX_RECEIPT_BYTES = 4096
MIN_ADVANCED_SECONDS = 8
WORK_BUDGET_SECONDS = 180
CLEANUP_BUDGET_SECONDS = 90
# Button geometry contract with the fixture page (absolute, no margins).
BUTTON_LEFT = 16
BUTTON_TOP = 16
BUTTON_WIDTH = 220
BUTTON_HEIGHT = 64
CLICK_OFFSET = (30, 30)
YELLOW = (255, 212, 0)

# External programs the owned fixture stack requires (preflight, fixed codes).
PREFLIGHT_COMMANDS = ('Xvfb', 'xdotool', 'xclip', 'openbox', 'wmctrl', 'xprop', 'xwininfo', 'dbus-run-session')

FIXTURE_HTML = """<!doctype html>
<html><head><meta charset="utf-8"><title>embed probe fixture</title>
<style>
html,body{margin:0;padding:0;background:#123}
#play{position:absolute;left:16px;top:16px;width:220px;height:64px;font-size:24px;
      background:#ffd400;color:#000;border:0;cursor:pointer}
#yt{position:absolute;left:16px;top:96px;width:640px;height:360px;border:0;background:#000}
</style></head>
<body>
<button id="play" type="button">PLAY</button>
<div id="yt"></div>
<script>
(function(){
  var player=null, ready=false, played=false;
  var states=[], errors=[], time=0.0, invalid=false, lastStage='boot';
  function intList(list, value, allowed){
    if(!Number.isInteger(value)){invalid=true;lastStage='invalid_event';return;}
    if(allowed.indexOf(value)<0){invalid=true;lastStage='unexpected_code';return;}
    if(list.length<512&&list.indexOf(value)<0)list.push(value);
  }
  window.__embedApiLoad=function(ok){if(!ready&&!player)lastStage=ok?'api_loaded':'api_load_error';};
  window.addEventListener('error',function(){if(!ready&&lastStage!=='api_load_error')lastStage='api_exception';});
  window.onYouTubeIframeAPIReady=function(){
    lastStage='api_ready';
    player=new YT.Player('yt',{
      videoId:'__VIDEO__',
      playerVars:{enablejsapi:1,origin:location.origin,rel:0,modestbranding:1},
      events:{
        onReady:function(){ready=true;lastStage='ready';},
        onStateChange:function(e){intList(states,e.data,[-1,0,1,2,3,5]);
          if(ready)lastStage='state_'+e.data;},
        onError:function(e){intList(errors,e.data,[2,5,100,101,150,153]);lastStage='error';}
      }});
  };
  document.getElementById('play').addEventListener('click',function(e){
    if(!e.isTrusted||!ready||played||invalid)return;
    played=true;lastStage='play_command';
    player.playVideo();
  });
  setInterval(function(){
    if(player&&ready&&typeof player.getCurrentTime==='function'){
      var t=player.getCurrentTime();
      if(typeof t==='number'&&isFinite(t))time=t;else{invalid=true;lastStage='invalid_clock';}
    }
    var payload={stage:lastStage,ready:ready,played:played,invalid:invalid,
                 states:states,errors:errors,time:Math.round(time*1000)/1000};
    try{navigator.sendBeacon('/receipt',JSON.stringify(payload));}
    catch(_){fetch('/receipt',{method:'POST',body:JSON.stringify(payload)}).catch(function(){});}
  },500);
})();
</script>
<script src="https://www.youtube.com/iframe_api" onload="window.__embedApiLoad(true)" onerror="window.__embedApiLoad(false)"></script>
</body></html>
"""


class Fixture:
    """Own loopback HTTP origin: one page, POST /receipt store, GET /summary read."""

    def __init__(self, video):
        self._template = FIXTURE_HTML
        self._video = video
        self._server = None
        self._thread = None
        self._lock = threading.Lock()
        self._latest = None
        self.page_gets=0;self.receipt_posts=0
        self.origin = None

    def start(self):
        template = self._template
        video = self._video
        lock = self._lock
        owner = self

        def validate(data):
            if not isinstance(data, dict):
                return None
            if set(data) != {'stage', 'ready', 'played', 'invalid', 'states', 'errors', 'time'}:
                return None
            if type(data.get('stage')) is not str or data['stage'] not in {'boot','api_loaded','api_load_error','api_exception','api_ready','ready','play_command','error','invalid_event','unexpected_code','invalid_clock', 'state_-1','state_0','state_1','state_2','state_3','state_5'}:
                return None
            for key in ('ready', 'played', 'invalid'):
                if type(data.get(key)) is not bool:
                    return None
            for key in ('states', 'errors'):
                value = data.get(key)
                if not isinstance(value, list) or len(value) > MAX_SAMPLES:
                    return None
                if any(type(v) is not int or v not in (ALLOWED_STATES if key=='states' else ALLOWED_ERRORS) for v in value):
                    return None
            t = data.get('time')
            if type(t) not in (int, float) or not math.isfinite(t) or not 0 <= t <= 10 ** 7:
                return None
            return data

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                path = canonicalize(self.path).split('?', 1)[0]
                if path == '/summary':
                    with lock:
                        payload = json.dumps(owner._latest).encode('utf-8')
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                    return
                if path != '/embed-probe.html':
                    self.send_response(404); self.end_headers(); return
                with lock:owner.page_gets+=1
                origin = f'http://127.0.0.1:{self.server.server_port}'
                payload = (template.replace('__VIDEO__', video)
                           .replace('__ORIGIN__', origin)).encode('utf-8')
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Content-Length', str(len(payload)))
                self.send_header('Cache-Control', 'no-store')
                self.end_headers()
                self.wfile.write(payload)

            def do_POST(self):
                if canonicalize(self.path).split('?', 1)[0] != '/receipt':
                    self.send_response(404); self.end_headers(); return
                try:
                    length = int(self.headers.get('Content-Length') or 0)
                except ValueError:
                    length = 0
                if not 0 < length <= MAX_RECEIPT_BYTES:
                    self.send_response(413); self.end_headers(); return
                self.connection.settimeout(3)
                try:
                    data = validate(json.loads(self.rfile.read(length).decode('utf-8')))
                except (ValueError, UnicodeError, OSError):
                    data = None
                with lock:
                    owner.receipt_posts+=1
                    owner._latest = {'invalid_receipt': True} if data is None else data
                self.send_response(204); self.end_headers()

            def log_message(self, *args):
                return

        self._server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.origin = f'http://127.0.0.1:{self._server.server_port}'
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self.origin + '/embed-probe.html'

    def summary(self):
        with self._lock:
            return self._latest

    def close(self):
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None


def summarize(receipt, first_time=None):
    """Second allowlist boundary on the test side; only ints/finite clock leave."""
    if not isinstance(receipt, dict) or receipt.get('invalid_receipt') is True:
        raise Blocked('embed_receipt_invalid')
    states, errors = [], []
    for key, allowed in (('states', ALLOWED_STATES), ('errors', ALLOWED_ERRORS)):
        for value in receipt.get(key) or []:
            if type(value) is not int or value not in allowed:
                raise Blocked('embed_unexpected_code')
            if value not in (states if key == 'states' else errors):
                (states if key == 'states' else errors).append(value)
    t = receipt.get('time')
    if type(t) not in (int, float) or not math.isfinite(t):
        raise Blocked('embed_receipt_invalid')
    ready = receipt.get('ready') is True
    played = receipt.get('played') is True
    invalid = receipt.get('invalid') is True
    if invalid:
        raise Blocked('embed_unexpected_code')
    advanced=max(0.0,float(t)-first_time) if first_time is not None else 0.0
    return {'stage': receipt.get('stage'), 'ready': ready, 'played': played,
            'states_seen': states, 'errors_seen': errors,
            'api_clock_seconds':float(t),'api_clock_advanced_seconds':round(advanced,3),
            'playing_observed':bool(ready and played and receipt.get('stage')=='state_1' and not errors and advanced>=MIN_ADVANCED_SECONDS)}


def yellow_observed(device, client, point):
    """Real OS paint proof: yellow button pixel at the owned Xvfb screen point."""
    if device.x is None or device.x.poll() is not None or client.display!=device.display:
        raise Blocked('embed_owned_display_unavailable')
    label = client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    if label!='main':raise Blocked('embed_owner_unavailable')
    origin = client.command('plugin:window|inner_position', label=label)
    scale = client.js('devicePixelRatio||1') or 1
    x = round(origin['x'] + point[0] * scale)
    y = round(origin['y'] + point[1] * scale)
    from PIL import ImageGrab
    with ImageGrab.grab(bbox=(x, y, x + 1, y + 1), xdisplay=device.display) as image:
        r, g, b = image.convert('RGB').getpixel((0, 0))
    return (r,g,b)==YELLOW


def initial_report():
    return {'schema': 1, 'diagnostic_only': True, 'feasibility_probe': True,
            'completed': False, 'video': VIDEO,
            'trusted_play_click': False, 'os_paint_observed': False,
            'ready': False, 'played': False,
            'states_seen': [], 'errors_seen': [], 'api_clock_advanced_seconds': 0.0,
            'playing_observed': False,
            'full_playback_acceptance': False, 'paired': False,
            'official_api_docs': ['https://developers.google.com/youtube/player_parameters',
                                  'https://support.google.com/youtube/answer/171780'],
            'budgets': {'work_seconds': WORK_BUDGET_SECONDS, 'cleanup_seconds': CLEANUP_BUDGET_SECONDS},
            'stage': 'setup', 'cleanup_completed': False}


def run(args):
    report = initial_report(); device = wm = None; root = None; client = None
    fixture = Fixture(VIDEO); close_attempted = False
    out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=False)
    started = time.monotonic(); work_deadline = started + WORK_BUDGET_SECONDS
    def remaining(cap):
        left = work_deadline - time.monotonic()
        if left <= 0: raise Blocked('embed_work_deadline')
        return min(cap, left)
    try:
        if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'linux':
            raise Blocked('gha_linux_only')
        report['stage'] = 'preflight'
        # Fixed preflight codes: a missing system program must read as an
        # environment defect, never as a YouTube availability signal, and the
        # reason carries no str(error)/raw log reflection.
        import shutil
        for command in PREFLIGHT_COMMANDS:
            if shutil.which(command) is None:
                raise Blocked('embed_preflight_missing_' + command)
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
        report['stage'] = 'owned-window-manager'
        wm = OwnedWindowManager(device, root / 'wm'); device.player_wm = wm
        switch_super(client, identifier, timeout=remaining(10))
        if client.js('!!document.querySelector(".pusto button")'):
            click_control(client, '.pusto button', remaining(10))
        click_control(client,'.obychnaya[data-id="1"]',remaining(10))
        report['stage'] = 'tab-live'
        deadline = time.monotonic() + remaining(60); tab = None
        while time.monotonic() < deadline:
            session = rows(client, identifier, timeout=remaining(3)); tab = session.get('aktivnaya')
            if any(v.get('id') == tab and v.get('browser_id', 0) > 0 for v in session.get('vkladki', [])):
                break
            time.sleep(.5)
        else:
            raise Blocked('embed_tab_not_live')
        report['active_fixture_url_matches']=any(v.get('id')==tab and v.get('adres')==url for v in session.get('vkladki',[]))
        report['stage']='official-api-ready'
        deadline=time.monotonic()+remaining(60)
        while time.monotonic()<deadline:
            receipt=fixture.summary()
            report['fixture_http_gets']=fixture.page_gets
            report['fixture_receipt_posts']=fixture.receipt_posts
            if receipt is not None:
                ready_state=summarize(receipt)
                report['fixture_stage']=ready_state['stage']
                report['states_seen']=ready_state['states_seen'];report['errors_seen']=ready_state['errors_seen']
                if ready_state['errors_seen']:
                    raise Blocked('embed_error_'+'_'.join(str(e) for e in ready_state['errors_seen']))
                if ready_state['ready']:
                    report['ready']=True;break
            time.sleep(.25)
        else:raise Blocked('official_api_not_ready')
        # Shell-side only: the CEF area bounds of the active tab.
        point = None
        deadline = time.monotonic() + remaining(20)
        while time.monotonic() < deadline:
            value = client.js('(()=>{const e=document.querySelector(".pole");if(!e)return null;const r=e.getBoundingClientRect();return r.width>300&&r.height>200?[Math.round(r.x+30),Math.round(r.y+30)]:null})()')
            if value:
                point = tuple(value); break
            time.sleep(.25)
        if point is None: raise Blocked('embed_pole_unavailable')
        foreground(device)
        report['stage'] = 'os-paint'
        deadline = time.monotonic() + remaining(30)
        while time.monotonic() < deadline:
            try:
                if yellow_observed(device, client, point):
                    report['os_paint_observed'] = True; break
            except Exception:
                pass
            time.sleep(.4)
        if not report['os_paint_observed']: raise Blocked('embed_button_not_painted')
        report['stage'] = 'trusted-play-click'
        client.mouse(*point)  # exactly one physical click on the yellow button
        report['stage'] = 'observation'
        deadline = time.monotonic() + remaining(120); summary = None; first_time=None; first_wall=None
        while time.monotonic() < deadline:
            receipt = fixture.summary()
            if receipt is not None:
                current=summarize(receipt)
                if current['played'] and first_time is None:
                    first_time=current['api_clock_seconds'];first_wall=time.monotonic()
                summary=summarize(receipt,first_time)
                report.update({key: summary[key] for key in ('ready', 'played', 'states_seen', 'errors_seen', 'api_clock_advanced_seconds', 'playing_observed')})
                report['fixture_stage'] = summary['stage']
                report['trusted_play_click']=summary['played']
                if summary['errors_seen']:
                    raise Blocked('embed_error_' + '_'.join(str(e) for e in summary['errors_seen']))
                if summary['playing_observed'] and first_wall is not None and time.monotonic()-first_wall>=MIN_ADVANCED_SECONDS:
                    break
            time.sleep(.5)
        if summary is None: raise Blocked('embed_receipt_unavailable')
        report['trusted_play_click'] = summary['played']  # receipt written only by the trusted handler
        if not summary['played']: raise Blocked('trusted_click_missing')
        if not summary['playing_observed'] or first_wall is None or time.monotonic()-first_wall<MIN_ADVANCED_SECONDS: raise Blocked('embed_playing_not_observed')
        report['stage'] = 'normal-owned-exit'; close_attempted = True
        close_for_privacy(client, device, report)
        report['completed'] = True; report['stage'] = 'complete'
    except Blocked as error:
        report['blocked_reason'] = str(error)
    except Exception as error:
        report['blocked_reason'] = 'embed_probe_unavailable'
        # Fixed diagnostic fields only. Never publish engine logs/URLs/headers.
        report['exception_type']=type(error).__name__[:64]
        if device is not None:
            report['app_exit_code']=device.app.poll() if device.app is not None else None
            report['xvfb_exit_code']=device.x.poll() if device.x is not None else None
            log=device.root/'app-1.log'
            if log.is_file() and not log.is_symlink():
                with log.open('rb') as stream:
                    stream.seek(max(0,log.stat().st_size-65536));raw=stream.read(65536).lower()
                report['startup_log_flags']={
                    'shared_library_error':b'error while loading shared libraries' in raw,
                    'symbol_lookup_error':b'symbol lookup error' in raw,
                    'display_error':b'cannot open display' in raw,
                    'address_in_use':b'address already in use' in raw,
                    'gtk_error':b'gtk-error' in raw,
                }
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
        if report['wall_seconds']>WORK_BUDGET_SECONDS+CLEANUP_BUDGET_SECONDS:
            report['completed']=False;report['blocked_reason']='embed_total_deadline_exceeded'
        (out / 'result.json').write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    return 0 if report['completed'] else 1


def self_test():
    good = {'stage': 'state_1', 'ready': True, 'played': True, 'invalid': False,
            'states': [3, 1, 1, 2], 'errors': [], 'time': 9.125}
    assert summarize(good,first_time=0.0)['playing_observed'] is True
    assert summarize(good)['playing_observed'] is False  # one absolute clock is not advancing evidence
    idle = {'stage': 'ready', 'ready': True, 'played': False, 'invalid': False,
            'states': [-1], 'errors': [], 'time': 0.5}
    assert summarize(idle)['playing_observed'] is False
    for bad in ({'stage': 'x', 'ready': True, 'played': True, 'invalid': False, 'states': [4], 'errors': [], 'time': 1},
                {'stage': 'x', 'ready': True, 'played': True, 'invalid': False, 'states': [], 'errors': [7], 'time': 1},
                {'stage': 'x', 'ready': True, 'played': True, 'invalid': True, 'states': [], 'errors': [], 'time': 1},
                {'stage': 'x', 'ready': True, 'played': True, 'invalid': False, 'states': [], 'errors': [], 'time': float('nan')},
                {'invalid_receipt': True}):
        try:
            summarize(bad)
        except Blocked:
            pass
        else:
            raise AssertionError('invalid_embed_receipt_accepted')
    assert f'left:{BUTTON_LEFT}px;top:{BUTTON_TOP}px;width:{BUTTON_WIDTH}px;height:{BUTTON_HEIGHT}px' in FIXTURE_HTML
    assert CLICK_OFFSET[0] > BUTTON_LEFT and CLICK_OFFSET[1] > BUTTON_TOP
    assert CLICK_OFFSET[0] < BUTTON_LEFT + BUTTON_WIDTH and CLICK_OFFSET[1] < BUTTON_TOP + BUTTON_HEIGHT
    assert 'iframe_api' in FIXTURE_HTML and 'YT.Player' in FIXTURE_HTML
    assert 'postMessage' not in FIXTURE_HTML  # official API only, no manual protocol
    assert 'playVideo()' in FIXTURE_HTML and FIXTURE_HTML.count('playVideo()') == 1
    import re as _re
    remote = [u for u in _re.findall(r'https://[^\s"\'<>\\)]+', FIXTURE_HTML) if '127.0.0.1' not in u]
    assert remote and all(u == 'https://www.youtube.com' or u.startswith('https://www.youtube.com/')
                          for u in remote), 'fixture must reference only the official youtube origin'
    fixture = Fixture(VIDEO); url = fixture.start()
    assert url.startswith('http://127.0.0.1:') and url.endswith('/embed-probe.html')
    import urllib.request
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        import http.client
        for target in ('/embed-probe.html',fixture.origin+'/embed-probe.html'):
            connection=http.client.HTTPConnection('127.0.0.1',fixture._server.server_port,timeout=3)
            try:
                connection.request('GET',target);response=connection.getresponse()
                assert response.status==200 and b'iframe_api' in response.read()
            finally:connection.close()
        assert fixture.page_gets==2
        for target in ('/receipt',fixture.origin+'/receipt'):
            connection=http.client.HTTPConnection('127.0.0.1',fixture._server.server_port,timeout=3)
            try:
                connection.request('POST',target,body=json.dumps(good).encode());response=connection.getresponse()
                assert response.status==204;response.read()
            finally:connection.close()
            assert fixture.summary()['ready'] is True and fixture.summary()['time']==good['time']
        assert fixture.receipt_posts==2
        bad=dict(good,time=float('nan'))
        request=urllib.request.Request(fixture.origin+'/receipt',data=json.dumps(bad).encode(),method='POST')
        with opener.open(request,timeout=3) as response:assert response.status==204
        assert fixture.summary()=={'invalid_receipt':True}
    finally:fixture.close()
    assert set(PREFLIGHT_COMMANDS) == {'Xvfb', 'xdotool', 'xclip', 'openbox', 'wmctrl', 'xprop', 'xwininfo', 'dbus-run-session'}
    assert initial_report()['full_playback_acceptance'] is False and initial_report()['paired'] is False
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
