# Независимая диагностика маршрутов

`linux_runtime_build={"run":"<producer_run>","core_only":true}` допускается только для диагностики уже собранного
Linux-бинарника через существующий строковый параметр `linux_runtime_build` (без добавления 26-го dispatch input). Код приложения должен совпадать с
producer. Новая сборка не нужна.

Этот режим проверяет настоящие UI-клики, native ACK, HTTP/HTTPS/SOCKS, раздельные
профили, сохранение после рестарта и отказ выбранного подключения без Direct.
Отдельный сценарий естественного GTK tooltip overlap не запускается. В отчёте
обязательны `diagnostic_only=true`, `tooltip_overlap_required=false` и
`coverage.native_tooltip_overlap=not_requested_diagnostic_only`.

Основная FULL-проверка и обычный Linux runtime сохраняют обязательный GTK-сценарий
с фактическим overlap, пустой input shape и единственным trusted click. Они не
заменяются этим диагностическим прогоном. Publisher запрещён при
`linux_runtime_build={"run":"<producer_run>","core_only":true}`; routes-шаг дополнительно требует пустой publish_tag.
Успех диагностики не означает общей приёмки, исправления GTK-сценария, успешного
YouTube, ресурсного результата или готовности обновления.
