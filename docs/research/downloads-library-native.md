# Downloads library — native тест и open-policy (подготовка)

Ветка fix/downloads-status-actions (worktree /matrix/nexusbro-kimi-downloads).
Файлы этого шага: НОВЫЙ `desktop/src-tauri/src/downloads_open_policy.rs`,
точечный `zagruzki.rs`, НОВЫЕ `tests/downloads_library_contract_test.py`,
`tests/gui/downloads_library_native.py`, обновлённый `tests/downloads_ui_checks.py`.
Статус: **подготовка; нативный тест НЕ запускался** (CI/GUI запрещены). Запуск — ROOT.

## Open/folder policy (production, реально протестирована)

`downloads_open_policy.rs` — чистая проверка пути из сохранённой записи:
- статус строго «Готово»; путь absolute; `canonicalize` + обычный не-symlink файл;
- trailing dot/space в имени и NTFS ADS (`:`) отклоняются (обход ассоциаций Windows);
- расширенный blocklist Windows-опасных типов (.url/.hta/.scf/.msc/.cpl/.reg/.pif/
  .gadget/.application/.msp/.mst/.psm1/.psd1/.py/.pl/.rb/.vb/.sct/.inf/.ins/.isp + прежние);
- Linux: execute-бит файла блокирует открытие независимо от расширения;
- folder: только существующая абсолютная папка, имя не-опция (не начинается с `-`);
- безопасные документы/изображения/архивы по явному клику открываются (тест safe_document_opens);
- ошибки нейтральные, без сырых путей; никакого shell/аргументов от frontend; auto-open нет.

Проверки: `rustc --test` компилирует РЕАЛЬНЫЙ production-файл и гоняет 9 юнит-тестов
(9/9 PASS, см. `tests/downloads_library_contract_test.py` — PASS, включая wiring:
zagruzki.rs использует `crate::downloads_open_policy::{check_open_target,check_folder_target}`,
inline-списков не осталось).

**Wiring для ROOT (его файл):** в `desktop/src-tauri/src/lib.rs` добавить
`mod downloads_open_policy;` — без этого crate не соберётся.

## Нативный тест (PREPARATION, GHA-only)

`tests/gui/downloads_library_native.py` по паттернам security_native/essentials:
- собственный loopback HTTP fixture: медленные chunked attachment'ы — known-length
  (реальный percent) и unknown-length (indeterminate-состояние), плюс безопасный note.txt;
- изоляция: `NEXUSBRO_TEST_DATA` (штатно) + `NEXUSBRO_TEST_DOWNLOAD_DIR` в fixture-папку
  (механизм уже есть в zagruzki.rs:25), без диалога сохранения;
- реальные скачивания обычной навигацией по attachment-ссылкам (никакого synthetic state);
- проверки: реальный прогресс known/unknown, рост received-байтов, завершение с
  реальными файлами на диске, библиотека открывается НАСТОЯЩИМ Ctrl+J, кнопки
  «Открыть файл»/«Показать в папке» есть только у завершённых, удаление записи
  НЕ удаляет файл;
- open-file: test-local `xdg-open`-shim первым в PATH дочернего процесса (записывает
  аргумент, ничего не запускает) — production не содержит test-bypass; в GHA shim
  получает путь безопасного .txt.

## Незакрытые runtime-критерии (передать ROOT)

1. Реальные OS-ассоциации файлов (открытие настоящим приложением) на GHA не доказуемы —
   только shim-факт передачи правильного пути.
2. Windows-путь explorer.exe — не покрыт (Linux-only тест).
3. Wiring `mod downloads_open_policy;` в lib.rs — файл ROOT'а.

Границы: workflows/desktop UI не менялись (кроме своего zagruzki.rs); запусков/CI/сети
кроме loopback-fixture в самом тесте нет; отчёт честно называет подготовку подготовкой.


## Прогон 45787393628: разбор и исправления фактов (26.11)

