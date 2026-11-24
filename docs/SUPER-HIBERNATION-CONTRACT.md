# Автоматическое засыпание Super и длительная проверка ресурсов

Владелец утвердил оба этапа 24.11.2026 после выпуска 0.2.26. Делаем автоматически, без ручных команд и новых сложных настроек. Публичная 0.2.26 сохраняется до законченной нативной приёмки и проверки публичного автообновления.

## Обязательные условия

- Цель: завершить процесс действительно неиспользуемой обычной Super, сохранив определения вкладок, выбранную вкладку, маршруты, входы, storage и позиции. Открытие запускает ровно одного владельца профиля и восстанавливает состояние до сетевой навигации.
- Текущий `ostanovit_resursy`/force-kill не является graceful hibernation. Таймаут означает отказ от сна либо удержание lease в Stopping, никогда разрешение второго writer.
- Защищены выбранная Super главного окна, ссылки других окон (первый этап может консервативно исключать любые дополнительные views), private, медиа включая paused/WebAudio/RTC/capture, загрузки, ввод/несохранённые данные, POST/history, DevTools, route changes и неизвестное состояние. Скрытие для диалога не означает idle.
- Существующие AUTO memory native/renderer guards сохраняются. Снятие запрета с выбранной ordinary-вкладки возможно только по подтверждённому parent idle lease, с проверкой текущего поколения и всех views, не глобально.
- Parent owns Running/Preparing/Stopping/Sleeping/Starting, generation, attempt и profile lease. При быстром возвращении во время Closing wake ждёт подтверждённого OS exit. Существующие сохранение/закрытие/редактирование/блокировка согласуются с этим lifecycle.
- Снимок последней session/scroll подтверждён до commit. Persistent cookies и storage реально flush, CEF CloseBrowser(false)/on_before_close/Shutdown завершаются до снятия lease и остановки сетевых ресурсов.
- Измеренная потеря HttpOnly session cookie запрещает простое закрытие/переоткрытие. Рабочее направление для проверки: session cookies передаются только в ограниченную память владельца через authenticated generation/attempt protocol; никакой глобальной настройки persist_session_cookies и plaintext snapshots на диске. Предпочтителен AEAD sealed blob со случайным ephemeral key, переданный stdin/stdout без секретов в argv/логах. При выходе приложения transient session material уничтожается. Точный API фиксируется после аудита CEF.
- Изоляция путей сохраняется: cookie restore по точному native profile/context key, без переноса между путями или Super; удалённые/недоступные пути не получают Direct fallback. Snapshot budget bounded; переполнение означает отказ, не потерю входов.

## Границы первого параллельного аудита (только чтение)

1. Engine/cookies: `cef_dvizhok.rs`, `cef_vkladki.rs`, `cef_route_context.rs`, `cef_memory_renderer.rs`, `sessiya.rs`, фактический установленный crate CEF. Найти точные shutdown/cookie/restore APIs и minimal native prototype. Автор аудита не правит эти файлы.
2. Parent/lease: `lib.rs`, `okna_app.rs`, `okna.rs`, процесс/Job ownership, route parent/network и existing task metrics. Указать точки state machine, wake/close races и process tree exit. Только чтение.
3. QA/soak: существующие native memory/background/performance scripts и workflows; реальные receipts cookies/storage/scroll, один writer, refusal и циклы. Предложить CI workload на два настоящих часа без ускорения времени, небольшие sample JSON/квитанции. Только чтение.

После сведения аудита отдельно фиксируются публичные Rust protocol/types и точные новые файлы каждого исполнителя ДО параллельного написания кода. Центральные файлы интегрирует один владелец, без concurrent edits.

## Приёмка

Реальное Windows и Linux приложение: session HttpOnly+secure/persistent cookies проверяет сервер Cookie headers; localStorage/scroll/tabs/routes сохраняются после исчезновения старого процесса и старта нового; rapid clicks не создают второй writer; active media/input/download/private/extra window защищены. Ошибка подготовки/flush/cookie restore не даёт ложного Sleeping или ранней навигации. Повторные циклы с настоящими PID и OS metrics. Нельзя считать unit/compile достаточными.

Длительный ресурсный прогон — два часа настоящего wall clock на CI: много сохранённых вкладок и Super, ограниченный живой набор, циклы сна/пробуждения, непрерывное медиа, метрики root+owned process tree, count/CPU/private commit/RSS, стабильные фазы и drift. Не выдавать сравнение разных VM за парный benchmark. В отчёте время, workload, сырые небольшие факты и границы вывода.

DE-1: никаких native Rust/CEF desktop-сборок, тяжёлых флотов или долгого браузерного soak. Нативные приложения и длительные испытания только ephemeral GitHub CI. Инфраструктура, backend deploy, env, сервисы, cron и платные API не меняются.
