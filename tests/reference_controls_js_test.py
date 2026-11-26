#!/usr/bin/env python3
"""Offline JS-регрессия фиксированного readonly SCRIPT из
desktop/src-tauri/src/cef_youtube_reference_probe.rs.

SCRIPT извлекается из Rust raw-string r#"..."# канонического источника и
исполняется под node (stdlib, без jsdom/npm) поверх tiny fake DOM, который
реализует ровно те API, что SCRIPT реально вызывает. Проверяются только
логические решения SCRIPT'а (выбор rect'ов, guard'ы, отсутствие утечки
текста в результат) — это НЕ доказательство реального UI, декодера или
воспроизведения.

Запуск: python3 tests/reference_controls_js_test.py [--source PATH]
"""

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

DEFAULT_SOURCE = (
    Path(__file__).resolve().parents[1]
    / "desktop"
    / "src-tauri"
    / "src"
    / "cef_youtube_reference_probe.rs"
)

SCRIPT_RE = re.compile(r'const SCRIPT: &str = r#"(.*?)"#;', re.S)

# Маркеры канонической (post-23aa/d80) версии SCRIPT. Если их нет — прочитан
# устаревший источник без controls, и тест обязан упасть, а не «пройти».
REQUIRED_MARKERS = (
    "controls",
    "ytp-menuitem-label",
    "menuitemradio",
    "elementFromPoint",
    "ytp-autohide",
)

