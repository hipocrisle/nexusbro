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
свежем локальном профиле с обычным `watch?v=M7lc1UVf-VE`. Один снапшот-командный
цикл через shipped `youtube_snapshot` (`probe(...,'light',...)`), ОДИН физический
play через существующий `physical(client, device, 'play', ...)` (доказательство
отрисовки контролов + один клик внутри).

## Жёсткие границы (унаследованы через импорт ROOT-хелперов)

- Guards: `uid==euid==--expect-uid` (не root); `--private-root` строго в `~/AI`
  (owner, 0700, strict resolve); `--exe`/`--cef`/`--components` строго в root;
  точный `--exe-sha256`; PATH-preflight (fixed codes); `--allow-local-virtual-display`
  обязателен. Никакого GHA (`local_mode_forbids_gha`), никакого
  `WEBKIT_DISABLE_SANDBOX` (SandboxedDevice), чужой DISPLAY/GPU не используется.
- `--components` принимается ТОЛЬКО как ровно `<exe_dir>/binaries/youtube-light`
  (тот путь, который само приложение передаёт в `--nb-youtube-components`), с
  реальными несимлинковыми непустыми файлами `yt-dlp` и `deno`. Запечатанный
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
- Если shipped light-плеер на watch-URL не открывается автоматически, прогон
  честно упадёт в `native_light_unavailable` — это известная непокрытая часть,
  не false PASS.

## Следующий шаг (ROOT)

Review границ → упаковка компонентов в `<exe>/binaries/youtube-light` → локальный
запуск:
`--private-root ~/AI/... --exe ... --cef ... --expect-uid 1001 --exe-sha256 <sha>
--components <exe>/binaries/youtube-light --allow-local-virtual-display`
→ результат в `~/AI/out/light-result-*.json`.
