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
  const downloadVideo = document.getElementById('download-video');
  const downloadAudio = document.getElementById('download-audio');
  const downloadCancel = document.getElementById('download-cancel');
  const downloadStatus = document.getElementById('download-status');
  downloadVideo.addEventListener('click', () => { if (configured && !disposed) console.debug('__NEXUSBRO_LIGHT_DOWNLOAD__:video:' + quality.value); });
  downloadAudio.addEventListener('click', () => { if (configured && !disposed) console.debug('__NEXUSBRO_LIGHT_DOWNLOAD__:audio'); });
  downloadCancel.addEventListener('click', () => { if (configured && !disposed) console.debug('__NEXUSBRO_LIGHT_DOWNLOAD__:cancel'); });

  const sponsor = document.getElementById('sponsor');
  const captions = document.getElementById('captions');
  const captionStatus = document.getElementById('caption-status');
  let textTrack = null;
  let restoreMetadata = null;
  function restoreClear() {
    if (!restoreMetadata) return;
    video.removeEventListener('loadedmetadata', restoreMetadata);
    audio.removeEventListener('loadedmetadata', restoreMetadata);
    restoreMetadata = null;
  }
  function captionsClear() {
    if (!textTrack) return;
    textTrack.mode = 'disabled';
    for (const cue of Array.from(textTrack.cues || [])) textTrack.removeCue(cue);
  }
  captions.addEventListener('change', () => {
    captionsClear(); captionStatus.textContent = '';
    if (!disposed && configured) console.debug('__NEXUSBRO_LIGHT_CAPTION__:' + captions.value);
  });
  let segments = [];
  let configured = false, disposed = false, intended = false, separate = false;
  let epoch = 0, syncing = false, duration = 0, timer = null;
  const setStatus = text => { status.textContent = text; };
  const tick = ready => console.debug(ready ? '__NEXUSBRO_LIGHT_READY__' : '__NEXUSBRO_LIGHT_TICK__');
  const media = () => separate ? [video, audio] : [video];
  function pause() { ++epoch; intended = false; syncing = false; clearInterval(timer); timer = null; media().forEach(m => m.pause()); button.textContent = 'Воспроизвести'; updateView(); tick(false); }
  function error() { if (disposed) return; pause(); setStatus('Не удалось воспроизвести видео. Можно открыть обычный YouTube.'); }
  function align() {
    // Chromium may suspend muted hidden video while audio keeps playing.
    // Seeking that suspended element repeatedly defeats the native saving.
    if (!separate || syncing || document.hidden || (video.paused && !audio.paused) || !Number.isFinite(video.currentTime)) return;
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
  function dispose() {
    disposed = true; ++epoch; pause(); sourcesClear();
    restoreClear(); qualities.clear(); segments = []; captionsClear();
    clearInterval(timer); button.disabled = true; quality.disabled = true; captions.disabled = true;
    downloadVideo.disabled = true; downloadAudio.disabled = true; downloadCancel.hidden = true;
    setStatus('Плеер остановлен. Обновите вкладку, чтобы продолжить просмотр.');
  }
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
      if (Array.isArray(config.captions) && config.captions.length <= 256) {
        config.captions.forEach((item, index) => {
          if (!item || typeof item.label !== 'string') return;
          const option = document.createElement('option'); option.value = String(index);
          option.textContent = item.label.slice(0,80) + (item.automatic ? ' · авто' : ''); captions.append(option);
        });
        captions.hidden = captions.options.length < 2;
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
      const savedPosition = typeof config.position === 'number' && Number.isFinite(config.position) ? config.position : 0;
      const restore = () => {
        if (disposed || restored || video.readyState < 1 || (separate && audio.readyState < 1)) return;
        restored = true; restoreClear();
        const position = savedPosition > 0 && savedPosition < video.duration - 5 ? savedPosition : 0;
        video.currentTime = position; if (separate) audio.currentTime = position;
        updateView(); tick(true);
      };
      restoreMetadata = restore;
      video.addEventListener('loadedmetadata', restore);
      audio.addEventListener('loadedmetadata', restore);
      video.muted = separate; video.src = config.video;
      if (separate) audio.src = config.audio;
      downloadVideo.disabled = false; downloadAudio.disabled = !separate;
      button.disabled = false; setStatus(config.resume_available === false ? 'Готово к воспроизведению · сохранение позиции недоступно' : 'Готово к воспроизведению'); return true;
    },
    setDownload(data) {
      if (disposed || !configured || !data) return;
      const busy = Boolean(data.choosing || data.progress);
      downloadVideo.disabled = busy; downloadAudio.disabled = busy || !separate; downloadCancel.hidden = !busy;
      if (data.choosing) downloadStatus.textContent = 'Выберите файл…';
      else if (data.error) { downloadStatus.textContent = String(data.error).slice(0,160); }
      else if (data.cancelled) downloadStatus.textContent = 'Загрузка отменена';
      else if (data.done) downloadStatus.textContent = 'Файл сохранён';
      else if (data.progress) {
        const p = data.progress;
        const names = {downloading:'Загрузка',muxing:'Объединение',verifying:'Проверка',publishing:'Сохранение'};
        const total = Number.isFinite(p.total_bytes) && p.total_bytes > 0 ? p.total_bytes : 0;
        const bytes = Number.isFinite(p.bytes) && p.bytes >= 0 ? p.bytes : 0;
        downloadStatus.textContent = (names[p.phase] || 'Загрузка') + (total ? ' · ' + Math.min(100, Math.floor(bytes / total * 100)) + '%' : ' · ' + Math.floor(bytes / 1048576) + ' МБ');
      }
    },
    setCaptions(data) {
      if (disposed || !configured || !data) return;
      captionsClear();
      captionStatus.textContent = data.loading ? 'Загрузка субтитров…' : typeof data.error === 'string' ? 'Субтитры недоступны' : '';
      if (data.error || data.loading || captions.value === '-1' || !Array.isArray(data.cues) || data.cues.length > 10000) return;
      if (!data.cues.every(c => c && Number.isFinite(c.start) && Number.isFinite(c.end) && c.start >= 0 && c.end > c.start && c.end <= 604800 && typeof c.text === 'string' && c.text.length <= 8192)) return;
      textTrack ||= video.addTextTrack('subtitles', 'Субтитры');
      for (const cue of data.cues) textTrack.addCue(new VTTCue(cue.start, cue.end, cue.text.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')));
      textTrack.mode = 'showing';
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
      return {configured, disposed, hidden: document.hidden, video_paused: video.paused, audio_paused: audio.paused, playing: intended && !video.paused && (!separate || !audio.paused),
        position: (separate ? audio.currentTime : video.currentTime) || 0, audio_position: separate ? audio.currentTime || 0 : video.currentTime || 0, duration, separate,
        video_width: video.videoWidth || 0, video_height: video.videoHeight || 0,
        skew: separate ? Math.abs(audio.currentTime - video.currentTime) : 0,
        decoded: video.getVideoPlaybackQuality?.().totalVideoFrames || 0,
        dropped: video.getVideoPlaybackQuality?.().droppedVideoFrames || 0};
    }
  }), writable: false, configurable: false});
})();
