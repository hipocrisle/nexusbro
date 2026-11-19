#!/usr/bin/env python3
"""Real Windows manual tab-unload acceptance; no injected IPC or CEF/CDP mocks."""
import argparse
import collections
import http.server
import json
import os
import pathlib
import platform
import re
import statistics
import subprocess
import sys
import threading
import time
import urllib.parse


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
    report = {"schema": 1, "completed": False, "platform": platform.platform(),
              "heap_mb": a.heap_mb, "min_private_drop_mb": a.min_drop_mb,
              "checks": [], "memory": {}, "coverage": {"beforeunload_refusal": "not_executed",
              "user_close_during_sleep": "not_executed", "automatic_unload": "not_implemented_off"}}
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

    class Fixture(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            path = urllib.parse.urlsplit(self.path).path
            with lock:
                hits[path] += 1
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
            names = {"/heap": "MemoryHeap", "/keeper": "MemoryKeeper", "/pinned": "MemoryPinned",
                     "/tools": "MemoryTools", "/loading": "MemoryLoading", "/downloads": "MemoryDownload",
                     "/private": "MemoryPrivate", "/unload": "MemoryBeforeUnload", "/draft": "MemoryDraft"}
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
                body += b'<input aria-label="Actual unsaved draft" style="position:absolute;left:20px;top:130px;width:300px;height:40px">'
            if path == "/heap":
                body += ("<script>const seen=localStorage.getItem('memory-fixture');"
                         "const cookie=document.cookie;localStorage.setItem('memory-fixture','preserved');"
                         "document.cookie='memoryfixture=preserved;path=/;max-age=86400;SameSite=Lax';"
                         f"window.fixtureHeap=new Uint8Array({a.heap_mb}*1024*1024);window.fixtureHeap.fill(37);"
                         "fetch('/receipt',{method:'POST',body:JSON.stringify({page:'MemoryHeap',seen,cookie,"
                         "bytes:fixtureHeap.byteLength,first:fixtureHeap[0],last:fixtureHeap[fixtureHeap.length-1],"
                         "url:location.href})});</script>").encode()
            elif path != "/loading":
                body += ("<script>fetch('/receipt',{method:'POST',body:JSON.stringify({page:document.title})});</script>").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body) + (4096 if path == "/loading" else 0)))
            self.end_headers()
            self.wfile.write(body)
            self.wfile.flush()
            if path == "/loading":
                release_loading.wait(90)
                try:
                    self.wfile.write(b" " * 4096)
                except OSError:
                    pass

        def do_POST(self):
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
                    if len(receipts) < 256:
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

    def row(tab_id):
        rows = [v for v in session()["vkladki"] if v["id"] == tab_id]
        if len(rows) > 1:
            raise AssertionError("Duplicate tab row appeared")
        return rows[0] if rows else None

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
                 "zakreplena": i == 3} for i, path, title in [
                    (1, "/heap", "MemoryHeap"), (2, "/keeper", "MemoryKeeper"),
                    (3, "/pinned", "MemoryPinned"), (4, "/tools", "MemoryTools"),
                    (5, "/loading", "MemoryLoading"), (6, "/downloads", "MemoryDownload"),
                    (7, "/unload", "MemoryBeforeUnload"), (8, "/draft", "MemoryDraft")]]
        folder = data / "profili" / "memory"
        folder.mkdir(parents=True)
        (folder / "obychnye.json").write_text(json.dumps({"vkladki": tabs, "aktivnaya": 2, "sleduyushchaya": 9}), encoding="utf-8")
        (data / "vkladki.json").write_text(json.dumps([{"id": "memory", "nazvanie": "MemorySuper",
            "proxy": "", "adres": base + "/keeper", "pustaya": False, "rezat_reklamu": True}]), encoding="utf-8")
        (data / "nastrojki.json").write_text(json.dumps({"avtoobnovleniya": False}), encoding="utf-8")
        cef = pathlib.Path(a.cef).resolve()
        env = {**os.environ, "CEF_PATH": str(cef), "PATH": str(cef) + os.pathsep + os.environ["PATH"],
               "NEXUSBRO_TEST_DATA": str(data), "WEBVIEW2_USER_DATA_FOLDER": str(out / "webview2"),
               "NEXUSBRO_PROBA": "0", "NEXUSBRO_TEST_DOWNLOAD_DIR": str(out / "downloads")}
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

        def open_sleep(title):
            click(title, ["TabItem"], "right")
            return wait(lambda: find("Выгрузить вкладку из памяти", ["MenuItem", "Button"]), "No actual sleep menu item")

        def warning_confirm(title, cancel=False):
            item = open_sleep(title)
            if not item.is_enabled():
                raise AssertionError("Sleep menu unexpectedly disabled for eligible inactive tab")
            item.click_input()
            wait(lambda: find("Выгрузить вкладку из памяти"), "No visible warning dialog")
            text = " ".join(c.element_info.name for c in controls() if c.element_info.name)
            if not all(word in text.lower() for word in ["перезагруз", "несохран", "история", "cookies"]):
                raise AssertionError("Warning omits reload/internal-state/history/storage consequences")
            ImageGrab.grab().save(out / ("warning-cancel.png" if cancel else "warning-confirm.png"))
            click("Отмена" if cancel else "Выгрузить", ["Button"])

        def native_logs():
            return "\n".join(p.read_text(encoding="utf-8", errors="replace") for p in folder.glob("*.log"))

        def protected(title, tab_id=None):
            prior_logs = native_logs()
            item = open_sleep(title)
            if not item.is_enabled():
                gui.press("escape")
                result = "native_menu_disabled"
            else:
                item.click_input()
                click("Выгрузить", ["Button"])
                if tab_id is not None:
                    wait(lambda: native_logs().count(f"NEXUSBRO_MEMORY tab={tab_id} refused=") > prior_logs.count(f"NEXUSBRO_MEMORY tab={tab_id} refused="),
                         "Actual engine did not log a protected-request refusal")
                wait(lambda: not find("Выгрузить", ["Button"]) and any(c.element_info.name for c in controls() if c.element_info.control_type == "Text"
                                 and any(w in c.element_info.name.lower() for w in ["нельзя", "в памяти", "откреп", "загруз", "инструмент", "сохран", "ввод"])),
                     "Protected request did not report an actual refusal")
                result = "backend_refusal_visible"
            time.sleep(.5)
            if tab_id is not None and (not row(tab_id) or row(tab_id).get("otlozhena")):
                raise AssertionError("Protected live tab was unloaded")
            report["checks"].append({"protected": title, "result": result})
            checkpoint()

        click("MemorySuper", ["Button"])
        tab("MemoryKeeper")
        wait(lambda: any(r.get("page") == "MemoryKeeper" for r in receipts), "Keeper did not actually load")
        cold = memory("cold_keeper")
        tab("MemoryHeap")
        wait(lambda: any(r.get("bytes") == a.heap_mb * 1024**2 and r.get("first") == r.get("last") == 37 for r in receipts),
             "Large heap was not allocated and touched by actual page")
        navigate(base + "/heap?visit=next")
        wait(lambda: row(1) and row(1).get("nazad") and sum(r.get("page") == "MemoryHeap" for r in receipts) >= 2,
             "Real pre-unload back history not established")
        protected("MemoryHeap", 1)  # Active page is never eligible.
        tab("MemoryKeeper")
        time.sleep(2)
        before = memory("before_sleep")
        if before - cold < a.min_drop_mb * 1024**2:
            raise AssertionError("Fixture heap did not measurably increase total private commit")
        warning_confirm("MemoryHeap", cancel=True)
        if row(1).get("otlozhena"):
            raise AssertionError("Cancel unloaded the live tab")
        report["checks"].append({"warning_cancel_preserves_live": True})
        warning_confirm("MemoryHeap")
        wait(lambda: row(1) and row(1).get("otlozhena") and not row(1).get("nazad") and not row(1).get("vpered"),
             "Native OnBeforeClose did not retain one sleeping row and clear tab history", 35)
        dropped = False
        for attempt in range(6):
            after = memory(f"after_sleep_{attempt}")
            if before - after >= a.min_drop_mb * 1024**2:
                dropped = True
                break
            time.sleep(1)
        if not dropped:
            raise AssertionError("Closing the actual page did not reduce total private commit by the required amount")
        report["checks"].append({"native_closed_memory_reclaimed": True, "drop_private_bytes": before - after})
        loads = sum(r.get("page") == "MemoryHeap" for r in receipts)
        tab("MemoryHeap")
        wait(lambda: sum(r.get("page") == "MemoryHeap" for r in receipts) > loads
             and row(1) and not row(1).get("otlozhena") and not row(1).get("zagruzka"), "Sleeping row did not actually reload")
        wake = [r for r in receipts if r.get("page") == "MemoryHeap"][-1]
        if wake.get("seen") != "preserved" or "memoryfixture=preserved" not in wake.get("cookie", ""):
            raise AssertionError("Cookies/localStorage were lost across real unload/wake")
        if row(1).get("nazad") or row(1).get("vpered"):
            raise AssertionError("Woken browser unexpectedly retained pre-unload navigation history")
        report["checks"].append({"same_row_woke_storage_preserved_history_reset": True})
        tab("MemoryPinned")
        wait(lambda: row(3) and not row(3).get("otlozhena") and not row(3).get("zagruzka"), "Pinned tab did not load")
        tab("MemoryKeeper")
        protected("MemoryPinned", 3)
        tab("MemoryLoading")
        wait(lambda: row(5) and row(5).get("zagruzka"), "Slow page did not remain actually loading")
        tab("MemoryKeeper")
        protected("MemoryLoading", 5)
        release_loading.set()
        wait(lambda: row(5) and not row(5).get("zagruzka"), "Slow fixture did not finish")
        tab("MemoryTools")
        wait(lambda: row(4) and not row(4).get("otlozhena") and not row(4).get("zagruzka"), "Tools fixture did not load")
        click("MemoryTools", ["TabItem"], "right")
        click("Инструменты разработчика", ["MenuItem", "Button"])
        wait(lambda: find("Закрыть инструменты разработчика", ["Button"]), "Actual DevTools did not open")
        tab("MemoryKeeper")
        protected("MemoryTools", 4)
        tab("MemoryTools")
        click("Закрыть инструменты разработчика", ["Button"])
        tab("MemoryDownload")
        wait(lambda: any(r.get("page") == "MemoryDownload" for r in receipts), "Download page did not load")
        physical_page_click()
        wait(lambda: hits.get("/download", 0), "Actual download did not start")
        tab("MemoryKeeper")
        protected("MemoryDownload", 6)
        gui.hotkey("ctrl", "j")
        click("Пауза", ["Button"])
        wait(lambda: find("Продолжить", ["Button"]), "Actual download did not pause")
        click("Закрыть библиотеку", ["Button"])
        protected("MemoryDownload", 6)
        gui.hotkey("ctrl", "j")
        click("Отменить", ["Button"])
        click("Закрыть библиотеку", ["Button"])
        stop_download.set()
        tab("MemoryBeforeUnload")
        wait(lambda: any(r.get("page") == "MemoryBeforeUnload" for r in receipts), "Beforeunload fixture did not load")
        physical_page_click()
        wait(lambda: any(r.get("guard") and r.get("trusted") for r in receipts), "Beforeunload guard was not armed by an actual trusted gesture")
        tab("MemoryKeeper")
        for attempt in range(2):
            prior = native_logs().count("refused=beforeunload")
            warning_confirm("MemoryBeforeUnload")
            wait(lambda: native_logs().count("refused=beforeunload") > prior,
                 "Native beforeunload refusal/retry callback was not observed")
            wait(lambda: any("осталась в памяти" in c.element_info.name for c in controls()),
                 "Beforeunload refusal was not visible to the owner")
            if not row(7) or row(7).get("otlozhena"):
                raise AssertionError("Refused beforeunload removed or slept the live row")
        report["coverage"]["beforeunload_refusal"] = "actual_native_refusal_and_retry"
        report["checks"].append({"actual_beforeunload_veto_live_retry": True})
        tab("MemoryDraft")
        wait(lambda: any(r.get("page") == "MemoryDraft" for r in receipts), "Draft fixture did not load")
        physical_page_click(150)
        gui.write("actual unsaved draft", interval=.03)
        tab("MemoryKeeper")
        protected("MemoryDraft", 8)
        click("MemoryKeeper", ["TabItem"], "right")
        click("Новая приватная вкладка", ["MenuItem", "Button"])
        navigate(base + "/private")
        wait(lambda: any(r.get("page") == "MemoryPrivate" for r in receipts), "Actual private tab did not load")
        tab("MemoryKeeper")
        protected("MemoryPrivate")
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
    p.add_argument("--timeout", type=int, default=480)
    a = p.parse_args()
    if not (128 <= a.heap_mb <= 512 and 32 <= a.min_drop_mb <= a.heap_mb // 2 and 120 <= a.timeout <= 900):
        p.error("Heap/drop/time outside bounded fixture limits")
    run(a)


if __name__ == "__main__":
    main()
