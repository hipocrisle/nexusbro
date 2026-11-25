# GTK tooltip trace — read-only LD_PRELOAD observer (подготовка, DIAGNOSTIC ONLY)

Worktree /matrix/nexusbro-kimi-gtk-trace, ветка research/gtk-tooltip-trace (base c6ef988).
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
- Максимум 64 строки на процесс, stderr, фиксированный формат:
  `NB_TOOLTIP_TRACE event=has|trigger|text requested=0|1 has=0|1 enabled=0|1
  timeout_ms=<0..120000> browse_ms=<0..120000> text_present=0|1`.
  Без путей, PID, title'ов, текста tooltip'а, URL, raw dlerror.
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
