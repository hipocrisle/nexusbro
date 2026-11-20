# CEF156: эксперимент RequestContext / proxy

Изолированный Linux/Xvfb example, не продуктовый код. Сборка и запуск только GitHub CI; на DE-1 не запускать. Никаких реальных аккаунтов, подписок или VPN.

```sh
cargo build --locked --example route_context_probe
CEF_PATH=/absolute/cef xvfb-run -a target/debug/examples/route_context_probe \
  --probe-root /absolute/nonexistent/disposable/scenario \
  --scenario independent \
  --proxy-a http://127.0.0.1:18081 \
  --proxy-b http://127.0.0.1:18082 \
  --target-base http://127.0.0.1:18080
```

Запуск из `desktop/src-tauri`. Использовать новый `--probe-root` для каждого сценария. CEF runtime и LD_LIBRARY_PATH предоставляет workflow. Отдельный процесс на сценарий исключает перенос command-line/preferences между сценариями. Бинарник сам обслуживает CEF subprocess entry до разбора probe-аргументов.

Сценарии:

- `independent`: A и B имеют разные дисковые cache_path; A получает proxyA, B proxyB.
- `shared`: B создаётся штатной shared-фабрикой от A; сначала выставляется proxyA на A, затем proxyB на B. Это намеренно проверяет общую preference, НЕ обещает независимые пути.
- `cli`: независимые A/B, но процесс дополнительно получает `--proxy-server=proxyA`; проверяет управляемость preference и фактический приоритет CLI.

Оба контекста должны вызвать `OnRequestContextInitialized` до настройки proxy и создания Browser. Предельное ожидание инициализации10с. JSONL stdout содержит по A/B `context`, булевы `can_set_proxy`, `set_proxy_ok`, `sharing`. Эти поля — измерения API, не подтверждение маршрута. Значения cookies/storage/proxy preference не выводятся. Для диагностики выводятся булевы результаты setters/validity, тип Value и тип зарегистрированной preference, а также ошибка SetPreference длиной до250 символов. Это только синтетический loopback experiment.

После настройки обеих preferences открываются настоящие CEF Browser A/B, соответственно same-origin `/first?context=A` и `/second?context=B`. Target и оба proxy проверяются через URL parser: только HTTP, host 127.0.0.1/localhost/[::1], явный нестандартный порт, пустые username/password, путь /, без query/fragment. Independent/shared не получают CLI proxy-флагов. Только в CLI-сценарии установлен `proxy-bypass-list=<-loopback>`; во всех сценариях запрашивается `bypass_list=<-loopback>` в preference, чтобы implicit bypass не делал тест фиктивным. Внешние fixture URLs и реальные сайты запрещены. Python может детерминированно вернуть `/second` только после POST `/first`, чтобы измерить видимость cookies/localStorage без гонки.

Python обязан:

1. Поднять target и два отличимых HTTP proxy, записать фактические запросы A/B и прямые обращения target.
2. На `/first` выставить синтетические cookie/localStorage и отправить отчёт; `/second` прочитает их и отправит свой отчёт. Получение страниц самим Chromium ещё не доказывает выполнение JS.
3. Только после получения обоих POST-отчётов создать пустой `<probe-root>/done`.
4. Проверить receipts отдельно от API JSON, включая отсутствие незаявленных direct-fallback. Для shared сравнить общность хранилища и фактический маршрут без предположения об успешном независимом routing.

Main loop ограничен35с от начала подготовки контекстов. После `done` даётся1с на завершение fixture-запросов, затем оба Browser закрываются и ожидаются `OnBeforeClose` максимум5с. Нормальный выход вызывает `cef::shutdown`. Если живые Browser не закрылись, процесс выходит с ошибкой без небезопасного shutdown; внешний CI supervisor обязан убирать всё дерево процессов. Внешний Python supervisor ограничен55с с учётом CEF initialize и cleanup. Отсутствие done/успешных main-frame load даёт ненулевой exit; последнее JSON `pages_loaded` не означает route SUCCESS. Ошибки также выдаются JSON.

Сейчас выполнены только статическое сопоставление API с установленным cef-rs156 и rustfmt. Компиляция, runtime-маршруты и общность хранилища НЕ проверены до запуска GitHub job. Продуктовые профили и маршрутизация не изменяются.

Для запуска новой workflow до её появления на default branch этот probe-branch
содержит временный launcher в зарегистрированном build.yml. Не сливать этот
launcher в продуктовую ветку: переносить только example, Python, отдельную
route-context-probe.yml и документацию. Маршруты пользователя не реализованы.

Диагностика первой runtime-попытки: independent дал can_set_proxy=true, set_proxy_ok=false, фактический DIRECT/DIRECT. Подтверждения независимых proxy пока нет. Официальный CEF pref_helper.cc проверяет регистрацию, изменяемость, валидность Value и совпадение его типа; Value::SetDictionary сам возвращает true после привязки dictionary. Поэтому добавлены измерения каждого этапа без изменения ожидаемых маршрутов.

