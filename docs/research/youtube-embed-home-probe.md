# YouTube official embed — Home local probe (подготовка entry, без запуска)

Ветка: research/youtube-embed-home (worktree /matrix/nexusbro-kimi-home, base 02fee9a).
Файлы: только два новых — `tests/gui/youtube_embed_home_probe.py` и эта документация.
Статус: **entry подготовлен, offline self-test PASS, ничего не запускалось**
(ни SSH, ни GUI, ни CI, ни сетевых видео-проб). Запуск и review — за ROOT.

## Контекст

GHA official-embed run 45630127021 (M7lc1UVf-VE): API ready, OS paint, один trusted
click → error 150; PNG 640×360 показал «Sign in to confirm you're not a bot» — значит
GHA-внешний challenge есть и у official embedded player. Больше GHA-проб не нужно.
Home Linux (обычный claude, uid 1001, glibc 2.39, /home/claude/AI, 56 ГБ free):
GTK3/WebKit4.1/JSCore/NSS/X11/Xfont2 уже есть; Xvfb/openbox/wmctrl/xdotool отсутствуют
(их ставит ROOT отдельно, подписанные apt-пакеты). Реальный десктоп/GPU владельца
использовать нельзя.

## Что делает entry

Локальный consumer того же official-фикстура (HTTP-store границы данных, allowlisted
codes, один физический click, жёлтый пиксель OS paint, API-ready → playing с
delta+wall ≥8с), но:

1. **Без GHA-подделки**: общий `run()` CI-скрипта и его guards не менялись; это
   отдельный модуль со своим `run()`. `GITHUB_ACTIONS=true` локально запрещён
   (`local_mode_forbids_gha`).
2. **Жёсткие guards до старта**: `uid == euid == --expect-uid`, не root;
   `--private-root` строго внутри `~/AI`, владелец = ожидаемый uid, режим 0700,
   strict resolve (эскейп через symlink невозможен); `--exe`/`--cef` строго внутри
   root; точный `--exe-sha256`; PATH-preflight тех же программ (fixed codes).
3. **Sandbox сохранён**: shared `Device.start` принудительно ставит
   `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` (GHA- accommodation). Локально это
   недопустимо: `SandboxedDevice.start()` — точная копия того же старта БЕЗ этого
   env. Если приложение не стартует под штатным sandbox → честный
   `sandbox_startup_blocked`, повтора с отключённым sandbox нет и не будет.
   HOME/системные env/службы/пакеты/sudo не трогаются.
4. **Обычный exit без receipt**: GHA shutdown-receipt локально недоступен
   (`receipt_unavailable_in_local_mode=true`, `clean_shutdown_receipt_verified`
   не выставляется никогда). Нормальный выход доказывается реальными проверками
   `native_app_identity`/`ExitWitness` (процессы/рождения), OS-normal close через WM.
   `full_playback_acceptance=false`, `paired=false` — всегда.
5. Только свой Xvfb (displayfd) + свой openbox, `--allow-local-virtual-display`
   обязателен; GPU/`apparatnoe_uskorenie=false` → CEF disable-gpu; чужой DISPLAY
   не используется. Owner profile/session/cookies не импортируются.

## Честные ограничения

- Self-test оффлайн: sandbox-env отсутствует в `SandboxedDevice.start`, local_close
  не вызывает `read_shutdown_receipt`, состав guards, флаги отчёта.
- Не проверено вживую: старт под настоящим sandbox на Home (может потребовать
  user namespaces — если нет, прогон честно упадёт в `sandbox_startup_blocked`),
  реальный yellow-paint/click/receipt, нормальный exit.
- ROOT решает: установка недостающих Xvfb/openbox/wmctrl/xdotool (подписанные
  пакеты), запуск `--private-root ~/AI/... --exe ... --exe-sha256 ...` после review.

## Следующий шаг

ROOT: review границ → подготовка недостающих пакетов/файлов → локальный запуск
(сетевой выход только к официальному youtube.com для iframe) → результат в
`~/AI/out/result-*.json`.
