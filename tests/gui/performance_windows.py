#!/usr/bin/env python3
"""Measure a real installed NexusBro on an isolated Windows desktop account.

This is a measurement harness, not a synthetic backend or a performance PASS.
Only the fixture's saved profile is seeded; every engine/tab is woken by mouse.
"""
import argparse
import collections
import http.server
import json
import math
import os
import pathlib
import platform
import re
import subprocess
import sys
import threading
import time
import urllib.parse


def role(name, command, root=False):
    if root:
        return "shell"
    family = "webview2" if "msedgewebview2" in name.lower() else "cef"
    kind = next((v.partition("=")[2] for v in command if v.startswith("--type=")), None)
    if kind:
        return family + "/" + kind
    if "--nb-dvizhok" in command:
        return "cef/browser"
    return family + "/browser" if family == "webview2" else "other"


def assert_background_throttled(reports, phases, pages, active_page):
    """Require actual stable reports; missing hidden reports never imply zero rAF."""
    evidence = []
    for name in ["warm_active", "warm_minimized", "warm_restored"]:
        phase = next(p for p in phases if p["phase"] == name)
        for page in sorted(pages):
            stable = [r for r in reports if r["page"] == page and r["received_phase"] == name
                      and r["reason"] == "interval" and r["interval_ms"] >= 1000
                      and r["received_seconds"] - r["interval_ms"] / 1000 >= phase["start_seconds"] + 2]
            if len(stable) < 2:
                raise AssertionError(f"Insufficient actual stable visibility reports: {name}/{page}: {len(stable)}")
            visible = name != "warm_minimized" and page == active_page
            if any(r["visibility"] != ("visible" if visible else "hidden") or r["hidden"] != (not visible) for r in stable):
                raise AssertionError(f"Unexpected actual document visibility: {name}/{page}")
            if visible:
                if not any(r["frames_delta"] > 0 and r["raf_hz"] >= 5 for r in stable):
                    raise AssertionError(f"Visible page animation did not resume: {name}/{page}")
            elif any(r["frames_delta"] != 0 or r["raf_hz"] != 0 for r in stable):
                raise AssertionError(f"Background page continued animation: {name}/{page}")
            evidence.append({"phase": name, "page": page, "actual_reports": len(stable),
                             "expected_visible": visible, "max_raf_hz": max(r["raf_hz"] for r in stable)})
    return evidence


