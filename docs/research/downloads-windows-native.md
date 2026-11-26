# Downloads — Windows native проверка open/folder (подготовка)

Ветка fix/downloads-status-actions (worktree /matrix/nexusbro-kimi-downloads).
Файлы этого шага: НОВЫЙ `tests/gui/downloads_windows_native.py` + эта документация.
Статус: **подготовка; нативное выполнение НЕ производилось** (CI/GUI запрещены).
Запуск/интеграция — ROOT.

## Что проверяет тест (GHA Windows only)

- Реальное CEF-скачивание безопасного `note.txt` с собственного loopback-origin
  (изоляция `NEXUSBRO_TEST_DATA` + `NEXUSBRO_TEST_DOWNLOAD_DIR`, механизм уже в
  zagruzki.rs); байты сверяются точно.
- Библиотека открывается настоящим Ctrl+J (pyautogui после foreground) с
  проверкой реального фокуса панели; клик по «Открыть файл» — физический
  (pyautogui по координатам кнопки точной статьи note.txt), не JS.
- Доказательство открытия: НОВЫЙ процесс (снимок процессов до клика), рождение
  после клика, известный исполняемый файл (`notepad.exe`) и ТОЧНЫЙ приватный
  путь файла в его командной строке. Чужие Notepad/Explorer не трогаются;
  cleanup завершает только точно идентифицированный own-born процесс.
- Папка: попытка UIA-доказательства адреса own-born Explorer с нашей папкой;
  при недоступности UIA — честный `folder_proof: unclosed`, без подделки.
- При отсутствии доказательств тест падает/blocked — никакого фиктивного SUCCESS.
  Ассоциации/реестр/env-файлы не меняются (ассоциация .txt — штатная GHA).

## Read-only анализ: canonicalize → `\\?\` путь в explorer.exe

Первоисточники: Microsoft Learn «Naming Files, Paths, and Namespaces»
(https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file):
префикс `\\?\` отключает нормализацию Win32 и предназначен для файловых API;
это НЕ «путь для shell».

Факты нашего кода: `check_open_target` возвращает `std::fs::canonicalize`
(desktop/src-tauri/src/downloads_open_policy.rs) — на Windows это всегда
verbatim-путь вида `\\?\C:\...`. `zagruzka_deystvie` передаёт его как есть в
`explorer.exe` (open) и родителя в `explorer.exe` (folder).

Риск (конкретный, без преувеличения): shell/Explorer не документирован как
принимающий verbatim-пути; передача `\\?\C:\...` аргументом explorer.exe
исторически ненадёжна (игнор/открытие «Компьютера» вместо цели). Вердикт:
**реальный риск молчаливого не-срабатывания open/folder на Windows**;
документацией прямой запрет не зафиксирован, поэтому это риск, а не доказанный
дефект — окончательно решит нативный прогон (UIA/title доказательство в тесте).

Рекомендация (зона ROOT — zagruzki.rs/policy): перед spawn снимать префикс
`\\?\` (например, `dunce::canonicalize` или ручной strip `\\?\`), сохраняя
канонические проверки на канонической форме. Проверки безопасности это не
ослабляет: strip — чистая нормализация представления.

## Коррекция прежнего замечания (по ROOT)

Мой прежний «блокер» по правам промежуточных каталогов был НЕ подтверждён:
`0o755 & 0o022 == 0` — 0755 не имеет group/world WRITE бит; допустимы
промежуточные 0755, требование — private data 0700. В статус этот блокер не
переносится. Gate допускает owned AI 0775 (group), запрещает world-write AI и
group-write внутри private fixture — зафиксировано как факт от ROOT.

## Исправления по ROOT review (33805bb)

- own_born_editor: ТОЧНОЕ совпадение отдельного argv-элемента после нормализации
  (кавычки/verbatim-префикс/слэши/регистр); note.txt.extra и substring никогда не
  засчитываются; exe только из известных каталогов Notepad (System32/Windows/
  WindowsApps Microsoft.WindowsNotepad_*), не «любая программа с именем notepad».
- explorer_folder_proof: PID окна обязан входить в fresh-explorer множество
  (снимок до/после), UIA-адрес сравнивается как точный нормализованный путь;
  basename в заголовке — не доказательство; нет UIA → честный unclosed.
- Отчёт: раздельные open_completed/folder_completed; full_acceptance=false, если
  folder unclosed.
- Перед кликом по папке: own-born редактор закрывается (terminate+wait), возврат
  foreground(device) и проверка присутствия панели — клик не попадёт в редактор.
- Cleanup: terminate+wait с проверкой is_running; cleanup не True, пока процесс жив.
- Строгий focus-guard: булево matches(":focus-within") без truthy-undefined.
- Offline-контракт: tests/downloads_windows_contract_test.py — 17 негативных/позитивных
  проверок (exact path vs .extra, foreign PID, reused-PID birth, unknown notepad
  location, foreign explorer, missing UIA) — PASS без GUI.

## Доказательство папки — NEW WINDOW (правка по pragmatic blocker ROOT)

Explorer обычно открывает НОВОЕ окно в УЖЕ существующем процессе — критерий
«свежий PID» всегда отрицательный. Доказательство переработано:
- снимок handles ДО клика (COM Shell.Application.Windows(), при недоступности
  COM — UIA handles; доступность не выдумывается: COM=None → только UIA-путь);
- после физического клика ищется НОВЫЙ hwnd (отсутствовал в снимке), чей
  LocationURL file:/// после percent-decode ТОЧНО равен нашей приватной папке
  (нормализация Windows); ctypes GetWindowThreadProcessId → владелец обязан
  быть каноническим %WINDIR%\explorer.exe со стабильным повторным чтением
  birth (защита от PID reuse) и не из будущего;
- UIA fallback: только НОВЫЙ hwnd + owning PID + точное значение адреса;
  подстрока/только title никогда не приёмка;
- чужой Explorer никогда не завершается и не управляется; нет доказательства →
  честный unclosed (folder_completed=false, full_acceptance=false, exit 1).
- preparation=true до реального прогона; false после preflight нативного run.
- Опрос папки возвращает None до дедлайна (wait_for не ловит Blocked) —
  мгновенного падения нет.
Offline-контракт дополнен: new-hwnd+exact URL (percent-encoding), old hwnd,
URL .extra, wrong exe, future birth, UIA exact vs substring/old — PASS.

## Незакрытые критерии

1. Реальный запуск на GHA Windows — за ROOT (после review).
2. UIA-доступность адреса Explorer на раннере — может дать честный `unclosed`.
3. Риск `\\?\` — до нативного прогона остаётся риском; фикс — за ROOT.