Факт из result.json: stage=progress-observation, blocked_reason=downloads_native_unavailable,
checks=[]. Разбор кода выявил ДВА дефекта самого скрипта (не app):

1. **Потеря причины**: wait_for/require бросают NativeFailure (AssertionError), а не Blocked —
   таймаут первого ожидания попадал в generic-ветку `downloads_native_unavailable`, сообщение
   терялось. Исправлено: allowlist фиксированных сообщений → typed-коды
   (known_progress_timeout / unknown_total_timeout / received_stall_timeout /
   *_complete_timeout / library_open_timeout), прочее — `native_assertion` с
   failure_type из allowlist; сырые строки не пишутся.
2. **Гонка навигаций**: три pereyti подряд с паузой 0.3с — новая навигация могла отменить
   предыдущую ДО on_before_download, и записи загрузки могло вовсе не появиться.
   Исправлено сериализацией: note.txt → подтверждение «Готово»+файл → known → наблюдение
   активного прогресса → «Готово» → unknown → наблюдение. Вложения остаются РЕАЛЬНЫМИ
   CEF attachment, требование активного прогресса не ослаблено.

Плюс санитизированные снимки: `downloads_before_wait` (до первого ожидания) и
`downloads_at_failure` (в отказе) — только file_kind (known/unknown/doc/other),
status из allowlist, ограниченные скаляры received/total/percent/speed; никаких
сырых url/path/id/name. preparation=false при реальном прогоне; open/folder shim
явно не OS-association proof.

Проверки: реальный localhost-прогон собственной Fixture без браузера (Content-Length
786432, Content-Disposition attachment, точные байты known/unknown/doc) + FakeClient
съём sanitize-снимка и typed-кодов — все PASS (tests/downloads_library_contract_test.py).
Какая именно ветка сработала в 45787393628 (таймаут known vs отсутствие записи) станет
видно по typed-коду следующего прогона; app-bug не угадывается.


## Trusted page click (26.11, после 45791564388: doc_complete_timeout/0 записей)

Прежний прогон: pereyti на attachment не создал запись загрузки вообще (count=0).
Гипотеза user-gesture/route НЕ объявляется доказанной причиной; следующий прогон
проверяет РЕАЛЬНЫЙ пользовательский flow:

- Плитки fixture: три раздельные области фиксированных координат с уникальным
  RGB (#c02020/#20c020/#2020c0) на фоне #123456; центры (140,80)/(140,200)/(140,320).
- Проверяемая gesture: ОДИН настоящий OS-клик (xdotool) по плитке в owned CEF-
  регионе: реальный OS-paint пикселя перед кликом (точное попадание цвета),
  owner PID/birth/окно перепроверяются, foreground возвращается. Никаких
  JS anchor.click/CEF-скриптов/выдуманных rect/native callbacks. Клик не
  повторяется; API-retry постфактум нет.
- Доказательство ветки: threadsafe-счётчики fixture (page/doc/known/unknown GET)
  обязаны вырасти после клика (http_branch_reached); typed tab facts до/после
  (browser_positive/route_pending/route_error_present/zagruzka/url_kind только
  по 4 известным URL — без raw url/path).
- Tab-live ждёт browser_id>0 И !zagruzka И !route_pending + факт page GET.
- Setup-навигация страницы — штатный seed (IPC допустим для setup); проверяемая
  download-gesture — только trusted. Attachment не заменяет документ — плитки
  остаются кликабельными для следующих загрузок (сериализация сохранена).
- Отчёт: observed_at_utc через datetime.now(timezone.utc); kind/status/bytes
  только safe-поля; openShim по note.txt остаётся честной delegation-проверкой.

Offline-проверки (без GUI, не string-assert): реальный localhost-прогон —
счётчики растут по каждому GET, плитки в HTML; чистая математика точек
(scale/rounding), color-match позитив/негатив, url_kind mapping, tab_facts
typed; отсутствие pereyti-загрузок в исходнике; wiring trusted_download ×3;
требование http_branch_reached. Все PASS (+ policy 10/10).
