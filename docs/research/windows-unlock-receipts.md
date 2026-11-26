# Windows unlock — санитизированные квитанции (диагностический пакет)

Worktree /matrix/nexusbro-kimi-unlock-receipts, ветка test/windows-unlock-receipts (base 5bbc225).
Файлы: точечно `tests/gui/youtube_player_native.py` (ТОЛЬКО trusted_fill/lock_receipt +
обогащение failure-отчёта; ExitWitness не тронут), НОВЫЕ
`tests/unlock_receipts_contract_test.py` и эта документация. GUI/CI/Home не трогались.

## Причина пакета (факт)

Windows producer 45780933571: stage `physical-unlock-B`, failure_code `unclassified`,
trace require в run. Какой именно статический guard упал и с каким типом ошибки —
невидимо: карта сообщений покрывает известные NativeFailure, но всё остальное
(тип ошибки, ожидавшийся шаг) терялось. Первопричина НЕ объявляется env без фактов.

## Что добавлено (без ослабления)

`diagnostics['unlock']` — санитизированная последовательность вокруг ровно ОДНОГО
trusted fill и одного физического клика кнопки (как и прежде; никаких ретраев,
JS form submit, ослабления browser identity/old-grant guard):

- `guard_pending` перед каждым статическим guard: `trusted_fill` →
  `unlock_button_click` → `master_ui_unlock` → `lock_browser_identity` →
  `old_grant_refusal` → `old_grant_revoked` → None. При падении отчёт получает
  `unlock_guard_pending` — видно, КАКОЙ guard ожидался.
- `fill`: `input_present`, `input_enabled`, `form_focused`, `value_matches` —
  только булевы; никогда passphrase/значение/длина/хэш.
- `button`: visible/enabled (или null, если кнопка неизвестна).
- `gate_present`/`gate_closed`, `okno_exists` — булевы.
- `browser_unchanged` (== до lock), `browser_positive` (>0) после unlock.
- `old_refusal_received`, `old_state_absent`, `old_error_present` — булевы.
- В отчёт при ошибке: `failure_type` из allowlist
  (NativeFailure/AssertionError/RuntimeError/OSError/TimeoutExpired/TypeError/
  KeyError/StopIteration/WebSocketException/Error, иначе `Other`) — «unclassified»
  больше не безликий; строки ошибок не пишутся.

Существующая карта фиксированных сообщений (unlock_input_missing,
unlock_input_mismatch, master_unlock_failed, lock_browser_changed,
old_grant_refusal_missing, old_grant_survived и др.) сохранена и покрыта
контракт-тестом.

## Гипотеза (НЕ факт, не исправлять без native-фактов)

Если при повторении `browser_unchanged=false` при `browser_positive=true` —
lock пересоздал браузер (гонка replace); если `browser_now==0` transient —
`browser_positive=false` это покажет. Различие появится в квитанции; код не
менялся.

## Проверки (offline)

`tests/unlock_receipts_contract_test.py` — PASS: receipt только булевы и без
секрета; порядок guard_pending; ровно один fill/клик; нет JS-submit/ретраев;
все поля квитанции; allowlist типа + pending guard в отчёте; карта сообщений
и require'ы не ослаблены.
