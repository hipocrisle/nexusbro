'use strict';
// Controller regression tests with explicit media events. These are not native
// playback, real decoder, YouTube availability, or resource-acceptance evidence.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../desktop/src-tauri/src/youtube_player/player.js'), 'utf8');
function fixture(separate = true) {
  class Element {
    constructor() { this.events = new Map(); this.paused = true; this.readyState = 4; this.currentTime = 12; this.duration = 120; this.options = []; this.value = ''; this.volume = 1; this.playCalls = 0; }
    addEventListener(name, callback, options) {
      const entries = this.events.get(name) || []; entries.push({callback, once: options?.once}); this.events.set(name, entries);
    }
    removeEventListener(name, callback) { this.events.set(name, (this.events.get(name) || []).filter(x => x.callback !== callback)); }
    emit(name) { for (const entry of [...(this.events.get(name) || [])]) { if (entry.once) this.removeEventListener(name, entry.callback); entry.callback({target: this}); } }
    play() { this.playCalls++; this.paused = false; return Promise.resolve(); }
    pause() { this.paused = true; }
    load() {}
    removeAttribute(name) { delete this[name]; }
    append(option) { this.options.push(option); }
  }
  const elements = new Map(); const document = new Element(); document.hidden = false;
  document.getElementById = name => { if (!elements.has(name)) elements.set(name, new Element()); return elements.get(name); };
  document.createElement = () => new Element();
  const window = new Element(); let timers = 0;
  vm.runInNewContext(source, {document, window, URL, Intl, console: {debug() {}}, setInterval() { timers++; return timers; }, clearInterval() {}, VTTCue: class {}});
  const video = document.getElementById('video'), audio = document.getElementById('audio');
  assert.equal(window.NB_LIGHT.configure({title: 'Fixture', video: 'https://fixture.googlevideo.com/videoplayback?video', ...(separate ? {audio: 'https://fixture.googlevideo.com/videoplayback?audio'} : {})}), true);
  return {video, audio, player: window.NB_LIGHT,
    click() { document.getElementById('play').emit('click'); },
    visibility(hidden) { document.hidden = hidden; document.emit('visibilitychange'); },
    status() { return document.getElementById('status').textContent; }};
}
const settle = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
test('hidden video waiting does not pause separate audio', async () => {
  const f = fixture(); f.click(); await settle(); f.visibility(true);
  f.video.readyState = 2; f.video.emit('waiting');
  assert.equal(f.audio.paused, false); assert.notEqual(f.status(), 'Буферизация…');
});
test('audio buffering resumes in background without waiting for suspended video', async () => {
  const f = fixture(); f.click(); await settle(); f.visibility(true);
  f.video.readyState = 2; f.audio.readyState = 2; f.audio.emit('waiting');
  assert.equal(f.audio.paused, true); assert.equal(f.status(), 'Буферизация…');
  const videoCalls = f.video.playCalls;
  f.audio.readyState = 4; f.audio.emit('canplay'); await settle();
  assert.equal(f.audio.paused, false); assert.equal(f.video.playCalls, videoCalls);
});
test('hiding during a visible video stall releases ready audio', async () => {
  const f = fixture(); f.click(); await settle();
  f.video.readyState = 2; f.video.emit('waiting'); assert.equal(f.audio.paused, true);
  f.visibility(true); await settle(); assert.equal(f.audio.paused, false);
});
test('visible two-track buffering still waits for both tracks', async () => {
  const f = fixture(); f.click(); await settle();
  f.video.readyState = 2; f.video.emit('waiting'); f.audio.emit('canplay'); await settle();
  assert.equal(f.audio.paused, true);
  f.video.readyState = 4; f.video.emit('canplay'); await settle(); assert.equal(f.audio.paused, false);
});
test('returning to foreground realigns video to the running audio clock', async () => {
  const f = fixture(); f.click(); await settle(); f.visibility(true);
  f.video.currentTime = 12; f.audio.currentTime = 45; f.video.paused = true;
  f.visibility(false); await settle();
  assert.equal(f.video.currentTime, 45); assert.equal(f.video.paused, false); assert.equal(f.audio.paused, false);
});
test('user pause and disposal cannot be undone by visibility or canplay', async () => {
  const f = fixture(); f.click(); await settle(); f.click();
  f.visibility(true); f.visibility(false); f.audio.emit('canplay'); await settle();
  assert.equal(f.audio.paused, true); assert.equal(f.video.paused, true);
  f.player.dispose(); f.visibility(true); f.visibility(false); f.audio.emit('canplay'); await settle();
  assert.equal(f.audio.paused, true); assert.equal(f.video.paused, true);
});
test('muxed background media keeps its own buffering guard', async () => {
  const f = fixture(false); f.click(); await settle(); f.visibility(true);
  f.video.readyState = 2; f.video.emit('waiting'); assert.equal(f.video.paused, true);
  f.video.readyState = 4; f.video.emit('canplay'); await settle(); assert.equal(f.video.paused, false);
});