HARNESS = r"""
'use strict';
// ---- tiny fake DOM: только то, что реально использует фиксированный SCRIPT ----
function matchOne(el, sel) {
  sel = sel.trim();
  if (sel[0] === '.') return el.classes.has(sel.slice(1));
  if (sel[0] === '#') { const p = sel.slice(1).split('.'); return el.id === p[0] && p.slice(1).every(c => el.classes.has(c)); }
  const m = sel.match(/^\[role=([a-z]+)\]$/);
  if (m) return el.attrs.role === m[1];
  if (/^[a-z][a-z0-9]*$/.test(sel)) return el.tag === sel;
  throw new Error('unsupported selector: ' + sel);
}
function queryAll(root, sel) {
  const sels = sel.split(',');
  const out = [];
  (function walk(n) { for (const c of n.children) { if (sels.some(s => matchOne(c, s))) out.push(c); walk(c); } })(root);
  return out;
}
class FakeNode {
  constructor(o) {
    o = o || {};
    this.tag = (o.tag || 'div').toLowerCase();
    this.id = o.id || '';
    this.classes = new Set(o.classes || []);
    this.attrs = o.attrs || {};
    this._text = o.text || '';
    this.rect = o.rect || null;
    this.display = o.display || 'block';
    this.visibility = o.visibility || 'visible';
    this.children = [];
    this.parent = null;
    if (o.videoData) this._videoData = o.videoData;
    if (o.quality) this._quality = o.quality;
    if (o.videoProps) Object.assign(this, o.videoProps);
    for (const c of (o.children || [])) this.append(c);
  }
  append(c) { c.parent = this; this.children.push(c); return c; }
  get classList() { const s = this.classes; return { contains: c => s.has(c) }; }
  get textContent() { return this._text + this.children.map(c => c.textContent).join(''); }
  getBoundingClientRect() {
    if (!this.rect) throw new Error('FakeNode without rect: ' + this.tag);
    const r = this.rect;
    return { x: r.x, y: r.y, width: r.width, height: r.height, left: r.x, top: r.y, right: r.x + r.width, bottom: r.y + r.height };
  }
  querySelectorAll(sel) { return queryAll(this, sel); }
  querySelector(sel) { const r = queryAll(this, sel); return r.length ? r[0] : null; }
  contains(other) { for (let n = other; n; n = n.parent) if (n === this) return true; return false; }
  getVideoData() { if (!this._videoData) throw new Error('no getVideoData'); return this._videoData; }
  getVideoPlaybackQuality() { if (!this._quality) throw new Error('no getVideoPlaybackQuality'); return this._quality; }
  click() { throw new Error('mutation forbidden: click'); }
  play() { throw new Error('mutation forbidden: play'); }
  pause() { throw new Error('mutation forbidden: pause'); }
  focus() { throw new Error('mutation forbidden: focus'); }
  scrollIntoView() { throw new Error('mutation forbidden: scrollIntoView'); }
}
function makeDocument(root, hits) {
  const doc = {
    _hits: hits || {},
    querySelectorAll: sel => queryAll(root, sel),
    querySelector(sel) { const r = queryAll(root, sel); return r.length ? r[0] : null; },
    elementFromPoint(x, y) {
      const k = Math.round(x) + ',' + Math.round(y);
      if (Object.prototype.hasOwnProperty.call(doc._hits, k)) return doc._hits[k];
      // по умолчанию «перекрытий нет»: последний в document order узел, чей rect содержит точку
      const containing = [];
      (function walk(n) {
        for (const c of n.children) {
          if (c.rect && x >= c.rect.x && x < c.rect.x + c.rect.width && y >= c.rect.y && y < c.rect.y + c.rect.height) containing.push(c);
          walk(c);
        }
      })(root);
      return containing.length ? containing[containing.length - 1] : null;
    }
  };
  return doc;
}
// ---- фикстура ----
const VID = 'dQw4w9WgXcQ';
function node(o) { return new FakeNode(o); }
function videoNode(rect) {
  return node({ tag: 'video', rect: rect,
    videoProps: { volume: 0.5, muted: false, sinkId: '', paused: false, ended: false, seeking: false,
      readyState: 4, currentTime: 12.5, duration: 300, videoWidth: 1280, videoHeight: 720, playbackRate: 1 },
    quality: { totalVideoFrames: 5000, droppedVideoFrames: 3 } });
}
function baseFixture(opts) {
  opts = opts || {};
  const video = videoNode({ x: 0, y: 0, width: 1280, height: 720 });
  const play = node({ tag: 'button', classes: ['ytp-play-button'], rect: { x: 16, y: 756, width: 48, height: 36 } });
  const settings = node({ tag: 'button', classes: ['ytp-settings-button'],
    rect: opts.settingsRect || { x: 1216, y: 756, width: 48, height: 36 } });
  const quality = node({ attrs: { role: 'menuitem' }, rect: { x: 880, y: 560, width: 240, height: 32 },
    children: [node({ classes: ['ytp-menuitem-label'], text: 'Quality' }), node({ text: ' Auto (480p)' })] });
  const p720 = node({ attrs: { role: 'menuitemradio' }, rect: { x: 880, y: 592, width: 240, height: 32 },
    children: [node({ classes: ['ytp-menuitem-label'], text: '720p' }), node({ text: ' HD' })] });
  const playerClasses = ['html5-video-player'];
  if (opts.autohide) playerClasses.push('ytp-autohide');
  if (opts.ad) playerClasses.push('ad-showing');
  const playerChildren = [video, play, settings, quality, p720];
  if (opts.twoVideos) playerChildren.push(videoNode({ x: 0, y: 0, width: 640, height: 360 }));
  const player = node({ id: 'movie_player', classes: playerClasses, rect: { x: 0, y: 0, width: 1280, height: 800 },
    videoData: { video_id: opts.videoId || VID }, children: playerChildren });
  const reject = node({ tag: 'button', text: 'Reject all', rect: { x: 24, y: 24, width: 120, height: 32 } });
  const bodyChildren = [player, reject];
  if (opts.extraDocSettings) bodyChildren.push(node({ tag: 'button', classes: ['ytp-settings-button'], rect: { x: 600, y: 24, width: 40, height: 32 } }));
  if (opts.markerButton) bodyChildren.push(node({ tag: 'button', text: 'PRIVATE_MARKER_QQ77', rect: { x: 200, y: 24, width: 100, height: 32 } }));
  const body = node({ tag: 'body', rect: { x: 0, y: 0, width: 1280, height: 800 }, children: bodyChildren });
  return { body: body, play: play, settings: settings, quality: quality, p720: p720, reject: reject };
}
function runScript() {
  return (__SCRIPT__);
}
function runCase(opts, hits) {
  const f = baseFixture(opts);
  globalThis.document = makeDocument(f.body, hits);
  globalThis.location = { href: 'https://www.youtube.com/watch?v=' + VID };
  globalThis.innerWidth = 1280;
  globalThis.innerHeight = 800;
  globalThis.getComputedStyle = el => ({ display: el.display, visibility: el.visibility });
  try { return { ok: true, value: runScript() }; }
  catch (e) { return { ok: false, error: String((e && e.message) || e) }; }
}
// центр play-кнопки: (16+48/2, 756+36/2) = (40, 774)
const overlay = node({ rect: { x: 0, y: 700, width: 1280, height: 100 } });
const cases = {
  happy: runCase({}),
  overlay_blocks_play: runCase({}, { '40,774': overlay }),
  offscreen_settings: runCase({ settingsRect: { x: 1240, y: 756, width: 70, height: 36 } }),
  autohide: runCase({ autohide: true }),
  doc_scope_settings: runCase({ extraDocSettings: true }),
  marker_button: runCase({ markerButton: true }),
  ad_showing: runCase({ ad: true }),
  video_id_mismatch: runCase({ videoId: 'aaaaaaaaaaa' }),
  two_videos: runCase({ twoVideos: true })
};
console.log(JSON.stringify(cases));
"""

SOURCE_ARG = None


class ReferenceControlsJsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if shutil.which("node") is None:
            raise unittest.SkipTest("node недоступен")
        source = Path(SOURCE_ARG) if SOURCE_ARG else DEFAULT_SOURCE
        text = source.read_text(encoding="utf-8")
        match = SCRIPT_RE.search(text)
        if not match:
            raise AssertionError(f"SCRIPT raw-string не найден в {source}")
        script = match.group(1)
        for marker in REQUIRED_MARKERS:
            if marker not in script:
                raise AssertionError(
                    f"SCRIPT не содержит '{marker}' — прочитана не каноническая версия ({source})"
                )
        cls._tmp = tempfile.TemporaryDirectory(prefix="controls-js-")
        harness_path = Path(cls._tmp.name) / "harness.js"
        harness_path.write_text(HARNESS.replace("__SCRIPT__", script), encoding="utf-8")
        proc = subprocess.run(
            ["node", str(harness_path)], capture_output=True, text=True, timeout=60
        )
        if proc.returncode != 0:
            raise AssertionError(f"node exit {proc.returncode}: {proc.stderr.strip()}")
        cls.cases = json.loads(proc.stdout)

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def case(self, name):
        entry = self.cases[name]
        self.assertTrue(
            entry["ok"], f"SCRIPT бросил исключение в кейсе {name}: {entry.get('error')}"
        )
        return entry["value"]

    def test_01_happy_path_values(self):
        r = self.case("happy")
        self.assertIsNotNone(r)
        self.assertEqual(r["volume"], 0.5)
        self.assertEqual(r["video_count"], 1)
        self.assertTrue(r["default_sink"])
        self.assertTrue(r["playing"])
        self.assertEqual(r["decoded"], 5000)
        self.assertEqual(r["dropped"], 3)
        self.assertEqual(r["controls"]["viewport"], {"width": 1280, "height": 800})
        self.assertEqual(
            r["controls"]["play"], {"x": 16, "y": 756, "width": 48, "height": 36}
        )

    def test_02_quality_row_matches_label_span_not_row_text(self):
        # textContent всей строки — "Quality Auto (480p)"; матч по нему дал бы null.
        # Rect обязан найтись по .ytp-menuitem-label с textContent "Quality".
        r = self.case("happy")
        self.assertEqual(
            r["controls"]["quality"], {"x": 880, "y": 560, "width": 240, "height": 32}
        )

    def test_03_p720_menuitemradio_distinct_from_menuitem(self):
        # 720p живёт в role=menuitemradio; запрос обязан включать обе роли,
        # а quality не должен перепутаться с p720 и наоборот.
        r = self.case("happy")
        self.assertEqual(
            r["controls"]["p720"], {"x": 880, "y": 592, "width": 240, "height": 32}
        )
        self.assertNotEqual(r["controls"]["p720"], r["controls"]["quality"])

    def test_04_overlay_hit_blocks_play_but_reject_all_stays(self):
        r = self.case("overlay_blocks_play")
        self.assertIsNone(r["controls"]["play"])
        self.assertIsNotNone(r["controls"]["settings"])
        self.assertTrue(r["controls"]["reject_all_present"])
        self.assertEqual(
            r["controls"]["reject_all"], {"x": 24, "y": 24, "width": 120, "height": 32}
        )

    def test_05_partly_offscreen_rect_is_null_even_if_center_inside(self):
        # settings: right = 1240+70 = 1310 > innerWidth 1280, центр (1275,774)
        # при этом внутри viewport — контракт требует null по полным bounds.
        r = self.case("offscreen_settings")
        self.assertIsNone(r["controls"]["settings"])
        self.assertIsNotNone(r["controls"]["play"])

    def test_06_autohide_hides_play_and_settings(self):
        r = self.case("autohide")
        self.assertIsNone(r["controls"]["play"])
        self.assertIsNone(r["controls"]["settings"])
        self.assertIsNotNone(r["controls"]["quality"])
        self.assertIsNotNone(r["controls"]["p720"])

    def test_07_settings_scoped_to_target_player(self):
        # Второй .ytp-settings-button существует вне player; глобальный запрос
        # дал бы два видимых rect -> null. Scoped-запрос обязан вернуть внутренний.
        r = self.case("doc_scope_settings")
        self.assertEqual(
            r["controls"]["settings"], {"x": 1216, "y": 756, "width": 48, "height": 36}
        )

    def test_08_unknown_button_private_text_not_leaked(self):
        r = self.case("marker_button")
        self.assertIsNotNone(r)
        self.assertNotIn("PRIVATE_MARKER", json.dumps(r))

    def test_09_ad_showing_returns_null(self):
        self.assertIsNone(self.case("ad_showing"))

    def test_10_video_id_mismatch_returns_null(self):
        self.assertIsNone(self.case("video_id_mismatch"))

    def test_11_two_videos_returns_null(self):
        self.assertIsNone(self.case("two_videos"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source",
        default=None,
        help="путь к cef_youtube_reference_probe.rs (default: относительно этого файла)",
    )
    args, rest = parser.parse_known_args()
    global SOURCE_ARG
    SOURCE_ARG = args.source
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(ReferenceControlsJsTest)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    failed = {t.id().rsplit(".", 1)[-1] for t, _ in result.failures + result.errors}
    names = sorted(n for n in dir(ReferenceControlsJsTest) if n.startswith("test_"))
    checks = {n: (n not in failed) for n in names}
    print(
        "RECEIPT "
        + json.dumps(
            {"total": result.testsRun, "failed": len(failed), "checks": checks},
            sort_keys=True,
        )
    )
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