def run(args):
    if sys.platform != "win32":
        raise RuntimeError("The measurement requires real Windows; no substituted result")
    import ctypes
    from ctypes import wintypes
    import psutil
    from pywinauto import Desktop
    from comtypes import COMError
    from PIL import ImageGrab
    import tkinter

    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    data = out / "data"
    data.mkdir()
    report = {"schema": 1, "measurement_completed": False, "optimized": False,
              "platform": platform.platform(), "processor": platform.processor(),
              "image": {key: os.environ.get(key) for key in ("ImageOS", "ImageVersion", "RUNNER_ARCH")},
              "total_physical_bytes": psutil.virtual_memory().total, "logical_cpus": psutil.cpu_count(),
              "requested": {"supers": args.supers, "tabs_per_super": args.tabs,
                            "saved_tabs_per_super": args.saved_tabs,
                            "saved_tabs_total": args.supers * args.saved_tabs,
                            "visited_tabs_total": args.supers * args.tabs},
              "phases": [], "samples": [], "fixture_hits": {},
              "visibility_probe": args.visibility_probe}
    hits = collections.Counter()
    hit_lock = threading.Lock()
    visibility_reports = []
    input_proofs = []
    visibility_dropped = 0
    current_phase = "setup"
    started = time.monotonic()
    app = cover = log = None
    owned = {}
    server = None
    u = ctypes.windll.user32
    u.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    u.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    u.IsIconic.argtypes = [wintypes.HWND]
    u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint]
    u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]

    class Fixture(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            path = urllib.parse.urlsplit(self.path).path
            parts = path.strip("/").split("/")
            if len(parts) != 3 or parts[0] != "page" or not all(p.isdecimal() for p in parts[1:]):
                self.send_error(404)
                return
            with hit_lock:
                hits[path] += 1
            title = f"Perf {parts[1]}-{parts[2]}"
            body = (f'<!doctype html><meta charset="utf-8"><title>{title}</title>'
                    '<style>html,body{margin:0;background:#185541;color:white;font:24px sans-serif}</style>'
                    f'<h1>{title}</h1><p>Static loopback performance fixture.</p>').encode()
            if args.visibility_probe:
                # Diagnostic-only: requests rAF without changing the DOM/painting.
                # This intentionally differs from the static CPU baseline.
                body += b'''<script>
(()=>{let frames=0,previousFrames=0,previousTime=performance.now(),seq=0;
const page=location.pathname;
function frame(){frames++;if(frames<60000)requestAnimationFrame(frame)}
requestAnimationFrame(frame);
function report(reason){const now=performance.now(),duration=now-previousTime;
const delta=frames-previousFrames;
const body={page,seq:++seq,reason,visibility:document.visibilityState,
hidden:document.hidden,focus:document.hasFocus(),frames,frames_delta:delta,
interval_ms:duration,raf_hz:duration>0?delta*1000/duration:0,client_ms:now};
previousTime=now;previousFrames=frames;
fetch('/visibility',{method:'POST',headers:{'Content-Type':'application/json'},
body:JSON.stringify(body),keepalive:true}).catch(()=>{});}
report('initial');document.addEventListener('visibilitychange',()=>report('visibilitychange'));
setInterval(()=>report('interval'),2000);
})();</script>'''
                if args.expect_background_throttled:
                    body += b'''<input id="physical-input" aria-label="Physical input proof"
style="position:absolute;left:20px;top:130px;width:320px;height:40px;box-sizing:border-box">
<script>document.getElementById('physical-input').addEventListener('input',event=>{
if(event.isTrusted&&event.target.value==='nexusbro-performance-input'){
fetch('/input-proof',{method:'POST',headers:{'Content-Type':'application/json'},
body:JSON.stringify({page:location.pathname,value:event.target.value,
trusted:event.isTrusted,focus:document.hasFocus()}),keepalive:true}).catch(()=>{});}});</script>'''
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            nonlocal visibility_dropped
            if not args.visibility_probe or self.path not in ["/visibility", "/input-proof"]:
                self.send_error(404)
                return
            try:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 2048:
                    raise ValueError("Invalid diagnostic payload size")
                payload = json.loads(self.rfile.read(size))
                if self.path == "/input-proof":
                    if (not args.expect_background_throttled or not isinstance(payload, dict)
                            or set(payload) != {"page", "value", "trusted", "focus"}
                            or payload["page"] != f"/page/{args.supers - 1}/{args.tabs - 1}"
                            or payload["value"] != "nexusbro-performance-input"
                            or payload["trusted"] is not True or payload["focus"] is not True):
                        raise ValueError("Invalid real physical input proof")
                    with hit_lock:
                        if len(input_proofs) < 4:
                            input_proofs.append({**payload, "received_seconds": time.monotonic() - started})
                    self.send_response(204)
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                keys = {"page", "seq", "reason", "visibility", "hidden", "focus", "frames",
                        "frames_delta", "interval_ms", "raf_hz", "client_ms"}
                if not isinstance(payload, dict) or set(payload) != keys:
                    raise ValueError("Invalid diagnostic payload keys")
                parts = payload["page"].strip("/").split("/")
                if (len(parts) != 3 or parts[0] != "page" or not all(p.isdecimal() for p in parts[1:])
                        or not 0 <= int(parts[1]) < args.supers or not 0 <= int(parts[2]) < args.tabs
                        or payload["reason"] not in ["initial", "interval", "visibilitychange"]
                        or payload["visibility"] not in ["visible", "hidden", "prerender"]
                        or not isinstance(payload["hidden"], bool) or not isinstance(payload["focus"], bool)):
                    raise ValueError("Invalid diagnostic page/state")
                if any(type(payload[k]) not in [int, float] or not math.isfinite(payload[k])
                       or payload[k] < 0 for k in ["seq", "frames", "frames_delta", "interval_ms", "raf_hz", "client_ms"]):
                    raise ValueError("Invalid diagnostic counters")
                with hit_lock:
                    if len(visibility_reports) < 20000:
                        visibility_reports.append({**payload, "received_seconds": time.monotonic() - started,
                                                   "received_phase": current_phase})
                    else:
                        visibility_dropped += 1
            except (ValueError, TypeError, KeyError, AttributeError):
                self.send_error(400)
                return
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *_):
            pass

    def check_limits():
        if time.monotonic() - started > args.timeout:
            raise TimeoutError("Overall benchmark timeout exceeded")
        if app is not None and app.poll() is not None:
            raise RuntimeError(f"NexusBro exited ({app.returncode})")
        processes = [psutil.Process(app.pid)] if app else []
        if processes:
            processes += processes[0].children(recursive=True)
        total = 0
        live = []
        for process in processes:
            try:
                identity = (process.pid, process.create_time())
                owned[identity] = process
                total += process.memory_info().rss
                live.append(process)
            except psutil.NoSuchProcess:
                continue
        if len(live) > args.max_processes or total > memory_cap:
            raise RuntimeError(f"Safety limit: processes={len(live)}, working_set_bytes={total}, cap={memory_cap}")
        return live

    def snapshot():
        found = {}
        for process in check_limits():
            try:
                with process.oneshot():
                    mem = process.memory_info()
                    cpu = process.cpu_times()
                    ident = (process.pid, process.create_time())
                    found[ident] = {"pid": process.pid, "created": ident[1],
                                    "role": role(process.name(), process.cmdline(), process.pid == app.pid),
                                    "cpu_seconds": cpu.user + cpu.system,
                                    "working_set_bytes": mem.rss,
                                    "private_bytes": mem.private}
            except psutil.NoSuchProcess:
                continue
        return time.monotonic(), found

    def tab_counts():
        """Read persisted lifecycle state; do not infer live pages from saved URLs."""
        saved = live = deferred = 0
        for s in range(args.supers):
            try:
                session = json.loads((data / "profili" / f"perf-{s}" / "obychnye.json").read_text(encoding="utf-8"))
            except (ValueError, OSError):
                return {"saved": None, "live": None, "deferred": None, "available": False}
            tabs = session["vkladki"]
            saved += len(tabs)
            deferred += sum(bool(v.get("otlozhena")) for v in tabs)
            live += sum(not v.get("otlozhena") for v in tabs)
        return {"saved": saved, "live": live, "deferred": deferred, "available": True}

    def measure(name):
        nonlocal current_phase
        with hit_lock:
            current_phase = name
        # Sampling itself stays outside the application's process tree.
        previous_time, previous = snapshot()
        phase_start = previous_time
        rows = []
        while time.monotonic() - phase_start < args.seconds:
            until = time.monotonic() + 1
            while time.monotonic() < until:
                if cover is not None:
                    cover.update()
                time.sleep(.025)
            now, current = snapshot()
            elapsed = now - previous_time
            groups = collections.defaultdict(lambda: {"cpu_seconds_delta": 0, "processes": 0,
                                                       "working_set_bytes": 0, "private_bytes": 0})
            per_process = []
            for identity, item in current.items():
                # New PIDs were not observed for the complete interval: expose
                # them but do not invent a full-interval CPU delta.
                old = previous.get(identity)
                delta = max(0, item["cpu_seconds"] - old["cpu_seconds"]) if old else None
                group = groups[item["role"]]
                group["processes"] += 1
                group["working_set_bytes"] += item["working_set_bytes"]
                group["private_bytes"] += item["private_bytes"]
                if delta is not None:
                    group["cpu_seconds_delta"] += delta
                per_process.append({**item, "cpu_seconds_delta": delta})
            for group in groups.values():
                group["cpu_core_percent"] = 100 * group["cpu_seconds_delta"] / elapsed
                group["cpu_machine_percent"] = group["cpu_core_percent"] / report["logical_cpus"]
            row = {"phase": name, "elapsed_seconds": elapsed, "roles": dict(groups),
                   "processes": per_process, "tab_counts": tab_counts(),
                   "system_cpu_machine_percent": psutil.cpu_percent(),
                   "unobserved_exited_processes": len(previous.keys() - current.keys())}
            report["samples"].append(row)
            rows.append(row)
            previous_time, previous = now, current
        duration = sum(r["elapsed_seconds"] for r in rows)
        cpu = sum(g["cpu_seconds_delta"] for r in rows for g in r["roles"].values())
        report["phases"].append({"phase": name, "duration_seconds": duration,
                                 "start_seconds": phase_start - started,
                                 "end_seconds": time.monotonic() - started,
                                 "tab_counts": tab_counts(),
                                 "cpu_core_percent": 100 * cpu / duration,
                                 "cpu_machine_percent": 100 * cpu / duration / report["logical_cpus"],
                                 "peak_processes": max(len(r["processes"]) for r in rows),
                                 "peak_working_set_bytes": max(sum(p["working_set_bytes"] for p in r["processes"]) for r in rows),
                                 "peak_private_bytes": max(sum(p["private_bytes"] for p in r["processes"]) for r in rows)})
        ImageGrab.grab().save(out / (name + ".png"))
        (out / "results.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        with hit_lock:
            current_phase = "between_phases"

    def wait_for(predicate, description, seconds=30):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            check_limits()
            value = predicate()
            if value:
                return value
            time.sleep(.15)
        raise TimeoutError(description)

    try:
        available = psutil.virtual_memory().available
        memory_cap = min(args.max_memory_mb * 1024**2, int(available * .6))
        report["limits"] = {"processes": args.max_processes, "working_set_bytes": memory_cap,
                            "available_before_bytes": available, "overall_timeout_seconds": args.timeout}
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{server.server_port}"
        definitions = []
        for s in range(args.supers):
            sid = f"perf-{s}"
            definitions.append({"id": sid, "nazvanie": f"PerfSuper{s}", "poryadok": s,
                                "proxy": "", "adres": base + f"/page/{s}/0", "pustaya": False,
                                "rezat_reklamu": True})
            folder = data / "profili" / sid
            folder.mkdir(parents=True)
            tabs = [{"id": t + 1, "adres": base + f"/page/{s}/{t}", "otlozhena": True,
                     "zagolovok": f"Perf {s}-{t}"} for t in range(args.saved_tabs)]
            (folder / "obychnye.json").write_text(json.dumps({"vkladki": tabs, "aktivnaya": 1,
                                                              "sleduyushchaya": args.saved_tabs + 1}), encoding="utf-8")
        (data / "vkladki.json").write_text(json.dumps(definitions), encoding="utf-8")
        (data / "nastrojki.json").write_text(json.dumps({"avtoobnovleniya": False,
                                                         "apparatnoe_uskorenie": False}), encoding="utf-8")
        env = {**os.environ, "NEXUSBRO_TEST_DATA": str(data), "NEXUSBRO_PROBA": "0",
               "WEBVIEW2_USER_DATA_FOLDER": str(out / "webview2")}
        if args.cef:
            cef = pathlib.Path(args.cef).resolve()
            if not (cef / "libcef.dll").is_file():
                raise RuntimeError("--cef must contain the actual libcef.dll")
            env.update({"CEF_PATH": str(cef), "PATH": str(cef) + os.pathsep + env["PATH"]})
        log = (out / "app.log").open("w", encoding="utf-8")
        app = subprocess.Popen([str(pathlib.Path(args.exe).resolve())], env=env, stdout=log, stderr=log)
        windows = Desktop(backend="uia")
        window = wait_for(lambda: next(iter(windows.windows(process=app.pid, title="NexusBro")), None),
                          "NexusBro shell did not appear", 45)
        hwnd = window.handle
        u.ShowWindow(hwnd, 9)
        width, height = min(1280, u.GetSystemMetrics(0)-40), min(820, u.GetSystemMetrics(1)-60)
        if width < 900 or height < 560:
            raise RuntimeError("Windows desktop too small for physical tab workload")
        if not u.SetWindowPos(hwnd, None, 20, 20, width, height, 0x0040):
            raise RuntimeError("Native SetWindowPos failed")
        window.set_focus()

        def click(name, kind):
            def find():
                try:
                    for control in window.descendants():
                        if control.element_info.name == name and control.element_info.control_type == kind:
                            if control.is_visible() and control.is_enabled():
                                try:
                                    control.iface_scroll_item.ScrollIntoView()
                                except Exception:
                                    pass
                                return control
                except (COMError, KeyError):
                    return None
                return None
            control = wait_for(find, f"Real UIA control missing: {kind}/{name}")
            box = control.rectangle()
            # The UIA default clickable point may be a nested close button.
            # Click the leading padding of the requested tab/Super instead.
            control.click_input(coords=(min(10, max(2, box.width() // 4)), box.height() // 2))

        measure("cold_saved_lazy")
        with hit_lock:
            report["cold_fixture_hits"] = dict(hits)
        if hits:
            raise AssertionError("Saved lazy startup unexpectedly requested a page")
        for s in range(args.supers):
            click(f"PerfSuper{s}", "Button")
            for t in range(args.tabs):
                click(f"Perf {s}-{t}", "TabItem")
                path = f"/page/{s}/{t}"
                wait_for(lambda: hits.get(path, 0) > 0, f"Tab did not really load: {path}")
            # Wait for the actual persisted loaded title, not only an HTTP request.
            def loaded():
                try:
                    session = json.loads((data / "profili" / f"perf-{s}" / "obychnye.json").read_text(encoding="utf-8"))
                    tabs = session["vkladki"]
                    visited = [v for v in tabs if 1 <= v["id"] <= args.tabs]
                    deferred = [v for v in tabs if args.tabs < v["id"] <= args.saved_tabs]
                    return (len(tabs) == args.saved_tabs
                            and len(visited) == args.tabs
                            and len(deferred) == args.saved_tabs - args.tabs
                            and all(not v.get("otlozhena") and not v.get("zagruzka") for v in visited)
                            and all(v.get("otlozhena") for v in deferred))
                except (ValueError, OSError):
                    return False
            wait_for(loaded, f"Super{s} did not finish loading every requested tab")
        _, warm_processes = snapshot()
        report["warm_cef_browser_processes"] = sum(p["role"] == "cef/browser" for p in warm_processes.values())
        if report["warm_cef_browser_processes"] != args.supers:
            raise AssertionError("Live CEF engine count differs from the requested supers")
        report["warm_tab_counts"] = tab_counts()
        with hit_lock:
            expected = {f"/page/{s}/{t}" for s in range(args.supers) for t in range(args.tabs)}
            if set(hits) != expected:
                raise AssertionError("Fixture requested pages outside the explicitly visited workload")
        time.sleep(args.settle)
        measure("warm_active")
        cover = tkinter.Tk()
        cover.title("Performance fixture cover")
        cover.geometry("1400x1000+0+0")
        cover.attributes("-topmost", True)
        cover.update()
        measure("warm_covered")
        cover.destroy()
        cover = None
        u.ShowWindow(hwnd, 6)
        if not u.IsIconic(hwnd):
            raise AssertionError("NexusBro did not minimize")
        measure("warm_minimized")
        u.ShowWindow(hwnd, 9)
        window.set_focus()
        measure("warm_restored")
        if args.expect_background_throttled:
            import pyautogui
            journal = (data / "zhurnal.log").read_text(encoding="utf-8")
            handles = re.findall(rf'perf-{args.supers - 1}: запуск ок, окно (\d+)', journal)
            if not handles:
                raise AssertionError("Owned active CEF container handle absent from the application journal")
            container = int(handles[-1])
            box = wintypes.RECT()
            if not u.GetWindowRect(container, ctypes.byref(box)) or box.bottom - box.top < 220:
                raise AssertionError("Restored native CEF viewport has no usable rectangle")
            rgb = []
            def painted():
                nonlocal rgb
                screen = ImageGrab.grab()
                points = [(box.left + 20, (box.top + box.bottom) // 2),
                          ((box.left + box.right) // 2, (box.top + box.bottom) // 2),
                          (box.right - 20, (box.top + box.bottom) // 2)]
                rgb = [screen.getpixel(point)[:3] for point in points]
                if all(c == (24, 85, 65) for c in rgb):
                    screen.save(out / "restored-cef-painted.png")
                    return True
                return False
            wait_for(painted, "Restored CEF viewport is not actually painted green", 15)
            report["restored_cef_rgb"] = rgb
            pyautogui.click(box.left + 100, box.top + 150)
            pyautogui.write("nexusbro-performance-input", interval=.04)
            wait_for(lambda: bool(input_proofs), "Restored CEF did not accept actual trusted keyboard input", 15)
            with hit_lock:
                report["background_throttle_acceptance"] = assert_background_throttled(
                    visibility_reports, report["phases"], expected,
                    f"/page/{args.supers - 1}/{args.tabs - 1}")
        if args.visibility_probe:
            with hit_lock:
                observed = {row["page"] for row in visibility_reports}
                if observed != expected or visibility_dropped:
                    raise AssertionError("Visibility probe missing a visited page or exceeded its report bound")
        report["measurement_completed"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        try:
            ImageGrab.grab().save(out / "failure.png")
        except Exception as screenshot_error:
            report["screenshot_error"] = type(screenshot_error).__name__
        raise
    finally:
        if cover is not None:
            cover.destroy()
        if app is not None:
            # Only descendants collected from our own root, with PID reuse guard.
            try:
                check_limits()
            except Exception:
                pass
            for (pid, created), process in reversed(list(owned.items())):
                try:
                    if process.create_time() == created:
                        process.kill()
                except psutil.NoSuchProcess:
                    pass
            if app.poll() is None:
                app.kill()
            app.wait(timeout=10)
        if log is not None:
            log.close()
        if server:
            server.shutdown()
            server.server_close()
        report["fixture_hits"] = dict(hits)
        if args.visibility_probe:
            report["visibility_reports"] = list(visibility_reports)
            report["visibility_reports_dropped"] = visibility_dropped
            report["restored_input_proofs"] = list(input_proofs)
        report["loaded_unique_pages"] = len(hits)
        report["total_seconds"] = time.monotonic() - started
        (out / "results.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(json.dumps({k: report.get(k) for k in ["measurement_completed", "loaded_unique_pages", "phases", "error"]}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", required=True, help="Installed/extracted NexusBro executable with bundled resources")
    parser.add_argument("--cef", help="CEF directory, if not adjacent to the extracted executable")
    parser.add_argument("--out", required=True, help="Fresh nonexistent result directory")
    parser.add_argument("--supers", type=int, default=4)
    parser.add_argument("--tabs", type=int, default=6)
    parser.add_argument("--saved-tabs", type=int, help="Saved tabs per super; defaults to --tabs; at most 5000 total")
    parser.add_argument("--visibility-probe", action="store_true", help="Diagnostic JS visibility/rAF reporting; changes the static CPU workload")
    parser.add_argument("--expect-background-throttled", action="store_true", help="Require actual hidden/zero-rAF background pages and restored painted/trusted-input proof")
    parser.add_argument("--seconds", type=int, default=15, help="Per measured phase")
    parser.add_argument("--settle", type=int, default=5)
    parser.add_argument("--timeout", type=int, default=480)
    parser.add_argument("--max-processes", type=int, default=128)
    parser.add_argument("--max-memory-mb", type=int, default=8192)
    args = parser.parse_args()
    if args.expect_background_throttled and (not args.visibility_probe or args.seconds < 8):
        parser.error("--expect-background-throttled requires --visibility-probe and --seconds >= 8")
    if args.saved_tabs is None:
        args.saved_tabs = args.tabs
    if not (1 <= args.supers <= 12 and 1 <= args.tabs <= 12 and 3 <= args.seconds <= 120
            and 0 <= args.settle <= 30 and 60 <= args.timeout <= 900
            and args.tabs <= args.saved_tabs and args.saved_tabs * args.supers <= 5000
            and 8 <= args.max_processes <= 128 and 512 <= args.max_memory_mb <= 8192):
        parser.error("Workload/time/resource limits outside safe bounds")
    run(args)


if __name__ == "__main__":
    main()
