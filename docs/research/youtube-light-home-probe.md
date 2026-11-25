# YouTube light mode — Home local probe (подготовка entry, без запуска)

Ветка: research/youtube-embed-home (worktree /matrix/nexusbro-kimi-home, base 02fee9a).
Файл: `tests/gui/youtube_light_home_probe.py` (+ эта документация). Статус: **entry
подготовлен, offline self-test PASS, ничего не запускалось** (ни SSH/GUI/CI/YouTube-
сети). ROOT владеет packaging/памятью/запуском; его правки локального embed-entry
(uncommitted) не тронуты, только импортируются.

## Назначение

Дополняет official-embed Home-пробу: проверяет штатный **лёгкий плеер продукта**
(youtube_light: pinned yt-dlp + Deno из exe-adjacent `binaries/youtube-light`,
фиксированный арг приложения `--nb-youtube-components`, cef_dvizhok.rs:289) на
свежем локальном профиле с обычным `watch?v=M7lc1UVf-VE`. Сначала ОДНА shipped-команда входа в лёгкий режим
(`call(..., komanda={'deystvie':'youtube_light','id':tab})`), затем bounded-
ожидание (≤60 с, паттерн `paired arm()`): экстракция асинхронна, первые
снапшоты могут отсутствовать — мгновенный фейл запрещён. Слушатель собирает
`youtube-state` И `youtube-result`; `start:false` с безопасным кодом
(`^[a-z0-9_]{1,48}$`) классифицируется как `native_light_<code>` со стадией
`extraction`, иначе по дедлайну — `real_extraction_unavailable_or_challenged`.
Снапшот-цикл через shipped `youtube_snapshot` (`probe(...,'light',...)`),
ОДИН физический play через `physical(client, device, 'play', ...)`.

## Жёсткие границы (унаследованы через импорт ROOT-хелперов)

- Guards: `uid==euid==--expect-uid` (не root); `--private-root` строго в `~/AI`
  (owner, 0700, strict resolve); `--exe`/`--cef`/`--components` строго в root;
  точный `--exe-sha256`; PATH-preflight (fixed codes); `--allow-local-virtual-display`
  обязателен. Никакого GHA (`local_mode_forbids_gha`), никакого
  `WEBKIT_DISABLE_SANDBOX` — в т.ч. унаследованной из окружения
  (`local_mode_forbids_inherited_sandbox_disable`, как в ROOT-local entry),
  чужой DISPLAY/GPU не используется. Device/WM живут в per-run `out/device`,
  `out/wm` (root уже содержит прошлые embed-директории).
- `--components` принимается ТОЛЬКО как Tauri resource_dir Linux:
  ровно `<exe.parent.parent>/lib/nexusbro/binaries/youtube-light`
  (tauri-utils platform.rs: exe_dir/../lib/<package> при существовании; приложение
  резолвит его через papka_resursov, cef_dvizhok.rs — НЕ exe-adjacent). Файлы:
  `yt-dlp`, `deno`, `ffmpeg` — реальные, несимлинковые, непустые. Запечатанный
  хэш компонентов авторитетен (pins не дублируем).
- Данные: из нативного снапшота наружу — только скаляры; safe failure-код —
  только `^[a-z0-9_]{1,48}$` из shipped-поля `code` формы `youtube-state`
  (cef_vkladki.rs:1248), иначе `unavailable`. Никаких raw URL/tokens/output,
  ни одного owner profile/session/cookie импорта.
- Успех пробности: position delta ≥8 с + `playing` + audio policy
  (`volume==1, muted=false, default_sink=true`) + счётчик decoded-кадров растёт
  (не image-only). Без аудио-захвата, без заявлений про рекламу/эффективность.
- `full_playback_acceptance=false`, `paired=false` — всегда. Выход: общий
  `local_close` (ExitWitness + identity, GHA-receipt не заявляется).

## Честные ограничения

- Self-test оффлайн: валидация состояния/кодов/флагов, defensive-импорт
  `LocalWindowManager` (fallback на `OwnedWindowManager` в старой версии entry).
- Не проверено вживую: реальный снапшот shipped-плеера, physical play на жёлтой
  кнопке light-плеера, нормальный exit — всё это только после review и запуска ROOT.
- Если экстракция не завершится за bounded-окно, прогон честно упадёт в
  `real_extraction_unavailable_or_challenged` с доступным safe-кодом — не false
  PASS. Вход в лёгкий режим делает shipped-команда, а не навигация.

## Следующий шаг (ROOT)

Review границ → упаковка компонентов в `<exe>/binaries/youtube-light` → локальный
запуск:
`--private-root ~/AI/... --exe ... --cef ... --expect-uid 1001 --exe-sha256 <sha>
--components <exe>/binaries/youtube-light --allow-local-virtual-display`
→ результат в `~/AI/out/light-result-*.json`.
