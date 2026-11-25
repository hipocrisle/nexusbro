# Paired ABBA Home — обычный watch YouTube vs shipped light (подготовка, без запуска)

Worktree /matrix/nexusbro-kimi-home-paired, ветка research/youtube-home-paired (base 5f6879b).

## СТАТУС: PREPARED BUT BLOCKED — нативный gate (не запускать)

ROOT-review выявил блокер, пропущенный при подготовке: нативный gate приложения
`desktop/src-tauri/src/cef_youtube_benchmark.rs:65` (`gate()` требует
`GITHUB_ACTIONS=true` + `NEXUSBRO_TEST_DATA`) — локальные normal/decode
snapshot-запросы текущего запечатанного бинарника (source 1f8ece5) отвечают
`benchmark_fixture_only`. Локальный paired-запуск НЕВОЗМОЖЕН без новой сборки
продукта или подделки GHA — оба пути запрещены. Entry поэтому отказывает в
preflight с фиксированным кодом `local_native_benchmark_fixture_only`, если в
private root нет явного declaration-файла `native-capability/paired-local-
benchmark`; такой capability сегодня в продукте НЕ существует (легитимный
путь на будущее: новая сборка с явной нативной поддержкой local benchmark +
повторная 2h-ресурсная приёмка — здесь НЕ реализовано и НЕ заявлено).
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
   `comparison_valid` всегда `false` (полные общие cross-arm инварианты
   декодера/аудио/позиций/fps здесь НЕ переверяются; len(arms)==4 — не
   критерий валидности); `measurement_valid` — только если все 4 руки
   `complete` с метриками; abort при первой blocked-руке; watchdog-/arm
   (deadline реально enforced, cleanup в finally самой arm, фиктивных
   done-маркеров и безусловного cleanup_success нет).

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
