# Закрытие строгого GTK-кейса, source1f8ece5

Обычный runtime run45651884307 целиком SUCCESS. Результат маршрутов:
`docs/verification/0.2.27/linux-strict-route-summary.json`:22checks,
diagnostic_only=false, mapped+owned+override_redirect tooltip реально
перекрывает кнопку, input_rectangles=0, один физический trusted click
дошёл до DOM и открыл диалог. App source не менялся, LD_PRELOAD не применялся.

Подготовка фикстуры: исходная вкладка с реальным многострочным title;
наведение на её подпись исключает собственный title крестика. Нейтральный
указатель находится над адресной строкой без title; после наблюдённого выхода
выдерживается600ms и проверяется отсутствие прежнего popup. Затем ровно
одно наведение, исходный строгий нативный контракт не ослаблен.

GTK3.24.41 скрывает прежний tooltip с BROWSE_DISABLE_TIMEOUT=500ms,
прежний trace начинал новое наведение через18ms после unmap. Это обоснование
устранения состояния фикстуры, не универсальное доказательство единственной
причины всех старых отказов. Источник:
https://raw.githubusercontent.com/GNOME/gtk/3.24.41/gtk/gtktooltip.c

Публикация существующих пакетов готовится CI-токеном после нативных проверок
и проверки настоящего двухчасового отчёта нужного source/build. Тег создаётся
неизменяемым на source сборки, выпуск сначала draft. Обычный push v* человеком
не нужен: он запустил бы повторную сборку. GitHub документирует подавление
рекурсивных push/tag workflow для операций собственного GITHUB_TOKEN:
https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow
