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

Оба контекста должны вызвать `OnRequestContextInitialized` до настройки proxy и создания Browser. Предельное ожидание инициализации10с. JSONL stdout содержит по A/B `context`, булевы `can_set_proxy`, `set_proxy_ok`, `sharing`. Эти поля — измерения API, не подтверждение маршрута. Значения cookies/storage/proxy preference не выводятся.

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
