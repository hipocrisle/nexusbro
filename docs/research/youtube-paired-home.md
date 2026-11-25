# Paired ABBA Home — обычный watch YouTube vs shipped light (подготовка, без запуска)

Worktree /matrix/nexusbro-kimi-home-paired, ветка research/youtube-home-paired (base 5f6879b).
Границы: ровно 3 файла — `tests/gui/youtube_paired_native.py` (минимальный DI),
НОВЫЙ `tests/gui/youtube_paired_home.py`, НОВЫЙ `docs/research/youtube-paired-home.md`.
Статус: подготовлено, offline self-test PASS, НИЧЕГО не запускалось (ни SSH/GUI/CI/сети).
ROOT: review → передача/запуск. Не трогались: local embed/light helpers, desktop,
workflows, memory, guides.

## Что сделано

1. **DI в arm()** (keyword-only, default = прежнее поведение байт-в-байт):
   `device_factory`, `close_owned`, `private_parent`, `window_manager_factory`,
   `state_probe`. GHA-run ничем не ослаблен: `run()` по-прежнему требует
   `GITHUB_ACTIONS=true`; общий `probe()` остаётся default для GHA; ловля
   `NativeFailure` добавлена только в bounded-цикл light-admission (для
   injected probe; для shared probe поведение идентично, т.к. он бросает
   Blocked).
2. **Локальный entry** `youtube_paired_home.py`: ORDER=light,normal,normal,light
   через тот же arm(); guards из общего local-модуля (uid==euid==expect-uid,
   ~/AI 0700, exe-sha256, resource_dir `lib/NexusBro/binaries/youtube-light` с
   pins yt-dlp/deno/ffmpeg через sealed hash, CEF в root); отказ при
   GHA/наследованном sandbox-disable; SandboxedDevice + LocalWindowManager,
   свой Xvfb, чужой DISPLAY/GPU не используется; local_close (OS-normal exit,
   GHA-receipt не заявляется); стоп на первой упавшей руке, без ретраев.
3. **state_probe (local)**: НЕ использует shared probe() (тот чистит __paired
   каждый вызов и теряет async youtube-result, а NativeFailure вне Blocked) —
   читает только НОВЫЕ события по монотонному индексу под точный TIP/tab
   (light/normal/decode + youtube-result), буфер не очищает (хвост 512);
   `youtube-result start=false+error=null` = успешное завершение, НЕ фейл;
   ошибка мапится только через фиксированный sidecar_enum (русские строки
   наружу не попадают); bounded 60s-вход в arm() пережидает асинхронную
   экстракцию.
4. **Отчёт**: всегда `diagnostic_only=true`, `acceptance_passed=false`,
   `full_playback_acceptance=false`, `paired_release_acceptance=false`;
   `measurement_valid`/`comparison_valid` могут быть true только по полным
   фактам; abort при первой blocked-руке.

## Видео-контракт

M7lc1UVf-VE: duration 1343.6, 1280×720, fps 30 (аргументы по умолчанию).
Если фактическое обычное (normal) качество адаптивно отличается — честный
`normal_or_light_quality_mismatch`, без JS-подгонки и сравнения несопоставимого.

## Честные ограничения

- Self-test оффлайн: DI-сигнатуры/дефолты, геометрия state_probe, флаги
  отчёта, ORDER. Не исполнялось: arm() целиком, physical-ключи нормального
  режима, sampler-метрики, cleanup per-arm.
- Известный риск: normal-режим на реальном YouTube может показать рекламу/
  consent — arm() имеет guard'ы на это (blocked, не клик); local entry их не
  ослабляет.
- Артефакты samples пишутся в per-arm dirs внутри ~/AI/out; отчёт —
  `paired-result-*.json` рядом.

## Следующий шаг (ROOT)

Review трёх файлов → запуск на Home:
`--private-root ~/AI/... --exe root/bin/nexusbro --cef root/cef
--components root/lib/NexusBro/binaries/youtube-light --expect-uid 1001
--exe-sha256 <sealed> --allow-local-virtual-display`.
Бюджеты: ~4×210 с + cleanup; ROOT-овский ресурсный 2h Windows (45637553346)
не трогать.
