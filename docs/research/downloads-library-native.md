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
