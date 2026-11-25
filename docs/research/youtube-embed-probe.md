# YouTube official embed — feasibility probe (изолированный зонд)

Ветка: research/youtube-embed-probe (worktree /matrix/nexusbro-kimi-embed, base c88ba5ea).
Статус: **скрипт готов, self-test PASS, нативный прогон НЕ запускался** (запускает ROOT/CI).
Production/workflow/desktop не менялись; новые файлы только эта документация и
`tests/gui/youtube_embed_reference_native.py`.

## Гипотеза

ChallengeRequired анонимного extractor не доказывает, что официальный embedded
player не играет. Зонд проверяет supported IFrame Player API в обычном CEF
NexusBro: реальный enclosing fixture-origin на loopback, из которого iframe
грузит официальный `https://www.youtube.com/embed/aqz-KE-bpKQ?enablejsapi=1&
origin=<fixture>`. Прямой навигации на `/embed/` и подделки Referer нет
(актуальное ограничение — error 153 без enclosing origin).

## Что делает скрипт

- Только `GITHUB_ACTIONS=true` + Linux, флаги `--exe --cef --out`; видео фиксировано.
- Fixture: собственный loopback HTTP-origin отдаёт один чистый HTML: официальный
  iframe (единственный внешний origin), одна контрастная кнопка Play, inline JS.
  Никаких remote script (iframe_api НЕ требуется — управление идёт документированным
  postMessage-протоколом: `listening` + `command/playVideo`).
- Один физический trusted click по кнопке (click_control), обработчик кнопки
  (isTrusted) шлёт официальную команду playVideo. Никакого autoplay/JS-кликов из
  тест-клиента, никакого native IPC из фрейма.
- Ждётся реальная отрисовка кнопки в owned-viewport, собирается fixture-receipt.
- Граница данных: из страницы наружу проходят ТОЛЬКО allowlisted целые —
  состояния {-1,0,1,2,3,5}, ошибки {2,5,100,101,150,153} и продвигающийся
  currentTime (≥8 с). Любой другой код → blocked. Сырые URL/заголовки/ошибки/
  cookies не сохраняются и не публикуются.
- Процесс-ownership, normal exit receipt и cleanup не ослаблены; бюджеты:
  работа ≤180 с, cleanup ≤90 с (под будущий CI).
- Отчёт всегда оставляет `full_playback_acceptance=false`, `paired=false` —
  API-часы и состояния — это feasibility, не paired-decoder приёмка; официальный
  API не доказывает отсутствие рекламы.

## Исходы

- `completed=true` + `playing_observed=true`: официальный embed стартует в
  обычном CEF — основание для продуктового решения (отдельный честный режим с
  ограничениями качества/рекламы/фона/загрузок, без ослабления nb-lite CSP).
- Любой `blocked_reason` (embed_refused 101/150, embed_origin_invalid 153,
  unexpected code, playing_not_observed): честный blocked, playback не заявлен.

## Следующий шаг

ROOT запускает зонд на существующем кандидате (FULL run 45592261113,
source 1f8ece5) вручную/через CI-шаг по его выбору; этот прогон сейчас не
запускался по ограничению задания.