Установлена причина пустой ошибки до вызова helper: cef-rs156 `CefStringData::default()` — Borrowed(None), преобразование `&mut CefStringUtf16` даёт NULL. Официальный [генератор CEF C-to-C++](https://github.com/chromiumembedded/cef/blob/8078/tools/make_cpptoc_impl.py) проверяет обязательный string_byref и возвращает0 при NULL; [SetPreference](https://github.com/chromiumembedded/cef/blob/8078/include/cef_preference.h) помечает optional только value, не error. Теперь output error создаётся `CefString::from("probe-placeholder")`, чтобы передать настоящий writable cef_string_t. На успешном вызове placeholder не выводится; на неуспешном ограниченная диагностика сохраняется. Фактические маршруты по-прежнему проверяются только Python receipts; исправление FFI ещё должно пройти runtime CI.

После сбоя Ubuntu APT в44520751378 probe переведён на минимальный самостоятельный
Cargo harness tests/route_context_probe (тот же Rust example, фиксированные
cef/serde_json/reqwest). Он не собирает Tauri/GTK/WebKit и не требует apt/NPM.
Xvfb и iproute2 уже присутствуют в GitHub Ubuntu24; наличие проверяется явно.
При отсутствии runtime-библиотеки ldd останавливает тест, без фиктивного успеха.
Команда из корня: cargo build --release --locked --manifest-path
tests/route_context_probe/Cargo.toml --target x86_64-unknown-linux-gnu.
Локально наDE1 выполнена только offlineгенерацияCargo.lock, не Rustсборка.

## Windows follow-up: old cache and HttpOnly authentication

The `probe/route-windows` branch extends the same bounded synthetic example; it
is never a product release workflow. It uses the hosted MSVC runner and the
product's exact verified Windows CEF archive. No Rust/CEF runs on the DE-1 host.

Three existing capability scenarios remain: independent, shared and CLI. An
additional legacy sequence starts three distinct native engine processes while
keeping one marked disposable root and the effective original cache path returned by CEF:

1. `legacy_seed`: the original global context with CLI proxy A receives a real
   persistent HttpOnly cookie, a session HttpOnly cookie, and localStorage.
2. `legacy_reopen`: no CLI proxy, context preference B, the exact same effective global
   root/cache. The server checks the actual Cookie header, and JS confirms
   HttpOnly cookies are not exposed through `document.cookie`. Persistent auth
   and localStorage must survive; session-cookie survival is measured, not
   assumed or silently enabled.
3. `legacy_parallel`: original effective default context through A and a fresh override
   subdirectory through B navigate concurrently. The original context must send
   its persistent HttpOnly auth; the override must see none of the original
   cookies/localStorage. No profile, Local State or cookie database is copied.

All cookie values are synthetic, but HttpOnly values are never printed. Native
flush and normal-close callbacks are required before a process is restarted.
The supervisor limits each engine to55s,40 owned processes and3GiB working set.
A temporary per-executable outbound firewall rule on the disposable Windows CI
runner blocks non-loopback IPv4 and all IPv6; fixtures use127.0.0.1 only. Rule
creation is verified and failure stops the test. The rule is removed in finally;
no firewall or service on DE-1 is touched. This is process-scoped isolation, not
an operating-system network namespace or a proof about Windows DNS services.

Coverage is basic HTTP/fresh synthetic profiles plus same-root/cache persistence.
HTTPS, service-worker startup leakage, actual VPNs, product integration and real
user profile migration remain explicitly unverified. `product_feature_ready`
stays false even if the capability matrix passes. Build and runtime results are
pending until the hosted Windows job completes.

A fourth legacy phase, `legacy_explicit`, leaves the global disk cache empty and
opens the recorded original effective cache through an explicit RequestContext with its own
handler, alongside a second fresh override subdirectory. The same server-side
auth/storage and concurrent-route assertions apply. This specifically measures
whether old Windows encrypted cookie storage remains readable with an unchanged
root and cache path when the global context no longer owns the disk cache. It
does not itself implement or prove a service-worker network admission gate.

First Windows run `44726414717` (source `fee5954`) compiled successfully. The
measurement did not start because the per-program firewall rejected an address
prefix/range. The follow-up uses a full CIDR decomposition excluding127/8 and no
`/0` prefixes, preserving the outbound block rather than dropping isolation.
No routing or authentication success is claimed from that first run.

Second Windows run `44727862779` compiled and installed the strict firewall.
Independent A/B, shared B/B and CLI A/A matrices all passed with real concurrent
navigation/storage observations. The legacy phase stopped before opening its
page: its preliminary same-path context was not reported as sharing with the
global context. The follow-up uses the documented shared-context factory only
as a global readiness barrier and separately requires the native global cache
path to equal the original `root/kesh` directory after filesystem canonicalization.
The actual server-side HttpOnly/auth and explicit-cache reopen gates remain
unchanged; no authentication preservation was established by that failed run.

Third Windows run `44728427390` measured a real effective-path distinction:
CEF reports `global_cache_leaf:"Default"`, not the requested `kesh`. This follows
the exact CEF156 [`CefSettings.cache_path` documentation](https://github.com/chromiumembedded/cef/blob/8078/include/internal/cef_types.h): a child value is ignored in favour
of the default profile. It explains why a separately requested `kesh` context
was not sharing the existing global profile. It is not evidence of lost cookies.

The corrected probe records only the native effective cache leaf in a guarded
metadata file under its own disposable root. Global reopen must resolve to the
same canonical directory; explicit legacy contexts open that very directory
when the global disk cache is empty. The path must remain an immediate child of
the same root, with no profile/cookie/Local State moves or copies. Server-side
HttpOnly authentication and storage-preservation assertions are unchanged.
Partial server observations are retained even if a later legacy phase fails.
