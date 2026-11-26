# ExitWitness — санитизированные диагностические квитанции (диагностический пакет)

Worktree /matrix/nexusbro-kimi-exit-witness, ветка test/exit-witness-diagnostics (base 98d7823).
Файлы: точечно `tests/gui/youtube_player_native.py` (ExitWitness + finally
close_for_privacy), НОВЫЕ `tests/exit_witness_diagnostics_test.py` и эта документация.
Нативных запусков/CI/GUI не было; production-проверки НЕ ослаблены.

## Причина пакета (факт из прогона)

Linux producer 45780933571 (source 7b8) упал после 16 real-проверок на
`privacy-normal-owned-exit` с `failure_code privacy_observation_incomplete`
(require в close_for_privacy). `ExitWitness.run()` ловил `except Exception:
self.error=True` — ПРИЧИНА терялась: неизвестно, что именно сломалось
(AccessDenied при обходе children? identity_reused? иное OSError?).

## Что добавлено (без ослабления)

- `ExitWitness._record_error(exc, where)`: санитизированная typed-квитанция —
  `error_class` строго из allowlist (иначе `Other`), числовой `errno` только
  bounded int 0..4095 (иначе null), `where` — собственная точка
  (`run.sample`/`all_gone`/`close`), плюс счётчики `samples_seen`,
  `identities_known`, `roots`. Никаких строк exception, путей, аргументов, PID.
- `run()` записывает квитанцию вместо голого `error=True`; `samples` считается.
- `all_gone()`/`close()` гарантируют квитанцию, если error выставлен без неё
  (defensive), и при живом потоке после join.
- `close_for_privacy` finally: `report['exit_witness_error']=witness.error_info`
  при наличии — квитанция попадает в отчёт ДАЖЕ при падении require выше.
- НЕ изменено: ownership/birth сверки, `require(not self.error)` в
  all_gone/close, no-orphans и normal-shutdown проверки; AccessDenied и
  RuntimeError по-прежнему роняют наблюдение (не превращаются в PASS);
  ретраев и искусственного exit нет. `NoSuchProcess` остаётся штатным
  PASS-путём; Linux FileNotFound→NoSuchProcess в kernel_birth — прежнее
  поведение resource_soak, не тронуто.

## Проверки (offline, без браузеров)

`tests/exit_witness_diagnostics_test.py` — PASS:
- AccessDenied в sample → квитанция class=AccessDenied, where=run.sample,
  счётчики; JSON квитанции без путей/PID/строк;
- errno: bounded int проходит, отсутствующий → null, путь из FileNotFoundError
  не утекает; неизвестный класс → `Other`;
- identity_reused (ребёнок со сменившейся личностью) → RuntimeError записан;
- NoSuchProcess → прежний PASS, error=False, квитанции нет;
- реальный крошечный own-процесс (`sleep`): sample фиксирует, после выхода
  `all_gone()` истинно — жизненный цикл без браузера;
- source-факты: квитанция в finally, require не ослаблен.

## Что это даёт следующему прогону

При повторении `privacy_observation_incomplete` в result.json появится
`exit_witness_error` с typed-причиной и местом — развилка
«AccessDenied-окружение vs identity_reused vs иное» станет различимой без
утечки данных. Если реальный баг будет доказан кодом — передаётся условием,
фикс без ослабления только через ROOT review.
