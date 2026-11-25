# YouTube official embed — feasibility probe (изолированный зонд)

Ветка: research/youtube-embed-probe (worktree /matrix/nexusbro-kimi-embed, base c88ba5ea).
Статус: **скрипт готов после review KIMI-EMBED-REVIEW, self-test PASS, нативный
прогон НЕ запускался** (запускает ROOT/CI на кандидате FULL 45592261113, source 1f8ece5).
Production/workflow/desktop/существующие тесты не менялись.

## Ключевая правка по review

`client.js` оценивает DOM оболочки Tauri, НЕ страницу CEF. Поэтому:

- Страница fixture общается с тестом **только через HTTP**: POST `/receipt`
  (sendBeacon, typed allowlisted payload ≤4096 Б, сервер валидирует и хранит
  последнюю сводку под lock; наблюдатель читает принятые HTTP-события из того же серверного объекта под lock; GET `/summary` остаётся доступен только для диагностики).
- Trusted click — **один физический `client.mouse(pole.x+30, pole.y+30)`**;
  кнопка fixture абсолютно позиционирована в (16,16), 220×64, сплошная жёлтая,
  без margin — точка (30,30) гарантированно внутри.
- OS paint доказывается **ImageGrab в owned Xvfb**: пиксель (30,30) от .pole
  должен быть жёлтым; DOM-геометрия как доказательство paint не принимается.
- Shell-side js используется только для `.pole` bounds / identity / rows —
  как в реальных референс-фикстурах репо.

## Официальный API

Только `https://www.youtube.com/iframe_api` + `YT.Player` с onReady /
onStateChange / onError; `player.getCurrentTime()` на таймере (500 мс).
Ручной postMessage-протокол удалён (необещанный документами).
Trusted DOM-обработчик кнопки зовёт `player.playVideo()` **ровно один раз**
после onReady и только при `e.isTrusted`.

## Границы данных

Из страницы наружу: stage (known strings ≤32), ready/played/invalid bool,
states {-1,0,1,2,3,5}, errors {2,5,100,101,150,153} (обе ≤512, строго int —
никаких coerce `|0`), finite currentTime. Второй allowlist-контур — на стороне
теста в `summarize()`. Сырые URL/ошибки/cookies не сохраняются.
Отчёт всегда `full_playback_acceptance=false`, `paired=false` — API-часы не
доказывают ни paired-decode, ни отсутствие рекламы.

## Исходы

- `completed=true` + `playing_observed=true`: официальный embed стартует в
  обычном CEF → основание для отдельного честного режима (ограничения
  качества/рекламы/фона/загрузок) без ослабления nb-lite CSP/MAIN-grant.
- Любой `blocked_reason` (embed_refused 101/150, unexpected code,
  playing_not_observed, receipt_invalid): честный blocked, playback не заявлен.

## Честные ограничения готовности

- Self-test покрывает: оба allowlist-контура, валидацию receipt-схемы, геометрию
  кнопки/клика, отсутствие postMessage-протокола, единственность внешнего
  origin, старт fixture-сервера.
- НЕ проверено вживую (только в нативном прогоне ROOT): реальный POST /receipt
  из CEF-страницы (sendBeacon/POST), жёлтый пиксель через ImageGrab на owned
  Xvfb, поведение YT.Player при challenge/рекламе.

## Следующий шаг

ROOT: один запуск зонда на существующем кандидате (`--exe --cef --out`),
результат — `result.json` рядом с out.


## Проверка ROOT перед первым CI

Исправлена связь настоящего HTTP POST с наблюдаемым серверным состоянием. Self-test
теперь отправляет реальный локальный POST, читает принятый receipt и проверяет
отказ NaN вместо прямой подстановки приватного поля. Перед единственным кликом
обязательно получен onReady; жёлтый пиксель проверяется точно в owned Xvfb.
Прогресс считается по двум API-замерам после клика и минимум 8 секундам реального
времени, а не по одному абсолютному currentTime. Отчёт остаётся диагностическим.

Запуск через существующий input без добавления 26-го параметра:
`linux_runtime_build={"run":"45592261113","probe":"official_embed"}`.
Workflow проверяет producer source/совпадение desktop, использует тот же бинарник,
не запускает посторонние native-сценарии и запрещает publisher для JSON-запроса.
Файл результата: `.dev-check/youtube-official-embed/result.json`; публикуется только
этот небольшой JSON. Это один эксперимент, без client roulette и без yt-dlp.
