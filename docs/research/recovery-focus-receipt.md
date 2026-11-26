# Recovery-lock focus — санитизированная квитанция (диагностический пакет)

Worktree /matrix/nexusbro-kimi-recovery-focus, ветка test/recovery-lock-focus (base e7db9ac).
Файлы: точечно `tests/gui/windows_physical.py` (recovery_lock + квитанция фокуса),
НОВЫЕ `tests/recovery_focus_receipt_test.py` и эта документация. GUI/CI/Home не трогались.

## Причина пакета (факт прогона 45786766631/source 7b)

`windows_physical recovery_lock initial-lock-before-reset-cancel`: после ЕДИНСТВЕННОГО
Ctrl+Shift+L UIA не нашёл поле мастер-пароля; по логу видимые элементы — обычная
оболочка/placeholder Sleeping, формы блокировки нет. Почему shortcut не привёл к
gate — неизвестно; env-причиной по факту UIA-missing это НЕ объявляется.

## Что добавлено (без изменения поведения)

`recovery_focus_receipt()` — санитизированная квитанция ДО единственного shortcut и
ПОСЛЕ (в т.ч. на пути ошибки, с сохранением первоначального исключения без изменений):

- `owned_foreground` — foreground-окно принадлежит семейству приложения (bool);
- `focus_role` ∈ {webkit, cef, shell, none, foreign} — чистая классификация
  `recovery_focus_role` (module-level, тестируемая): main-процесс → shell,
  msedgewebview2 → webkit, свой процесс с `--type=`/`--nb-dvizhok` → cef,
  вне семейства → foreign, нет фокуса → none. Семейство = PID окна приложения +
  дочерние процессы (psutil); класс окна Chrome_RenderWidgetHostHWND сам по себе
  CEF от WebView2 не различает — поэтому роль по PID/процессу, как в существующей
  диагностике shell_keyboard_native_focus (:1012, существующий типизированный
  паттерн, переиспользован; нового app-кода не добавлено);
- `ctrl_released`/`shift_released` — GetAsyncKeyState, клавиши отпущены (bool);
- `uia_gate` — present/missing (булевы состояния, без текста).
Никаких raw-заголовков, PID, путей, аргументов в квитанции.

Ровно ОДИН Ctrl+Shift+L как прежде; никаких повторов shortcut, JS-блокировки
вместо пользовательской, обхода восстановления. Квитанция пишется в results
(`recovery_lock_focus`: stage/before/after) и на пути ошибки — перед re-raise.

## Гипотезы (НЕ факты, различимые квитанцией)

- `owned_foreground=true`, `focus_role=shell` → shortcut ушёл оболочке, а не
  CEF/webview-поверхности (актуальный обработчик мог требовать webview-фокус);
- `focus_role=none` → placeholder Sleeping оставил фокус пустым;
- `focus_role=foreign` → стороннее окно перехватило foreground.
Следующий Windows-прогон с этой квитанцией различит ветку; код не менялся.

## Проверки (offline)

`tests/recovery_focus_receipt_test.py` — PASS: классификация ролей (включая
webkit/cef/foreign/none), allowlist ролей, один shortcut без ретраев/JS,
исходная ошибка re-raise, квитанция до/после и на пути ошибки, фиксированные
поля без raw-значений.
