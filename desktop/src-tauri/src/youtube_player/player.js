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
  const quality = document.getElementById('quality');
  const qualities = new Map();
  const sponsor = document.getElementById('sponsor');
  let segments = [];
  let configured = false, disposed = false, intended = false, separate = false;
  let epoch = 0, syncing = false, duration = 0, timer = null;
  const setStatus = text => { status.textContent = text; };
  const tick = ready => console.debug(ready ? '__NEXUSBRO_LIGHT_READY__' : '__NEXUSBRO_LIGHT_TICK__');
  const media = () => separate ? [video, audio] : [video];
  function pause() { ++epoch; intended = false; syncing = false; clearInterval(timer); timer = null; media().forEach(m => m.pause()); button.textContent = 'Воспроизвести'; updateView(); tick(false); }
  function error() { pause(); setStatus('Не удалось воспроизвести видео. Можно открыть обычный YouTube.'); }
  function align() {
    if (!separate || syncing || !Number.isFinite(video.currentTime)) return;
    if (Math.abs(audio.currentTime - video.currentTime) > 0.12) video.currentTime = audio.currentTime;
    audio.playbackRate = video.playbackRate;
  }
  async function play() {
    if (!configured || disposed) return;
    intended = true; const run = ++epoch;
    if (timer === null) timer = setInterval(updateView, 250);
    if (media().every(m => m.readyState >= 3)) syncing = false;
    for (const m of media()) m.preload = 'auto';
    try {
      align(); await Promise.all(media().map(m => m.play()));
      if (run !== epoch) return;
      if (disposed || !intended) { media().forEach(m => m.pause()); return; }
      button.textContent = 'Пауза'; setStatus(''); tick(false);
    } catch (_) { if (run === epoch && intended && !syncing) error(); }
  }
  function sourcesClear() {
    for (const m of [video, audio]) { m.pause(); m.removeAttribute('src'); m.load(); }
  }
  function dispose() { disposed = true; ++epoch; pause(); sourcesClear(); clearInterval(timer); }
  button.addEventListener('click', () => intended ? pause() : void play());
  seek.addEventListener('input', () => {
    if (!duration || !configured) return;
    const t = Math.min(duration, Math.max(0, Number(seek.value)));
    video.currentTime = t; if (separate) audio.currentTime = t; tick(false);
  });
  quality.addEventListener('change', () => {
    const next = qualities.get(quality.value); if (!next || disposed) return;
    const resume = intended, position = separate ? audio.currentTime : video.currentTime;
    pause(); const change = epoch; video.src = next;
    setStatus('Смена качества…');
    video.addEventListener('loadedmetadata', () => {
      if (disposed || epoch !== change) return;
      const t = Math.min(position, Number.isFinite(video.duration) ? video.duration : position);
      video.currentTime = t; if (separate) audio.currentTime = t;
      if (resume) void play(); else setStatus('Готово к воспроизведению');
    }, {once:true});
    video.load();
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
  for (const m of [video, audio]) m.addEventListener('ended', () => { pause(); setStatus('Просмотр завершён'); });
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
  function updateView() {
    if (disposed || !configured) return;
    duration = Number.isFinite(video.duration) ? video.duration : 0;
    if (separate && Number.isFinite(audio.duration)) duration = Math.min(duration, audio.duration);
    seek.max = String(duration); seek.value = String(video.currentTime || 0);
    clock.textContent = `${Math.floor(video.currentTime || 0)} / ${Math.floor(duration)} с`;
    if (intended && !syncing) {
      align();
      if (sponsor.checked && duration > 0) {
        const current = separate ? audio.currentTime : video.currentTime;
        const skip = segments.find(s => current >= s[0] && current < s[1] - 0.2 && s[1] <= duration);
        if (skip) { video.currentTime = skip[1]; if (separate) audio.currentTime = skip[1]; tick(false); setStatus('Пропущена интеграция SponsorBlock'); }
      }
    }
  }
  for (const m of [video, audio]) { m.addEventListener('loadedmetadata', updateView); m.addEventListener('seeked', updateView); }
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
      if (config.qualities && (!Array.isArray(config.qualities) || config.qualities.length > 256)) return false;
      for (const item of config.qualities || []) {
        if (!item || typeof item.id !== 'string' || item.id.length > 128 || typeof item.label !== 'string' || !valid(item.url)) return false;
      }
      if (typeof config.source === 'string' && /^https:\/\/www\.youtube\.com\/watch\?v=[A-Za-z0-9_-]{11}$/.test(config.source)) {
        const ordinary = document.getElementById('ordinary');
        ordinary.href = config.source; ordinary.hidden = false;
      }
      configured = true; separate = Boolean(config.audio);
      for (const item of config.qualities || []) {
        if (qualities.has(item.id)) continue;
        qualities.set(item.id, item.url);
        const option = document.createElement('option'); option.value = item.id; option.textContent = item.label.slice(0,80);
        option.selected = item.url === config.video; quality.append(option);
      }
      quality.hidden = qualities.size < 2;
      document.getElementById('title').textContent = config.title.slice(0, 512);
      document.title = config.title.slice(0, 512) + ' — Лёгкий YouTube';
      let restored = false;
      const restore = () => {
        if (restored || video.readyState < 1 || (separate && audio.readyState < 1)) return;
        restored = true;
        const position = typeof config.position === 'number' && Number.isFinite(config.position) && config.position > 0 && config.position < video.duration - 5 ? config.position : 0;
        video.currentTime = position; if (separate) audio.currentTime = position;
        updateView(); tick(true);
      };
      video.addEventListener('loadedmetadata', restore);
      audio.addEventListener('loadedmetadata', restore);
      video.muted = separate; video.src = config.video;
      if (separate) audio.src = config.audio;
      button.disabled = false; setStatus(config.resume_available === false ? 'Готово к воспроизведению · сохранение позиции недоступно' : 'Готово к воспроизведению'); return true;
    },
    setAux(data) {
      if (disposed || !configured || !data) return;
      if (Array.isArray(data.segments) && data.segments.length <= 500) {
        segments = data.segments.filter(s => Array.isArray(s) && s.length === 2 && s.every(Number.isFinite) && s[0] >= 0 && s[1] > s[0] && s[1] <= 604800);
        sponsor.title = `Сегменты SponsorBlock: ${segments.length}`;
      } else { segments = []; sponsor.title = 'SponsorBlock временно недоступен'; }
      const ratings = document.getElementById('ratings');
      if (Number.isSafeInteger(data.dislikes) && data.dislikes >= 0) {
        ratings.textContent = 'Дизлайки: ' + new Intl.NumberFormat('ru-RU', {notation:'compact'}).format(data.dislikes);
        ratings.title = 'Оценка Return YouTube Dislike';
      } else { ratings.textContent = 'Дизлайки недоступны'; ratings.title = 'Сервис Return YouTube Dislike временно недоступен'; }
    },
    dispose,
    snapshot() {
      return {configured, disposed, playing: intended && !video.paused && (!separate || !audio.paused),
        position: video.currentTime || 0, audio_position: separate ? audio.currentTime || 0 : video.currentTime || 0, duration, separate,
        video_width: video.videoWidth || 0, video_height: video.videoHeight || 0,
        skew: separate ? Math.abs(audio.currentTime - video.currentTime) : 0,
        decoded: video.getVideoPlaybackQuality?.().totalVideoFrames || 0,
        dropped: video.getVideoPlaybackQuality?.().droppedVideoFrames || 0};
    }
  }), writable: false, configurable: false});
})();
