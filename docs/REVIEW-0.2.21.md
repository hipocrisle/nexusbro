# NexusBro 0.2.21 — кандидат на публикацию

Публичная версия пока 0.2.20. Исходник приложения: `1e0a5d8`, пакеты собраны в [44771343618](https://github.com/hipocrisle/nexusbro/actions/runs/44771343618). Последующие изменения тестов не меняют `desktop`; runtime workflow проверяет это сравнением Git. Измеренный Windows installer: SHA256 `6a6f0edbbd9dcef6ad60dc8e34fe94e5371cc4834a40c45b39727198be5142bd`.

## Поведение

- Обычная вкладка выбирает путь Super, Direct или сохранённое подключение по ID. Внутри Super один effective путь разделяет cookies/storage; разные пути и разные Super изолированы. Авторизация между путями не копируется. Недоступный или удалённый путь не заменяется Direct.
- Облако запускает прежний sync run с сохранённым выбором без открытия настроек. Зелёный статус требует настоящего ACK; локальные и приватные Super не получают облачных значков, включая свёрнутую панель.
- Ручные unload/keep и их диалог удалены. Автоматическая политика по умолчанию — 30 минут; восстановленные невыбранные страницы lazy. Медиа, ввод, загрузки и unsafe/unknown страницы защищаются автоматически. Старое `memory_keep` не создаёт скрытого исключения.

## Изоляция и сохранение

Строгий intent/IPC, immutable per-engine descriptors, singleflight транспорта и ACK с window/engine/tab/browser/attempt identity препятствуют применению устаревшей операции. Транспорт и RequestContext ограничены; worker/download leases сохраняются до завершения движка. Контекст допускает запросы только после принятия обязательных proxy/WebRTC/prediction preferences.

[Точный CEF g8ea0c73](https://github.com/chromiumembedded/cef/blob/8ea0c73b5dbed1389c8dafb911a98c41246bb8cb/libcef/browser/chrome/chrome_browser_context.cc#L86) превращает вложенный cache path в OffTheRecord. Это было обнаружено реальной проверкой дисковых cookies, хотя RAM auth работала. Итоговая схема — `root/route-<64hex>`, непосредственный ребёнок Super root. Прежний `Default` не перемещается. Общий helper проверяет canonical parent/leaf и отвергает aliases; историческая очистка перечисляет только принадлежащие Super профили. Никакого копирования auth или миграции вложенных временных каталогов нет.

Пустая вкладка наблюдается только после регистрации раннего скрипта и свежего history-neutral перехода. Старый initial document не захватывается повторно. Подтверждённая native сетевая ошибка явно завершает loading по browser ID и navigation epoch; это не выдаёт разрешения на выгрузку, не принимает неизвестный GET и не ослабляет POST/edit/media guards.

## Выполненные проверки

| Проверка | Результат | Доказательство |
|---|---|---|
| Security: vault, реальные route/guard/path modules, sync-server | PASS | [44771343618](https://github.com/hipocrisle/nexusbro/actions/runs/44771343618) |
| Linux: 18 маршрутов, HTTP/HTTPS CONNECT, storage/auth, параллельность, STUN, сбой транспорта, рестарты и удалённый ID | PASS | [linux-routes.json](verification/0.2.21/linux-routes.json) |
| Linux: encrypted vault, CEF credentials, cloud click/ошибки/recovery, два устройства | PASS | [linux-security.json](verification/0.2.21/linux-security.json) |
| Linux: окна и базовый runtime | PASS | [linux-runtime.json](verification/0.2.21/linux-runtime.json) |
| Windows: физический интерфейс | PASS | [windows-physical.json](verification/0.2.21/windows-physical.json) |
| Windows: все 18 маршрутов, включая строгие disk cookies после остановки процессов | PASS | [windows-routes.json](verification/0.2.21/windows-routes.json) |
| Windows: настоящие visibility/rAF, восстановленный ввод и отрисовка | PASS | [windows-background.json](verification/0.2.21/windows-background.json) |
| Windows: автоматическая выгрузка и media clocks | PASS | [44778487077](https://github.com/hipocrisle/nexusbro/actions/runs/44778487077), [raw](verification/0.2.21/windows-memory.json), [audit](verification/0.2.21/windows-memory-audit.json) |
| Парное измерение точного финального installer против публичной 0.2.20 | PASS | [44774520973](https://github.com/hipocrisle/nexusbro/actions/runs/44774520973), [comparison](verification/0.2.21/performance-comparison.json) |
| Публикация и публичные подписи трёх пакетов | Не выполнены | Обновление ещё не объявлено готовым |

Windows runtime [44774513843](https://github.com/hipocrisle/nexusbro/actions/runs/44774513843) принял physical/routes/background, но AUTO-тест остановился на повторном чтении изменяемого JSON: между чтениями строка становилась временно недоступной. Test-only `fa5cf49` использует один снимок на predicate/iteration; отсутствие строки не считается успехом. После восстановления строгого read-only `normal_policy` и проверки единого снимка AUTO/media принят настоящим прогоном `44778487077`. Промежуточный запуск остановился на начальном экране проверки защиты до любых AUTO-действий; повтор на свежей VM без изменения приложения прошёл. Точная причина той задержки старта не установлена.

Другие исправления теста не ослабляют проверки: живой Windows Cookies DB может быть заблокирован, поэтому фактические SQLite persistent positives обязательны после остановки собственных процессов до рестарта. Inspector использует [документированный Microsoft механизм для elevated host](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/security#for-an-elevated-host-app-use-appropriate-override-flags): exact-executable policy только на disposable runner с восстановлением прежнего значения. Настройки приложения и DE1 этим не меняются.

## Ресурсы

Одна Windows VM, обе версии, 8 Super / 1000 сохранённых / 24 открытые страницы; одинаковый loopback alias и подтверждённый HTTP proxy path. 72/72 phase/page checks, нулевой скрытый rAF, настоящие trusted input и paint после возврата. Reference installer SHA256 `18952dfcef0b6c4364d6a35e424cc1e1e16bc45699d1040f4b1a8e601a02d682` соответствует публичной 0.2.20.

| Фаза | CPU машины 0.2.20 → 0.2.21 | Private commit, MiB 0.2.20 → 0.2.21 |
|---|---:|---:|
| Активная | 2,835% → 2,301% | 1351,1 → 1309,0 |
| Перекрытая | 1,767% → 2,045% | 1326,9 → 1316,5 |
| Свёрнутая | 1,483% → 1,128% | 1324,9 → 1309,9 |
| Восстановленная | 0,922% → 1,025% | 1303,6 → 1293,6 |

Это один синтетический парный замер, не обещание процентов для любых сайтов. Прежний active CPU uplift в промежуточном кандидате не повторился. Working set немного выше; сумма working set процессов содержит shared pages и не равна уникальной RAM. Постоянное количество процессов одинаково. Сырые данные: [reference](verification/0.2.21/performance-reference.json), [candidate](verification/0.2.21/performance-candidate.json).

## Практические границы

Восемь effective путей на движок; глобальный лимит override Xray не включает старые базовые туннели. Whole-Super hibernation не внедрена. Автономный старт Service Worker до admission не был воспроизведён; готовый worker и его регистрация после рестарта проверены настоящими B-only запросами. Полная DNS-анонимность не заявляется: explicit TURN hostname lookup остаётся upstream ограничением. Нормальный media test не заменяет проверку сетевого звонка или capture. Контракты и primary sources: [TAB-ROUTES-PLAN.md](TAB-ROUTES-PLAN.md).

## Автоматическая память: фактическое доказательство

Окно действительно минимизировано 15,172 секунды. Native журнал подтверждает автоматический запрос, две проверки с freeze и graceful close только вкладки 9/browser14 с сохранением строки. Private commit: 882139136 → 639361024 байт, освобождено 231,53 MiB. Ни ручного unload, ни ручного keep, ни включения политики через Save в тесте нет.

HTML audio clock продвинулся на 53,415 секунды при wall53,391; WebAudio — на 53,031 при wall53,032, состояние running. Paused HTML audio сохранило currentTime0,023598. Все четыре media/API страницы получили несколько native automatic refusal, сохранились RTC new/stable, pinned/devtools/loading/download/private/draft. После wake осталась та же строка id9, cookie/localStorage preserved и scroll600. Проверка capture и пользовательского закрытия во время sleep отдельно не выполнялась; они не объявляются принятыми этим тестом.
