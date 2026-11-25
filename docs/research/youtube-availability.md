# YouTube availability — исследование ChallengeRequired (лёгкий режим NexusBro)

Дата: 2026-11-25. Ветка: research/youtube-availability (worktree /matrix/nexusbro-kimi-ytavail).
Границы соблюдены: только чтение кода/малые официальные страницы, без yt-dlp сетевых
вызовов, без Home SSH, без изменения существующих файлов.

## Подтверждённые факты

1. **Наш sidecar не задаёт `player_client`.** `desktop/src-tauri/src/youtube_light_sidecar.rs`
   (`command()`, строки ~122-154) собирает yt-dlp с `--ignore-config --no-plugin-dirs
   --no-remote-components --no-cache-dir --no-cookies --no-js-runtimes +
   --js-runtimes deno:<pinned>` и т.д., но НЕ передаёт `--extractor-args`. Production
   вызов (`extract()`, строки 619-636) использует `chrome=false` → без `--impersonate`.
2. **Дефолт yt-dlp 2026.11.23 при наличии JS-runtime: `tv_samsung,visionos,web`**
   (README pinned tag, раздел `youtube` extractor-args: "By default, `tv_samsung,visionos,web`
   is used"; без JS-runtime `web` и `tv_samsung` опускаются). Значит наш запрос
   **включает `web`-клиент** — самый защищённый Innertube-клиент (PO Token контексты).
3. **ChallengeRequired — подтверждённый внешний паттерн, не единичный сбой:**
   - датацентер (GHA): challenge стабильно, обе ОС; изолированный impersonate-chrome
     эксперимент (run 454548) дал тот же ChallengeRequired → TLS-отпечаток не причина;
   - Home (residential RU): запрос №1 → реальный JSON 173106B / 40 форматов; запрос №2
     через минуты → ChallengeRequired 548B. Это репутация IP + частота запросов, не
     только класс IP.
4. **Парсер-дефект интеграции** (формат без обоих кодеков валил весь список) — реальный
   баг, уже исправлен в 34be02e и подтверждён на Home. Других однозначных дефектов
   параметров sidecar по коду не найдено: флаги `--no-*` корректны, EJS-runtime
   передаётся явно, env очищается.
5. **Stream-Tube-Desktop (Avaneesh-Inamdar, main 0.7.3-alpha) — реальная разница
   архитектур, не маркетинг:** README (primary source) — playback идёт через
   **собственный прямой Innertube-клиент с клиентами visionOS/AndroidVR** ("a playback
   client (visionOS, Android VR or yt-dlp)", Cargo.toml yt-dlp как зависимость
   отсутствует — yt-dlp только для загрузок), libmpv для проигрывания, встроенный
   fallback-даунлоадер 360p/720p без yt-dlp. `web`-клиент для playback не используется
   вообще; README сам допускает bot errors — то есть и этот путь не пуленепробиваемый,
   это та же кошки-мышки.

## Вывод по блокеру

**ChallengeRequired при анонимном доступе — в основном внешнее ограничение**
(репутация/класс IP + частота запросов + выбор Innertube-клиента), а не доказанный
дефект нашего кода. Единственный найденный кодовый дефект (parser) уже починен.
Исключение `web`-клиента из запроса — обоснованная гипотеза снижения частоты
challenge (поддержана официальной документацией yt-dlp и архитектурой Stream-Tube),
но НЕ доказанная: доказать может только bounded-эксперимент, а гарантировать
стабильность нельзя в принципе — YouTube меняет правила без предупреждения.

## Варианты (с границами)

### В1. Исправление supported extractor-интеграции (рекомендуется, минимальный)
Добавить в `command()` sidecar один аргумент:
`--extractor-args youtube:player_client=tv_samsung,visionos,android_vr`
(все три работают без PO Token; `-web` официально поддерживаемый синтаксис:
`player_client=default,-web`). Это не ослабляет CSP/frame-src, cookie-изоляцию,
routes; не трогает cookies владельца; чисто выбор Innertube-клиентов.
Файлы: `youtube_light_sidecar.rs` (args + юнит-тесты состава команды рядом,
паттерн уже есть: `tests` в конце файла). Полный diff — ниже.
Минимальный ОДИН эксперимент (после согласования с Codex): на Home, через
существующий pinned helper, фиксированное официальное тестовое видео
(Big Buck Bunny aqz-KE-bpKQ — уже использовалось), N=3 с аргументом vs N=3 без,
сравнение JSON-vs-Challenge по sealed receipts, без публикации сырых URL/JSON.

### В2. Самостоятельный anonymous direct player (путь Stream-Tube)
Свой Rust-Innertube-клиент (visionos/android_vr) вместо yt-dlp для метаданных
playback. Плюс: контроль, нет JS-runtime зависимости. Минус: недели работы,
дублирование логики yt-dlp (n/sig, форматы, age-gate), cat-and-mouse переезжает
к нам; Stream-Tube с тем же подходом ВСЁ РАВНО допускает bot errors. Не рекомендую
сейчас.

### В3. Официальный embedded player (iframe) — честные ограничения
iframe youtube-nocookie внутри MAIN-frame: требует ослабить `frame-src none` и
нативный MAINframe-grant — **по границам задачи нельзя**. Плюс реклама, нет
загрузок, ограничения фона. Не рекомендую.

## Рекомендация

В1: минимальный интеграционный патч (один аргумент + тесты) + честная документация
«анонимный доступ не гарантирован; при ChallengeRequired — понятный класс ошибки
и fallback». Если эксперимент на Home не покажет разницу — прямо зафиксировать,
что блокер внешний, и решать на уровне продукта (отложить лёгкий режим или
допускать инициируемый владельцем sign-in flow — вне этого исследования).

## Reviewable patch (только для согласования, НЕ применять)

```diff
--- a/desktop/src-tauri/src/youtube_light_sidecar.rs
+++ b/desktop/src-tauri/src/youtube_light_sidecar.rs
@@ c.args([... "--no-js-runtimes",]) .arg("--js-runtimes") ...
+        // Stay on PO-Token-free Innertube clients: the default set includes
+        // the `web` client, which is the most challenged for anonymous
+        // datacenter requests (ChallengeRequired). Supported yt-dlp syntax:
+        // README 2026.11.23, youtube:player_client.
+        .args([
+            "--extractor-args",
+            "youtube:player_client=tv_samsung,visionos,android_vr",
+        ])
```
Юнит-тесты: расширить существующие проверки состава args в `tests`-модуле
файла (паттерн `args.contains(...)` уже используется, см. строки ~874-915).

## Следующий шаг

1. Codex согласует файлы/изменения (sidecar args + тесты).
2. Один bounded Home-эксперимент N=3 vs N=3 по плану выше.
3. По фактам: либо влить В1 с документацией о негарантии, либо зафиксировать
   внешний блокер и эскалировать решение владельцу.

## Primary sources

- yt-dlp README 2026.11.23 (pinned): `https://raw.githubusercontent.com/yt-dlp/yt-dlp/2026.11.23/README.md` — раздел `#### youtube`, `player_client` default `tv_samsung,visionos,web`, синтаксис исключения `-web`, `player_skip`, `po_token`, `fetch_pot`.
- Stream-Tube-Desktop main (0.7.3-alpha): `https://raw.githubusercontent.com/Avaneesh-Inamdar/Stream-Tube-Desktop/main/README.md` (playback clients visionOS/AndroidVR/yt-dlp, строки ~100-104; downloads через yt-dlp со fallback) и `Cargo.toml` (yt-dlp не зависимость).
- Наш код: `desktop/src-tauri/src/youtube_light_sidecar.rs` строки 122-200 (command/configure), 619-636 (extract, chrome=false), 874-915 (тесты состава args).
