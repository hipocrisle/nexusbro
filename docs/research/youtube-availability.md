# YouTube availability — ChallengeRequired (исправленная версия, 25.11)

Ветка: research/youtube-availability. Исправление после проверки Codex: первичный
источник — ИСХОДНИКИ pinned yt-dlp, не README; README 2026.11.23 рассинхронизирован
с кодом. Без экспериментов, без патчей, production не менялся.

## Подтверждённые факты (primary sources)

1. **Дефолтные player-клиенты yt-dlp 2026.11.23 (ИСХОДНИК, не README):**
   `yt_dlp/extractor/youtube/_video.py:138` → `_DEFAULT_CLIENTS = ('tv_samsung', 'visionos')`;
   `_DEFAULT_JSLESS_CLIENTS = ('visionos',)`. Анонимный запрос с JS-runtime берёт
   именно их (`_get_requested_clients`, строки 2857–2904 зоны).
   **README (line ~1689) утверждает `tv_samsung,visionos,web` — это расхождение
   README с кодом; коду верить.** Следствие: наш sidecar, не задающий
   `--extractor-args`, уже НЕ использует `web`-клиент для player-запроса.
   Прежний вывод отчёта 72837b3 («подтверждённый дефект из-за web») — **отменён**.
2. **Наш sidecar** (`youtube_light_sidecar.rs`, `command()` ~122–154): флаги
   `--no-*` корректны, EJS-runtime явный, env очищается, production `chrome=false`.
   Кодового дефекта в интеграции, кроме уже исправленного parser-бага, не найдено.
3. **Parser-fix 34be02e: live НЕ принят.** На Home запрос с исправленным helper дошёл
   до ChallengeRequired на этапе extractor — parser не выполнялся. Есть только unit
   PASS. Заявлять «исправление подтверждено на живом трафике» нельзя.
4. **ChallengeRequired — факты наблюдений, без доказанной причины:**
   - датацентер (GHA): challenge стабильно, обе ОС;
   - один impersonate-chrome эксперимент (454548): challenge тоже — **один провал
     НЕ доказывает, что TLS-отпечаток не причина** (возможны и другие сигналы);
   - Home residential: 1 успех (JSON 173106B/40 форматов) → затем challenge на
     повторе — **это НЕ доказывает** репутацию IP/частоту как причину
     (селекция клиента, видео, время суток — не исключены).
   Итого: причина ChallengeRequired для наших запросов **не установлена**.
5. **Stream-Tube-Desktop (main, КОД, не README):**
   - `src/api/innertube.rs:18` — для browsing/search используется WEB-клиент
     (это поиск/ленты, не player endpoint);
   - `src/api/streams.rs:67-79` — player: собственные `visionos()` → `android_vr()`
     цепочка (клиент выбираемый), комментарий: visionOS отдаёт HLS-манифест без
     PO Token; yt-dlp — один из выбираемых playback-клиентов;
   - `src/downloads.rs:280,306` — загрузки через yt-dlp с явным
     `--extractor-args youtube:player_client=ios;formats=missing_pot`;
   - `src/downloads.rs:277` — их тоже бьёт: "videos made for kids are refused to
     yt-dlp's default clients"; README сам допускает bot errors.
   Реальная разница архитектур: player-метаданные через собственный visionOS/
   AndroidVR Innertube-клиент (не через yt-dlp web-экстрактор), но их решение
   **тоже не иммунно** — это та же кошки-мышки.

## Гипотезы (НЕ доказаны, в порядке проверяемости)

- Г1: ChallengeRequired приходит от конкретного клиента (tv_samsung или visionos);
  проверяется изоляцией клиентов, а не «магическим патчем».
- Г2: Сокращение числа запросов (`player_skip=configs,webpage`, офиц. аргумент)
  снижает частоту challenge — гипотеза, не обещание.
- Г3: Внешний фактор (IP/ASN/частота) доминирует — ни один имеющийся факт её не
  доказывает и не опровергает; серия Home-наблюдений с контролем переменных
  могла бы её прояснить, но владельцу не обещать стабильность.

## Проверенное основание для следующего bounded эксперимента

Цель — **диагностика, не фикс**: установить, от какого клиента/этапа приходит
challenge. Один bounded прогон (после согласования с Codex, на Home, residential,
официальные тестовые видео типа Big Buck Bunny aqz-KE-bpKQ, sealed receipts,
без публикации сырых URL):
1. helper с `--extractor-args youtube:player_client=tv_samsung` — N=2;
2. helper с `youtube:player_client=visionos` — N=2;
3. (опционально) `youtube:player_client=visionos;player_skip=configs,webpage` — N=2.
Исходы: JSON vs ChallengeRequired по каждому клиенту → факт «какой клиент уязвим
для нашего контура». Это основание решения (вписывать client-политику в sidecar,
жить с пониженным качеством/ограничениями, или признать внешний блокер) — а не
обещание «стало работать».

## Варианты продукта (без обещаний стабильности)

- **В1 (диагностика first)**: сначала эксперимент выше; только по его фактам —
  минимальная client-политика в sidecar (например, предпочтение visionos при
  ChallengeRequired от tv_samsung) + понятный класс ошибки пользователю.
- **В2**: собственный прямой Innertube-клиент (путь Stream-Tube) — недели работы,
  cat-and-mouse переезжает к нам; их README допускает bot errors — иммунитета нет.
- **В3**: iframe embedded player — конфликтует с nb-lite CSP (`frame-src none`,
  MAIN-grant нельзя ослаблять); реклама, нет загрузок. Не рассматривается.

## Следующий шаг

Передать Codex: причина ChallengeRequired **не установлена**; README yt-dlp
рассинхронизирован с исходником (default = tv_samsung,visionos, без web);
предложен bounded диагностический эксперимент с изоляцией клиентов (схема выше).
Production не менять до фактов эксперимента.

## Primary sources

- `yt-dlp 2026.11.23: yt_dlp/extractor/youtube/_video.py` (raw.githubusercontent) —
  line 138 `_DEFAULT_CLIENTS`; lines 2857–2904 `_get_requested_clients`/POT policy.
  Расхождение: README.md line ~1689 (default `tv_samsung,visionos,web`).
- Stream-Tube-Desktop main: `src/api/innertube.rs` (WEB для browsing),
  `src/api/streams.rs` (visionos→android_vr player), `src/downloads.rs`
  (YTDLP_IOS = `youtube:player_client=ios;formats=missing_pot`).
- Наш код: `desktop/src-tauri/src/youtube_light_sidecar.rs` (command/extract).
