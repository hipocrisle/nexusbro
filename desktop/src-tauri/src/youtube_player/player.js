/* Native CEF prototype: separate elementary WebM streams. No remote scripts,
 * cookies, service workers, iframe, privileged JS bridge or implicit autoplay.
 * Native integration must bind this document and its sources to one route.
 */
(() => {
  'use strict';
  const video = document.getElementById('video');
  const audio = document.getElementById('audio');
  const button = document.getElementById('play');
  const seek = document.getElementById('seek');
  const status = document.getElementById('status');
  const clock = document.getElementById('clock');
  let configured = false, disposed = false, intended = false, separate = false;
  let epoch = 0, syncing = false, duration = 0;
  const setStatus = text => { status.textContent = text; };
  const media = () => separate ? [video, audio] : [video];
  function pause() { intended = false; media().forEach(m => m.pause()); button.textContent = 'Воспроизвести'; }
  function error() { pause(); setStatus('Не удалось воспроизвести видео. Можно открыть обычный YouTube.'); }
  function align() {
    if (!separate || syncing || !Number.isFinite(video.currentTime)) return;
    if (Math.abs(audio.currentTime - video.currentTime) > 0.12) video.currentTime = audio.currentTime;
    audio.playbackRate = video.playbackRate;
  }
  async function play() {
    if (!configured || disposed) return;
    intended = true; const run = ++epoch;
    try {
      align(); await Promise.all(media().map(m => m.play()));
      if (run !== epoch) return;
      if (disposed || !intended) { media().forEach(m => m.pause()); return; }
      button.textContent = 'Пауза'; setStatus('');
    } catch (_) { if (run === epoch) error(); }
  }
  function sourcesClear() {
    for (const m of [video, audio]) { m.pause(); m.removeAttribute('src'); m.load(); }
  }
  function dispose() { disposed = true; ++epoch; pause(); sourcesClear(); clearInterval(timer); }
  button.addEventListener('click', () => intended ? pause() : void play());
  seek.addEventListener('input', () => {
    if (!duration || !configured) return;
    const t = Math.min(duration, Math.max(0, Number(seek.value)));
    video.currentTime = t; if (separate) audio.currentTime = t;
  });
  document.getElementById('volume').addEventListener('input', event => {
    const v = Math.min(1, Math.max(0, Number(event.target.value)));
    (separate ? audio : video).volume = v;
  });
  document.getElementById('fullscreen').addEventListener('click', () => {
    const p = document.fullscreenElement ? document.exitFullscreen() : document.getElementById('player').requestFullscreen();
    p?.catch(() => setStatus('Полноэкранный режим недоступен'));
  });
  for (const m of [video, audio]) m.addEventListener('error', error);
  video.addEventListener('ended', () => { pause(); setStatus('Просмотр завершён'); });
  // Pause both clocks during a buffer stall; resume only the user's play intent.
  for (const m of [video, audio]) {
    m.addEventListener('waiting', () => {
      if (!intended) return; syncing = true; media().forEach(item => item.pause()); setStatus('Буферизация…');
    });
    m.addEventListener('canplay', () => {
      if (syncing && intended && media().every(item => item.readyState >= 3)) {
        syncing = false; align(); void play();
      }
    });
  }
  const timer = setInterval(() => {
    if (disposed || !configured) return;
    duration = Number.isFinite(video.duration) ? video.duration : 0;
    seek.max = String(duration); seek.value = String(video.currentTime || 0);
    clock.textContent = `${Math.floor(video.currentTime || 0)} / ${Math.floor(duration)} с`;
    if (intended && !syncing) align();
  }, 250);
  window.addEventListener('pagehide', dispose, {once: true});
  // Called only by the native document controller after route/generation checks.
  // This object grants NO native privileges; a web page cannot obtain a proxy.
  Object.defineProperty(window, 'NB_LIGHT', {value: Object.freeze({
    configure(config) {
      if (disposed || configured || !config || typeof config.title !== 'string') return false;
      const valid = value => {
        try { const u = new URL(value); return u.protocol === 'https:' && !u.username && !u.password && !u.port && /(^|\.)googlevideo\.com$/.test(u.hostname); }
        catch (_) { return false; }
      };
      if (!valid(config.video) || (config.audio && !valid(config.audio))) return false;
      configured = true; separate = Boolean(config.audio);
      document.getElementById('title').textContent = config.title.slice(0, 512);
      video.muted = separate; video.src = config.video;
      if (separate) audio.src = config.audio;
      button.disabled = false; setStatus('Готово к воспроизведению'); return true;
    },
    dispose,
    snapshot() {
      return {configured, disposed, playing: intended && !video.paused && (!separate || !audio.paused),
        position: video.currentTime || 0, duration, separate,
        skew: separate ? Math.abs(audio.currentTime - video.currentTime) : 0,
        decoded: video.getVideoPlaybackQuality?.().totalVideoFrames || 0,
        dropped: video.getVideoPlaybackQuality?.().droppedVideoFrames || 0};
    }
  }), writable: false, configurable: false});
})();
