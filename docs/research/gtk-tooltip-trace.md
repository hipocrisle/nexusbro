# GTK tooltip trace — read-only LD_PRELOAD observer (подготовка, DIAGNOSTIC ONLY)

Worktree /matrix/nexusbro-kimi-gtk-lifecycle, ветка research/gtk-tooltip-lifecycle (base 1df902e — fixes ROOT'а
(typed void setter / marker recheck / guint / thread-local guard / atomic cap / observed bits) СОХРАНЕНЫ).
Файлы: НОВЫЙ `tests/gui/gtk_tooltip_trace.c` + эта документация. Компиляция
проверена (`cc -std=c11 -shared -fPIC -O2 -Wall -Wextra -Werror ... -ldl -pthread`,
все три символа экспортированы). **Ничего не исполнялось** — ни приложение, ни
тесты, ни CI. Запуск/ревью/интерпретация — за ROOT. Это НЕ фикс и НЕ release
acceptance; фиксированные нативные факты justify ОДИН будущий trace-прогон.

## Что делает observer

- LD_PRELOAD pass-through для НЕИЗМЕНЁННОГО CI-бинарника. Перехватывает ровно:
  `gtk_widget_set_has_tooltip`, `gtk_widget_trigger_tooltip_query`,
  `gtk_tooltip_set_text`. Каждый вызов форвардится с точным C ABI и аргументами.
  НИКОГДА не пишет GTK-состояние: ни настроек, title, окон, shape, raise, кликов,
  принудительных query.
- Лог включён только при одновременном: `GITHUB_ACTIONS=true` +
  `NEXUSBRO_TEST_DATA` задан + `NEXUSBRO_TEST_TOOLTIP_TRACE_ARM` указывает на
  СУЩЕСТВУЮЩИЙ файл (ROOT создаёт маркер непосредственно перед ОДНИМ hover;
  содержимое не читается — existence = arm-сигнал).
- Максимум 64 строк на процесс (включая записи сигналов), stderr, фиксированный формат
  (новые поля ТОЛЬКО в конце — прежние префиксы не ломаются):
  `NB_TOOLTIP_TRACE event=has|trigger|text|realize|map|unmap observed=0|1 requested=0|1
  has=0|1 enabled=0|1 timeout_ms=<0..120000> browse_ms=<0..120000> text_present=0|1
  mapped=0|1 visible=0|1 hooks=0|1 tick_ms=<0..600000>`.
  tick_ms — относительные миллисекунды от первого resolve. hooks=1 только если emission-hooks
  реально зарегистрированы; hooks=0 — ЯВНЫЙ бит невозможности, НИКОГДА не доказательство
  «map не было». Без путей, PID, title'ов, текста tooltip'а, URL, raw dlerror.
- Lifecycle-observer: emission hooks на GtkWidget realize/map/unmap (опциональные GLib-символы;
  регистрация при первом wrapper-вызове, когда GTK-классы уже инициализированы; отдельный
  pthread_once, thread-local guard против re-entry/deadlock). Callback — точный GLib ABI
  (gboolean hint/guint/values/data), ничего не потребляет и не переписывает, возвращает TRUE,
  не удерживает object-refs. Фильтр: g_type_check_instance_is_a(GtkWindow) +
  gtk_window_get_type_hint()==GDK_WINDOW_TYPE_HINT_TOOLTIP(10) — логируются ТОЛЬКО tooltip-окна.
  Никакого synthetic tooltip/show/hide/resize/query-manipulation. Отсутствие optional-символов
  (например, stub unit-теста) -> hooks=0, форвард всегда выполняется, краш невозможен.
- Фильтр виджетов: только типы с префиксом `WebKit` (g_type_name_from_instance
  через dlsym RTLD_DEFAULT); прочие вызовы форвардятся без лога. Чтение has-tooltip
  происходит ДО форварда setter'а (безопасное время жизни указателя).
- Разрешение символов через pthread_once + RTLD_NEXT; жёсткий сбой разрешения →
  фиксированный `_exit(112)` без текста ошибки. Отсутствие optional read-символов
  → поля observed=0, форвард всегда выполняется; краш диагностики невозможен.
  Recursion-guard, потокобезопасный атомарный счётчик.

## Как ROOT использует (предложение, не реализация)

CI-шаг собирает .so, ставит LD_PRELOAD для ОДНОГО прогона linux-runtime с
созданием arm-маркера перед hover и снятием после; результат — grep stderr по
префиксу в JSON-поле `tooltip_trace` (prohibits publisher по аналогии с другими
диагностиками). Интерпретация согласно source-audit:
`event=has requested=1 has=0 enabled=0` → подавление настройками/пустым текстом;
`has=1 enabled=1 timeout_ms>0`, но окон нет → GTK-side показ не состоялся
(вне observer'а); `text_present=0` при живом DOM-title → пустой tooltipText на
входе GTK.

## Границы

Нет новых app-флагов/сборок/CI/env/GTK-конфигов; бинарник 1f не пересобран;
ROOT-овские workflow/security_native.py/tab_routes.py не тронуты; ресурсный 2h
(45637553346) не затронут. Paired-ветка остаётся BLOCKED. Source-observer —
наблюдатель, не исправление; реальные факты оправдывают максимум один будущий
trace-прогон.

ROOT review: correct void setter ABI, recheck marker on every call (no startup
false latch), exact unsigned GTK timeout property storage, thread-local recursion
guard and atomic64-record cap. Optional getter readiness is explicit `observed`;
unknown getters are not interpreted as disabled GTK. A stub-native-ABI contract
test verifies exact forwarding, late arming/disarming, bounded output and no
private text export. It never starts GTK or a browser and is not acceptance.

ROOT integration: structured JSON `probe=tooltip_trace`, exact owned CI .so
path only, actual native shell library mapping verified with held OS identity.
The ordinary CI mode is unchanged; any observed-with-interposer route result
is diagnostic_only, and structured requests are forbidden to the publisher.


## Статус lifecycle-расширения

Нативный trace 45646243444 показал: текст есть (text_present=1), enabled=1, timeout=500,
browse=60 — а Xlib-наблюдение видит 0 mapped за 50 поллов. Расширение отвечает ровно на один
вопрос: very-short map/unmap vs never-map. Интерпретация: строки realize/map/unmap с
tick_ms между has/trigger → окно жило короче 80мс-окна поллов; их полное отсутствие при
hooks=1 → GTK не создавал окно вообще (тогда дальше — GTK-internal, вне observer'а); hooks=0 →
наблюдение невозможно, вывод не делается. COMPILED + ROOT-unit-stub PASS; НЕ исполнялось
ни с реальным GTK, ни с приложением. Это наблюдатель, не фикс; source-observer не является
исправлением. Бинарник 1f не пересобран; 2h ресурсный (45637553346) не затронут.
