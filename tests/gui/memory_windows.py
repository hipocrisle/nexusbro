#!/usr/bin/env python3
"""Real Windows automatic-only tab-unload and continuous-media acceptance; no IPC or CEF/CDP mocks."""
from origin_request import canonicalize
import argparse
import collections
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


def automatic_media_refusals(logs, tab_id, kind):
    """A generic interaction refusal does not prove the media/API hook works."""
    pattern = (rf'NEXUSBRO_MEMORY tab={tab_id}\b[^\n]*?browser=(\d+)\b'
               r'[^\n]*?automatic=true\b[^\n]*?refused=([^\n]+)')
    found = []
    for browser, reason in re.findall(pattern, logs):
        lower = reason.lower()
        shared_api = "media, connection or worker api used" in lower
        specific = (("media" in lower or "audio" in lower or "video" in lower) if kind == "html"
                    else ("audiocontext" in lower or shared_api) if kind == "webaudio"
                    else ("rtc" in lower or "peerconnection" in lower or shared_api))
        if specific and "trusted page interaction" not in lower:
            found.append({"browser_id": int(browser), "reason": reason})
    return found


def run(a):
    if sys.platform != "win32":
        raise RuntimeError("Requires an actual disposable Windows desktop")
    import ctypes
    from ctypes import wintypes
    import psutil
    import pyautogui as gui
    from pywinauto import Desktop
    from comtypes import COMError
    from PIL import ImageGrab
    from performance_windows import role

    out = pathlib.Path(a.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    data = out / "data"
    data.mkdir()
    report = {"schema": 2, "automatic_only": True, "completed": False, "platform": platform.platform(),
              "heap_mb": a.heap_mb, "min_private_drop_mb": a.min_drop_mb,
              "normal_policy_minutes": 30, "isolated_idle_seconds": 8,
              "checks": [], "memory": {}, "coverage": {"beforeunload_refusal": "not_executed",
              "user_close_during_sleep": "not_executed", "automatic_unload": "not_executed",
              "fake_capture": "not_executed_no_fake_capture_interface"}}
    started = time.monotonic()
    hits, receipts = collections.Counter(), []
    lock = threading.Lock()
    release_loading, stop_download = threading.Event(), threading.Event()
    app = log = server = None
    owned = {}
    u = ctypes.windll.user32
    u.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    u.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                               ctypes.c_int, ctypes.c_int, wintypes.UINT]
    u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    # Quiet, real looping PCM media; no network service or device permission.
    wav = io.BytesIO()
    with wave.open(wav, "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(44100)
        one_second = b"".join(struct.pack("<h", round(160 * math.sin(2 * math.pi * 220 * i / 44100))) for i in range(44100))
        # A 120-second loop avoids modulo-30 aliasing a whole background pause.
        audio.writeframes(one_second * 120)
    tone = wav.getvalue()

    class Fixture(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.path = canonicalize(self.path)
            path = urllib.parse.urlsplit(self.path).path
            with lock:
                hits[path] += 1
            if path == "/tone.wav":
                self.send_response(200)
                self.send_header("Content-Type", "audio/wav")
                self.send_header("Content-Length", str(len(tone)))
                self.end_headers()
                self.wfile.write(tone)
                return
            if path == "/download":
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Disposition", 'attachment; filename="memory-fixture.bin"')
                self.send_header("Content-Length", str(32 * 1024**2))
                self.end_headers()
                try:
                    for _ in range(512):
                        if stop_download.is_set():
                            break
                        self.wfile.write(b"x" * 65536)
                        self.wfile.flush()
                        time.sleep(.2)
                except (BrokenPipeError, ConnectionResetError, OSError):
                    pass
                return
            names = {"/unvisited": "MemoryUnvisited", "/keeper": "MemoryKeeper", "/pinned": "MemoryPinned",
                     "/tools": "MemoryTools", "/loading": "MemoryLoading", "/downloads": "MemoryDownload",
                     "/private": "MemoryPrivate", "/unload": "MemoryBeforeUnload", "/draft": "MemoryDraft",
                     "/auto-heap": "MemoryAutoHeap", "/unvisited-other": "MemoryUnvisitedOther", "/audio": "MemoryAudio",
                     "/audio-paused": "MemoryAudioPaused", "/web-audio": "MemoryWebAudio", "/rtc": "MemoryRTC"}
            if path not in names:
                self.send_error(404)
                return
            page = names[path]
            body = (f'<!doctype html><meta charset="utf-8"><title>{page}</title>'
                    '<style>html,body{margin:0;background:#185541;color:white;font:24px sans-serif}'
                    'button{position:absolute;left:20px;top:85px;width:300px;height:36px}</style>'
                    f'<h1>{page}</h1>').encode()
            if path == "/downloads":
                body += b'<button onclick="location.href=\'/download\'">Start actual download</button>'
            if path == "/unload":
                body += b'''<button id="guard">Enable actual beforeunload guard</button><script>
document.getElementById('guard').onclick=e=>{if(e.isTrusted){window.onbeforeunload=e=>{e.preventDefault();e.returnValue='';};
fetch('/receipt',{method:'POST',body:JSON.stringify({page:'MemoryBeforeUnload',guard:true,trusted:e.isTrusted})});}};</script>'''
            if path == "/draft":
                body += b'''<input id="actual-draft" aria-label="Actual unsaved draft" style="position:absolute;left:20px;top:130px;width:300px;height:40px">
<button id="draft-snapshot" style="top:175px">Snapshot actual unsaved draft</button><script>
document.getElementById('draft-snapshot').onclick=e=>{if(e.isTrusted)fetch('/receipt',{method:'POST',body:JSON.stringify({
page:'MemoryDraft',reason:'draft-snapshot',trusted:true,value:document.getElementById('actual-draft').value})});};</script>'''
            if path == "/auto-heap":
                body += ("<style>body{min-height:4000px}#auto-snapshot{position:fixed}</style>"
                         "<button id='auto-snapshot'>Snapshot real scroll/storage</button><script>"
                         "const seen=localStorage.getItem('memory-auto-fixture'),cookie=document.cookie;"
                         "localStorage.setItem('memory-auto-fixture','preserved');"
                         "document.cookie='memoryauto=preserved;path=/;max-age=86400;SameSite=Lax';"
                         f"window.autoHeap=new Uint8Array({a.heap_mb}*1024*1024);autoHeap.fill(41);"
                         "function state(reason,trusted){fetch('/receipt',{method:'POST',body:JSON.stringify({"
                         "page:document.title,reason,trusted,bytes:autoHeap.byteLength,first:autoHeap[0],"
                         "last:autoHeap[autoHeap.length-1],seen,cookie,scroll_y:scrollY})});}"
                         "state('allocated',false);document.getElementById('auto-snapshot').onclick=e=>{"
                         "if(e.isTrusted)state('snapshot',true);};let scrollReports=0;"
                         "addEventListener('scroll',e=>{if(e.isTrusted&&scrollReports++<32)state('native-scroll',true);});</script>").encode()
            if path in ["/audio", "/audio-paused", "/web-audio", "/rtc"]:
                kind = "html" if path.startswith("/audio") else "webaudio" if path == "/web-audio" else "rtc"
                if kind == "html":
                    body += b'<audio id="actual-audio" src="/tone.wav" loop preload="auto"></audio>'
                body += ("<button id='media-start'>Start actual media/resource</button>"
                         "<button id='media-snapshot' style='top:130px'>Snapshot actual resource clock</button>"
                         "<button id='media-pause' style='top:175px'>Pause actual HTML audio</button>"
                         f"<script>const mediaKind={json.dumps(kind)};let context=null,oscillator=null,peer=null;"
                         "const element=document.getElementById('actual-audio');"
                         "function state(reason,trusted,error){const value={page:document.title,reason,trusted,kind:mediaKind};"
                         "if(element)Object.assign(value,{current_time:element.currentTime,duration:element.duration,"
                         "paused:element.paused,ready_state:element.readyState});"
                         "if(context)Object.assign(value,{current_time:context.currentTime,context_state:context.state});"
                         "if(peer)Object.assign(value,{connection_state:peer.connectionState,signaling_state:peer.signalingState});"
                         "if(error)value.error=String(error);fetch('/receipt',{method:'POST',body:JSON.stringify(value)}); }"
                         "document.getElementById('media-start').onclick=async e=>{if(!e.isTrusted)return;try{"
                         "if(element)await element.play();"
                         "else if(mediaKind==='webaudio'){context=new AudioContext();const gain=context.createGain();"
                         "gain.gain.value=.005;oscillator=context.createOscillator();oscillator.frequency.value=220;"
                         "oscillator.connect(gain);gain.connect(context.destination);oscillator.start();await context.resume();}"
                         "else peer=new RTCPeerConnection({iceServers:[]});state('start',true);"
                         "}catch(error){state('start',true,error);}};"
                         "document.getElementById('media-snapshot').onclick=e=>{if(e.isTrusted)state('snapshot',true);};"
                         "document.getElementById('media-pause').onclick=e=>{if(e.isTrusted&&element){element.pause();state('pause',true);}};"
                         "</script>").encode()
            if path not in ["/loading", "/auto-heap"]:
                body += ("<script>fetch('/receipt',{method:'POST',body:JSON.stringify({page:document.title})});</script>").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body) + (4096 if path == "/loading" else 0)))
            self.end_headers()
            self.wfile.write(body)
            self.wfile.flush()
            if path == "/loading":
                release_loading.wait(a.timeout)
                try:
                    self.wfile.write(b" " * 4096)
                except OSError:
                    pass

        def do_POST(self):
            self.path = canonicalize(self.path)
            if self.path != "/receipt":
                self.send_error(404)
                return
            try:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 4096:
                    raise ValueError()
                item = json.loads(self.rfile.read(size))
                if not isinstance(item, dict):
                    raise ValueError()
                with lock:
                    if len(receipts) < 1024:
                        receipts.append({**item, "received_seconds": time.monotonic() - started})
            except (ValueError, TypeError):
                self.send_error(400)
                return
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *_):
            pass

    def checkpoint():
        report["receipts"] = list(receipts)
        report["hits"] = dict(hits)
        (out / "results.json").write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")

    def processes():
        if time.monotonic() - started > a.timeout:
            raise TimeoutError("Memory acceptance overall timeout")
        if app.poll() is not None:
            raise RuntimeError(f"Application exited: {app.returncode}")
        root = psutil.Process(app.pid)
        live = []
        for p in [root, *root.children(recursive=True)]:
            try:
                owned[(p.pid, p.create_time())] = p
                live.append(p)
            except psutil.NoSuchProcess:
                pass
        if len(live) > 96:
            raise RuntimeError("Fixture process count exceeded 96")
        return live

    def wait(predicate, description, seconds=25):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            processes()
            value = predicate()
            if value:
                return value
            time.sleep(.15)
        raise AssertionError(description)

    def session():
        try:
            return json.loads((data / "profili" / "memory" / "obychnye.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return {"vkladki": []}

    def snapshot(tab_ids, predicate=lambda state, rows: True):
        # The persisted session can be temporarily unreadable while native CEF
        # writes it. Read once per observation; never mix rows from different
        # generations or interpret a missing row as a successful negative.
        state = session()
        observed = {}
        for item in state["vkladki"]:
            if item["id"] in observed:
                raise AssertionError("Duplicate tab row appeared")
            observed[item["id"]] = item
        if any(tab_id not in observed for tab_id in tab_ids):
            return None
        return (state, observed) if predicate(state, observed) else None

    def required_snapshot(tab_ids):
        return wait(lambda: snapshot(tab_ids), "Required native session rows were unavailable")

    def memory(label):
        samples = []
        for _ in range(5):
            entries = []
            for p in processes():
                try:
                    with p.oneshot():
                        mem = p.memory_info()
                        entries.append({"pid": p.pid, "created": p.create_time(),
                                        "role": role(p.name(), p.cmdline(), p.pid == app.pid),
                                        "private_bytes": mem.private, "working_set_bytes": mem.rss})
                except psutil.NoSuchProcess:
                    pass
            private = sum(p["private_bytes"] for p in entries)
            if private > 6 * 1024**3:
                raise RuntimeError("Fixture private commit exceeded 6 GiB")
            samples.append({"private_bytes": private, "processes": entries})
            time.sleep(.3)
        result = {"median_private_bytes": statistics.median(s["private_bytes"] for s in samples), "samples": samples}
        report["memory"][label] = result
        checkpoint()
        return result["median_private_bytes"]

    try:
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{server.server_port}"
        tabs = [{"id": i, "adres": base + path, "zagolovok": title, "otlozhena": True,
                 "zakreplena": i in (2, 3), **({"memory_keep": True} if i == 9 else {})} for i, path, title in [
                    (1, "/unvisited", "MemoryUnvisited"), (2, "/keeper", "MemoryKeeper"),
                    (3, "/pinned", "MemoryPinned"), (4, "/tools", "MemoryTools"),
                    (5, "/loading", "MemoryLoading"), (6, "/downloads", "MemoryDownload"),
                    (7, "/unload", "MemoryBeforeUnload"), (8, "/draft", "MemoryDraft"),
                    (9, "/auto-heap", "MemoryAutoHeap"), (10, "/unvisited-other", "MemoryUnvisitedOther"),
                    (11, "/audio", "MemoryAudio"), (12, "/audio-paused", "MemoryAudioPaused"),
                    (13, "/web-audio", "MemoryWebAudio"), (14, "/rtc", "MemoryRTC")]]
        folder = data / "profili" / "memory"
        folder.mkdir(parents=True)
        (folder / "obychnye.json").write_text(json.dumps({"vkladki": tabs, "aktivnaya": 2, "sleduyushchaya": 15}), encoding="utf-8")
        (data / "vkladki.json").write_text(json.dumps([{"id": "memory", "nazvanie": "MemorySuper",
            "proxy": "", "adres": base + "/keeper", "pustaya": False, "rezat_reklamu": True}]), encoding="utf-8")
        (data / "nastrojki.json").write_text(json.dumps({"avtoobnovleniya": False, "memory_saver_minutes": 30}), encoding="utf-8")
        cef = pathlib.Path(a.cef).resolve()
        env = {**os.environ, "CEF_PATH": str(cef), "PATH": str(cef) + os.pathsep + os.environ["PATH"],
               "NEXUSBRO_TEST_DATA": str(data), "WEBVIEW2_USER_DATA_FOLDER": str(out / "webview2"),
               "NEXUSBRO_PROBA": "0", "NEXUSBRO_TEST_DOWNLOAD_DIR": str(out / "downloads"),
               "NEXUSBRO_TEST_MEMORY_IDLE_SECONDS": "8"}
        log = (out / "app.log").open("w", encoding="utf-8")
        app = subprocess.Popen([str(pathlib.Path(a.exe).resolve())], env=env, stdout=log, stderr=log)
        desktop = Desktop(backend="uia")
        win = wait(lambda: next(iter(desktop.windows(process=app.pid, title="NexusBro")), None), "No real shell window", 45)
        u.ShowWindow(win.handle, 9)
        width, height = min(1280, u.GetSystemMetrics(0) - 40), min(820, u.GetSystemMetrics(1) - 60)
        if width < 900 or height < 560 or not u.SetWindowPos(win.handle, None, 20, 20, width, height, 0x40):
            raise RuntimeError("Native test desktop does not fit fixture")
        win.set_focus()

        def controls():
            found = []
            for window in [win, *desktop.windows(process=app.pid, title="NexusBro — меню")]:
                try:
                    found.extend(window.descendants())
                except (COMError, KeyError):
                    continue
            return found

        def find(name, kinds=None):
            for c in controls():
                try:
                    if c.element_info.name == name and (not kinds or c.element_info.control_type in kinds) and c.is_visible():
                        return c
                except COMError:
                    pass
            return None

        def click(name, kinds=None, button="left"):
            c = wait(lambda: find(name, kinds), f"Missing actual UI control: {name}")
            if not c.is_enabled():
                raise AssertionError(f"Actual control disabled: {name}")
            try:
                c.iface_scroll_item.ScrollIntoView()
            except Exception:
                pass
            box = c.rectangle()
            c.click_input(button=button, coords=(min(10, max(2, box.width() // 4)), box.height() // 2))
            return c

        def tab(title):
            click(title, ["TabItem"])

        def navigate(url):
            click("Адрес страницы", ["ComboBox", "Edit"])
            gui.hotkey("ctrl", "a")
            gui.write(url, interval=.002)
            gui.press("enter")

        def physical_page_click(y=103):
            journal = (data / "zhurnal.log").read_text(encoding="utf-8")
            handles = re.findall(r'memory: запуск ок, окно (\d+)', journal)
            if not handles:
                raise AssertionError("No owned CEF container in actual application journal")
            box = wintypes.RECT()
            if not u.GetWindowRect(int(handles[-1]), ctypes.byref(box)):
                raise AssertionError("Actual CEF rectangle missing")
            gui.click(box.left + 100, box.top + y)

        def native_logs():
            return "\n".join(p.read_text(encoding="utf-8", errors="replace") for p in folder.glob("*.log"))

        def automatic_refusals(tab_id):
            return re.findall(rf'NEXUSBRO_MEMORY tab={tab_id}\b[^\n]*?automatic=true\b[^\n]*?refused=([^\n]+)', native_logs())

        def assert_no_memory_menu(title):
            click(title, ["TabItem"], "right")
            wait(lambda: find("Новая вкладка", ["MenuItem", "Button"]), "Actual tab context menu did not open")
            for name in ["Выгрузить вкладку из памяти", "Не выгружать автоматически",
                         "Разрешить автоматическую выгрузку", "Выгрузить"]:
                if find(name, ["MenuItem", "Button"]):
                    raise AssertionError("Removed manual memory control remains visible: " + name)
            gui.press("escape")

        def assert_live(tab_id, observed=None):
            if observed is None:
                _, observed = required_snapshot([tab_id])
            item = observed.get(tab_id)
            if not item or item.get("otlozhena"):
                raise AssertionError(f"Automatically protected tab is not live: {tab_id}")
            return item

        click("MemorySuper", ["Button"])
        tab("MemoryKeeper")
        wait(lambda: any(r.get("page") == "MemoryKeeper" for r in receipts), "Keeper did not actually load")
        if any(hits[path] for path in ["/unvisited", "/unvisited-other", "/auto-heap", "/audio"]):
            raise AssertionError("Saved dormant pages were loaded before selection")
        _, dormant = required_snapshot([1, 10])
        if not dormant[1].get("otlozhena") or not dormant[10].get("otlozhena"):
            raise AssertionError("Dormant fixture rows were not lazy")
        report["checks"].append({"saved_tabs_lazy_without_page_requests": True})
        # Omit memory_saver in the fresh settings file: prove the application
        # default is ON without a Save/toggle that could initialize the child.
        initial_policy_bytes = (data / "nastrojki.json").read_bytes()
        initial_policy = json.loads(initial_policy_bytes)
        if initial_policy.get("memory_saver", True) is not True or initial_policy.get("memory_saver_minutes", 30) != 30:
            raise AssertionError("Startup did not retain the default automatic policy")
        click("Настройки NexusBro", ["Button"])
        click("Вкладки и поиск", ["Button"])
        checkbox = wait(lambda: find("Экономить память неактивных вкладок", ["CheckBox"]), "No actual automatic memory setting")
        if checkbox.iface_toggle.CurrentToggleState != 1:
            raise AssertionError("Automatic memory saving is not enabled by default")
        click("Отмена", ["Button"])
        if (data / "nastrojki.json").read_bytes() != initial_policy_bytes:
            raise AssertionError("Read-only automatic policy inspection modified settings")
        report["checks"].append({"default_automatic_on_normal_30_minutes_without_settings_mutation": True})
        cold = memory("cold_keeper")
        tab("MemoryPinned")
        wait(lambda: snapshot([3], lambda _, r: not r[3].get("otlozhena") and not r[3].get("zagruzka")), "Pinned fixture did not load")
        tab("MemoryLoading")
        wait(lambda: snapshot([5], lambda _, r: r[5].get("zagruzka")), "Slow fixture did not remain loading")
        tab("MemoryTools")
        wait(lambda: snapshot([4], lambda _, r: not r[4].get("otlozhena") and not r[4].get("zagruzka")), "Tools fixture did not load")
        click("MemoryTools", ["TabItem"], "right")
        click("Инструменты разработчика", ["MenuItem", "Button"])
        wait(lambda: find("Закрыть инструменты разработчика", ["Button"]), "Actual DevTools did not open")
        tab("MemoryDownload")
        wait(lambda: any(r.get("page") == "MemoryDownload" for r in receipts), "Download fixture did not load")
        physical_page_click()
        wait(lambda: hits.get("/download", 0), "Actual download did not start")
        download_started = time.monotonic()
        tab("MemoryBeforeUnload")
        wait(lambda: any(r.get("page") == "MemoryBeforeUnload" for r in receipts), "Beforeunload fixture did not load")
        physical_page_click()
        wait(lambda: any(r.get("guard") and r.get("trusted") for r in receipts), "Actual trusted gesture did not arm beforeunload")
        tab("MemoryDraft")
        wait(lambda: any(r.get("page") == "MemoryDraft" for r in receipts), "Draft fixture did not load")
        physical_page_click(150)
        gui.write("actual unsaved draft", interval=.03)
        click("MemoryKeeper", ["TabItem"], "right")
        click("Новая приватная вкладка", ["MenuItem", "Button"])
        navigate(base + "/private")
        wait(lambda: any(r.get("page") == "MemoryPrivate" for r in receipts), "Actual private fixture did not load")
        tab("MemoryKeeper")

        def actual_media_action(page, y, reason):
            previous = sum(r.get("page") == page and r.get("reason") == reason for r in receipts)
            physical_page_click(y)
            wait(lambda: sum(r.get("page") == page and r.get("reason") == reason for r in receipts) > previous,
                 f"Actual trusted media {reason} receipt missing for {page}")
            value = [r for r in receipts if r.get("page") == page and r.get("reason") == reason][-1]
            if value.get("trusted") is not True or value.get("error"):
                raise AssertionError(f"Real media action did not execute: {value}")
            return value

        # Genuine media remains protected with automatic saving ON throughout;
        # nobody enables per-tab exceptions or sends an unload command.
        media_before = {}
        for page, tab_id in [("MemoryAudio", 11), ("MemoryAudioPaused", 12),
                             ("MemoryWebAudio", 13), ("MemoryRTC", 14)]:
            tab(page)
            wait(lambda: snapshot([tab_id], lambda _, r: not r[tab_id].get("zagruzka") and not r[tab_id].get("otlozhena")),
                 f"Real media fixture did not finish loading: {page}")
            value = actual_media_action(page, 103, "start")
            if page.startswith("MemoryAudio"):
                if value.get("paused") is not False or value.get("ready_state", 0) < 2 or not value.get("duration"):
                    raise AssertionError("HTML audio did not actually start")
                if page == "MemoryAudioPaused":
                    value = actual_media_action(page, 193, "pause")
                    if value.get("paused") is not True:
                        raise AssertionError("HTML audio did not actually pause")
            elif page == "MemoryWebAudio" and value.get("context_state") != "running":
                raise AssertionError("Actual WebAudio oscillator context is not running")
            elif page == "MemoryRTC" and (value.get("connection_state") != "new" or value.get("signaling_state") != "stable"):
                raise AssertionError("Actual local RTCPeerConnection was not created")
            media_before[page] = value

        wait(lambda: time.monotonic() - download_started >= 10 and snapshot([6], lambda _, r: not r[6].get("otlozhena")),
             "Active download was not retained beyond the automatic idle interval", 20)
        assert_live(6)
        report["checks"].append({"active_download_retained_without_manual_exception": True})
        gui.hotkey("ctrl", "j")
        click("Пауза", ["Button"])
        wait(lambda: find("Продолжить", ["Button"]), "Actual download did not pause")
        click("Закрыть библиотеку", ["Button"])
        report["download_paused_at_seconds"] = time.monotonic() - started
        tab("MemoryKeeper")
        for title in ["MemoryKeeper", "MemoryPinned", "MemoryLoading", "MemoryBeforeUnload",
                      "MemoryDraft", "MemoryPrivate", "MemoryAudio"]:
            assert_no_memory_menu(title)
        report["checks"].append({"manual_memory_commands_absent_from_actual_tab_menus": True})
        protected_ids = [2, 3, 4, 5, 6, 7, 8, 11, 12, 13, 14]
        _, protected = required_snapshot(protected_ids)
        for tab_id in protected_ids:
            assert_live(tab_id, protected)
        if not protected[5].get("zagruzka"):
            raise AssertionError("Loading guard was not actually in progress")
        # Every other loaded ordinary page has an intrinsic guard; unloaded
        # saved rows stay dormant. No unrelated safe page can create RAM gain.
        baseline_closed = re.findall(r'NEXUSBRO_MEMORY tab=(\d+)\b[^\n]*closed=row-retained', native_logs())
        if baseline_closed:
            raise AssertionError("A guarded setup page was automatically unloaded before the baseline")
        tab("MemoryAutoHeap")
        wait(lambda: any(r.get("page") == "MemoryAutoHeap" and r.get("reason") == "allocated"
                         and r.get("bytes") == a.heap_mb * 1024**2 and r.get("first") == r.get("last") == 41 for r in receipts),
             "Safe automatic page did not actually allocate/touch its heap")
        _, heap = required_snapshot([9])
        if heap[9].get("nazad") or heap[9].get("vpered"):
            raise AssertionError("Automatic fixture unexpectedly has navigation history")
        # Native wheel scrolling, without keys/forms/sessionStorage/media.
        journal = (data / "zhurnal.log").read_text(encoding="utf-8")
        container = int(re.findall(r'memory: запуск ок, окно (\d+)', journal)[-1])
        box = wintypes.RECT()
        if not u.GetWindowRect(container, ctypes.byref(box)):
            raise AssertionError("Automatic fixture viewport missing")
        gui.moveTo(box.left + 400, box.top + 300)
        # PyAutoGUI's Windows backend passes clicks as raw dwData. Win32
        # requires WHEEL_DELTA=120 per notch; -6 raw units moved only 5px.
        # https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-mouse_event
        # A private DLL/function wrapper must not alter PyAutoGUI's shared
        # user32.mouse_event prototype (it passes c_long mouse coordinates).
        wheel_event = ctypes.WinDLL("user32", use_last_error=True).mouse_event
        wheel_event.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.DWORD,
                                wintypes.DWORD, ctypes.c_size_t]
        wheel_event.restype = None
        wheel_delta = -6 * 120
        wheel_event(0x0800, 0, 0, ctypes.c_uint32(wheel_delta).value, 0)
        report["automatic_wheel_input"] = {"transport": "win32_mouse_event", "delta": wheel_delta,
                                            "notches": 6, "required_scroll_pixels": 200}
        # Do not click this safe page before auto: the conservative early hook
        # deliberately protects trusted pointer/click interactions. The native
        # wheel's own scroll event supplies real scroll evidence without a click.
        wait(lambda: any(r.get("page") == "MemoryAutoHeap" and r.get("reason") == "native-scroll"
                         and r.get("trusted") and r.get("scroll_y", 0) >= 200 for r in receipts),
             "Actual automatic-fixture wheel scroll receipt missing")
        time.sleep(.6)  # Let native wheel smoothing finish before recording y.
        scroll_before = [r for r in receipts if r.get("page") == "MemoryAutoHeap" and r.get("reason") == "native-scroll"][-1]
        if not scroll_before.get("trusted") or scroll_before.get("scroll_y", 0) < 200:
            raise AssertionError("Safe automatic fixture was not physically scrolled")
        active_started = time.monotonic()
        while time.monotonic() - active_started < 10:
            processes()
            current, active_rows = required_snapshot([9])
            if current.get("aktivnaya") != 9:
                raise AssertionError("Automatic heap lost selection during active-page protection check")
            assert_live(9, active_rows)
            if active_rows[9].get("memory_pending"):
                raise AssertionError("Selected heap entered an automatic unload operation")
            time.sleep(.5)
        report["checks"].append({"active_heap_retained_beyond_idle_interval": True,
                                 "active_seconds": time.monotonic() - active_started})
        current, active_rows = required_snapshot([9])
        if current.get("aktivnaya") != 9:
            raise AssertionError("Automatic heap must be selected while taking its pre-close baseline")
        auto_before = memory("before_automatic_sleep_active_heap")
        _, active_rows = required_snapshot([9])
        assert_live(9, active_rows)
        if active_rows[9].get("memory_pending"):
            raise AssertionError("Selected heap unexpectedly entered an unload operation")
        if auto_before - cold < a.min_drop_mb * 1024**2:
            raise AssertionError("Touched heap did not measurably increase application private commit")
        tab("MemoryKeeper")
        protected_hits = {path: hits[path] for path in ["/audio", "/audio-paused", "/web-audio", "/rtc", "/private"]}
        if not normal_policy():
            raise AssertionError("Normal automatic saving policy changed during acceptance")
        # Minimize the real native shell while media is playing and a different
        # page is selected. Keep the actual OS minimized for at least 15 seconds.
        u.IsIconic.argtypes = [wintypes.HWND]
        u.IsIconic.restype = wintypes.BOOL
        minimize_started = time.monotonic()
        u.ShowWindow(win.handle, 6)
        wait(lambda: bool(u.IsIconic(win.handle)), "Actual application did not minimize", 5)
        closed_while_minimized = False
        first_closed_minimized_at = None
        while time.monotonic() - minimize_started < 15:
            processes()
            if not u.IsIconic(win.handle):
                raise AssertionError("Native window restored itself during the minimize interval")
            observed = snapshot([9])
            if observed and observed[1][9].get("otlozhena"):
                closed_while_minimized = True
                if first_closed_minimized_at is None:
                    first_closed_minimized_at = time.monotonic() - started
            time.sleep(.5)
        minimize_finished = time.monotonic()
        u.ShowWindow(win.handle, 9)
        wait(lambda: not u.IsIconic(win.handle), "Actual shell did not restore", 5)
        win.set_focus()
        report["actual_minimize_interval"] = {
            "started_seconds": minimize_started - started,
            "finished_seconds": minimize_finished - started,
            "duration_seconds": minimize_finished - minimize_started,
            "automatic_close_observed_while_minimized": closed_while_minimized,
            "first_closed_observed_seconds": first_closed_minimized_at,
            "selected_tab_id": 2,
        }
        if not closed_while_minimized:
            raise AssertionError("Automatic heap did not close while the application was genuinely minimized")
        # No unload command, keep setting, or interaction with the inactive heap.
        wait(lambda: snapshot([9], lambda _, r: r[9].get("otlozhena") and not r[9].get("memory_pending")),
             "Idle automatic fixture did not actually close into its retained sleeping row", 45)
        logs = native_logs()
        if not re.search(r'NEXUSBRO_MEMORY tab=9\b[^\n]*requested=automatic', logs):
            raise AssertionError("Automatic closure has no actual native automatic-request proof")
        if not re.search(r'NEXUSBRO_MEMORY tab=9\b[^\n]*verified=two-phase close=graceful automatic=true', logs):
            raise AssertionError("Automatic closure did not complete actual two-phase verification/graceful close")
        auto_dropped = False
        for attempt in range(6):
            auto_after = memory(f"after_automatic_sleep_{attempt}")
            if auto_before - auto_after >= a.min_drop_mb * 1024**2:
                auto_dropped = True
                break
            time.sleep(1)
        if not auto_dropped:
            raise AssertionError("Automatic native close did not reclaim the required private commit")
        report["checks"].append({"actual_automatic_two_phase_close": True,
                                 "drop_private_bytes": auto_before - auto_after,
                                 "isolated_idle_seconds": 8, "normal_policy_minutes": 30})
        media_kinds = {11: "html", 12: "html", 13: "webaudio", 14: "rtc"}
        wait(lambda: all(automatic_media_refusals(native_logs(), tab_id, kind)
                         for tab_id, kind in media_kinds.items()),
             "Missing actual automatic media/API-specific refusal; trusted-interaction-only protection is insufficient", 45)
        report["automatic_media_refusal_proofs"] = {
            str(tab_id): {"kind": kind, "actual_native_automatic_refusals": automatic_media_refusals(native_logs(), tab_id, kind)}
            for tab_id, kind in media_kinds.items()}
        closed = re.findall(r'NEXUSBRO_MEMORY tab=(\d+)\b[^\n]*closed=row-retained', native_logs())
        if closed != ["9"]:
            raise AssertionError("RAM decrease was contaminated by another automatic close: " + str(closed))
        if re.search(r'NEXUSBRO_MEMORY[^\n]*requested=manual', native_logs()):
            raise AssertionError("A manual unload was issued in automatic-only acceptance")
        report["checks"].append({"only_heap_automatically_closed_legacy_exception_ignored": True})
        wait(lambda: len(automatic_refusals(7)) >= 2,
             "Automatic beforeunload protection was not retried safely", 30)
        report["coverage"]["beforeunload_refusal"] = "actual_automatic_refusal_and_retry"
        report["automatic_beforeunload_refusals"] = automatic_refusals(7)
        _, retained = required_snapshot(protected_ids + [1, 10])
        for tab_id in protected_ids:
            if retained[tab_id].get("otlozhena"):
                raise AssertionError(f"Protected pinned/media/resource tab was automatically unloaded: {tab_id}")
        if hits["/unvisited"] or hits["/unvisited-other"] or not retained[1].get("otlozhena") or not retained[10].get("otlozhena"):
            raise AssertionError("Dormant saved pages were loaded by automatic memory saving")
        if not retained[5].get("zagruzka"):
            raise AssertionError("The protected loading fixture unexpectedly completed")
        if time.monotonic() - started - report["download_paused_at_seconds"] < 8:
            raise AssertionError("Paused download was not observed beyond the automatic idle interval")
        report["checks"].append({"pinned_loading_devtools_draft_private_active_and_paused_download_retained": True})
        for path, before_hits in protected_hits.items():
            if hits[path] != before_hits:
                raise AssertionError("Protected page navigated/reloaded during automatic idle window: " + path)
        media_after = {}
        _, media_rows = required_snapshot([11, 12, 13, 14])
        for page, tab_id in [("MemoryAudio", 11), ("MemoryAudioPaused", 12),
                             ("MemoryWebAudio", 13), ("MemoryRTC", 14)]:
            if media_rows[tab_id].get("otlozhena"):
                raise AssertionError("Protected media row was sleeping before physical inspection")
            tab(page)
            after_state = actual_media_action(page, 148, "snapshot")
            before_state = media_before[page]
            if (before_state["received_seconds"] > report["actual_minimize_interval"]["started_seconds"]
                    or after_state["received_seconds"] < report["actual_minimize_interval"]["finished_seconds"]):
                raise AssertionError("Actual resource clock receipts do not span the native minimized interval")
            if page == "MemoryAudio":
                first_after = after_state
                time.sleep(.8)
                after_state = actual_media_action(page, 148, "snapshot")
                immediate_delta = (after_state["current_time"] - first_after["current_time"]) % after_state["duration"]
                if not .25 <= immediate_delta <= 3.5:
                    raise AssertionError("Two actual HTML playback snapshots did not show a progressing media clock")
                report["html_audio_followup_clock"] = {"first": first_after, "second": after_state}
                if after_state.get("paused") is not False:
                    raise AssertionError("Real HTML audio stopped during automatic idle period")
                duration = after_state["duration"]
                actual = after_state["current_time"] - before_state["current_time"]
                wall = after_state["received_seconds"] - before_state["received_seconds"]
                if not 1 <= wall < duration - 3 or actual < 0:
                    raise AssertionError("HTML clock fixture exceeded its non-wrapping observation window")
                if abs(actual - wall) > 3:
                    raise AssertionError("Actual HTML playback clock did not continue across automatic idle period")
            elif page == "MemoryAudioPaused":
                if after_state.get("paused") is not True or abs(after_state["current_time"] - before_state["current_time"]) > .15:
                    raise AssertionError("Paused HTML audio was changed or resumed by automatic saver")
            elif page == "MemoryWebAudio":
                delta = after_state["current_time"] - before_state["current_time"]
                wall = after_state["received_seconds"] - before_state["received_seconds"]
                if after_state.get("context_state") != "running" or delta < 1 or abs(delta - wall) > 3:
                    raise AssertionError("Actual WebAudio clock did not continue across automatic idle period")
            elif after_state.get("connection_state") != "new" or after_state.get("signaling_state") != "stable":
                raise AssertionError("Retained local RTCPeerConnection was lost")
            media_after[page] = after_state
        report["media_clock_proofs"] = {"before": media_before, "after": media_after}
        tab("MemoryDraft")
        previous_draft = sum(r.get("reason") == "draft-snapshot" for r in receipts)
        physical_page_click(193)
        wait(lambda: sum(r.get("reason") == "draft-snapshot" for r in receipts) > previous_draft,
             "Actual draft snapshot was not received")
        draft = [r for r in receipts if r.get("reason") == "draft-snapshot"][-1]
        if not draft.get("trusted") or draft.get("value") != "actual unsaved draft":
            raise AssertionError("Automatic memory saving lost the real unsaved draft")
        report["checks"].append({"actual_unsaved_draft_preserved_without_exception": True})
        tab("MemoryPrivate")
        time.sleep(.5)
        if hits["/private"] != protected_hits["/private"]:
            raise AssertionError("Private page reloaded after automatic saver inspection")
        tab("MemoryAutoHeap")
        wait(lambda: snapshot([9], lambda _, r: not r[9].get("otlozhena") and not r[9].get("zagruzka")),
             "Automatic sleeping row did not wake")
        previous = sum(r.get("page") == "MemoryAutoHeap" and r.get("reason") == "snapshot" for r in receipts)
        physical_page_click()
        wait(lambda: sum(r.get("page") == "MemoryAutoHeap" and r.get("reason") == "snapshot" for r in receipts) > previous,
             "No actual snapshot of automatic wake scroll/storage")
        wake_auto = [r for r in receipts if r.get("page") == "MemoryAutoHeap" and r.get("reason") == "snapshot"][-1]
        if (wake_auto.get("seen") != "preserved" or "memoryauto=preserved" not in wake_auto.get("cookie", "")
                or abs(wake_auto.get("scroll_y", 0) - scroll_before["scroll_y"]) > 64):
            raise AssertionError("Automatic wake lost actual cookie/localStorage/scroll state")
        report["checks"].append({"automatic_same_row_wake_cookie_storage_scroll_preserved": True})
        report["coverage"]["automatic_unload"] = "actual_native_two_phase_auto_close_private_commit_drop_and_wake"
        report["coverage"]["media"] = "actual_html_playing_paused_webaudio_clocks_and_local_rtc_retained_across_native_minimize"
        report["completed"] = True
        ImageGrab.grab().save(out / "completed.png")
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        try:
            ImageGrab.grab().save(out / "failure.png")
        except Exception:
            pass
        raise
    finally:
        release_loading.set()
        stop_download.set()
        if app is not None:
            try:
                processes()
            except Exception:
                pass
            for (_, created), p in reversed(list(owned.items())):
                try:
                    if p.create_time() == created:
                        p.kill()
                except psutil.NoSuchProcess:
                    pass
            if app.poll() is None:
                app.kill()
            app.wait(timeout=10)
        if log:
            log.close()
        if server:
            server.shutdown()
            server.server_close()
        report["total_seconds"] = time.monotonic() - started
        checkpoint()
        print(json.dumps({k: report.get(k) for k in ["completed", "checks", "error"]}, ensure_ascii=False))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--exe", required=True)
    p.add_argument("--cef", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--heap-mb", type=int, default=192)
    p.add_argument("--min-drop-mb", type=int, default=64)
    p.add_argument("--timeout", type=int, default=600)
    a = p.parse_args()
    if not (128 <= a.heap_mb <= 512 and 32 <= a.min_drop_mb <= a.heap_mb // 2 and 120 <= a.timeout <= 600):
        p.error("Heap/drop/time outside bounded fixture limits")
    run(a)


if __name__ == "__main__":
    main()
