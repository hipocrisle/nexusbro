# Лёгкий YouTube: контракт следующего общего релиза

Статус: реализация начата, готовность не подтверждена. Владелец требует один релиз вместе с ресурсным этапом; отдельный sleep-only релиз запрещён.

## Поведение

Действие «Открыть в лёгком режиме» для одного видео YouTube открывает встроенный плеер вместо тяжёлой страницы. Обычный режим остаётся доступен. Первоначально публичные видео без переноса Google cookies. Ошибки ограничений/доступа показывать честно; не обещать воспроизведение любого видео. Плейлисты не запускать автоматически.

Первый пакет: качество, аудио, субтитры, позиция просмотра, полноэкранный режим, непрерывное фоновое воспроизведение; SponsorBlock, Return YouTube Dislike; скачивание видео/аудио с прогрессом и отменой. Никакого отдельного сервера, ручной установки компонентов владельцем или скрытого переключения в обычный режим.

## Сеть и безопасность

Единственный источник полномочий — существующая вкладка и её подтверждённый эффективный NativeRoute. Запрос из произвольной страницы не может выбрать Super, путь, executable или локальный файл. Все запросы извлечения, проигрывания, перемотки, субтитров, картинок, внешних API и скачивания используют родительский loopback transport. Ошибка/утрата маршрута останавливает операцию; Direct fallback запрещён даже для метаданных. Смена пути аннулирует старый запрос/плеер до запуска нового. Устаревшие generation/operation результаты отбрасываются.

Не экспортировать cookies/пароли, не загружать пользовательские конфигурации или плагины внешних утилит. URL допускаются только распознанные YouTube video ID, без произвольных URL/playlist/shell/options. Подписанные stream URL и конфиденциальные заголовки не логировать и не хранить на диске. Ограничить stdout/stderr, время выполнения, размер JSON, число процессов и повторов. Sidecar только из проверенного пакета приложения; обновления проверять, не исполнять случайный PATH.

## Ресурсы и жизненный цикл

Активное воспроизведение защищает вкладку и Super от автоматического сна; сворачивание и другая выбранная вкладка не ставят паузу. Закрытие вкладки, блокировка браузера, смена пути и завершение приложения согласованно отменяют операции. Родитель владеет полным деревом sidecar-процессов, учитывает их в диспетчере и ресурсном эксперименте. Ограничить буферы и завершать помощники после извлечения; не запускать экземпляр при каждой перерисовке UI.

## Границы пакетов

- Root: этот контракт, lib.rs/IPC, привязка к вкладке/маршруту/блокировке, общий релиз и workflow.
- Модель данных: новые youtube_light.rs и youtube_light/*; строгий video ID, bounded metadata, quality/session/error types, без регистрации команд в lib.rs до интеграции.
- Плеер/извлечение: отдельные новые модули после проверки официальных интерфейсов и лицензий. Решение libmpv против существующего CEF video фиксировать до подключения бинарных зависимостей.
- UI: новые YouTubeLight.tsx/css и типы; App/ObychnyeVkladki интегрирует root после согласования интерфейса.
- QA: новые tests/gui/youtube_light*; общий workflow меняет root. Не изменять существующие hibernation fixtures параллельно.

## Приёмка

Настоящий Windows/Linux бинарник: публичное видео с видео+аудио; качество/субтитры/seek; фон и сворачивание; route-isolation с отказом прокси без прямых утечек; отсутствие cookies export; SponsorBlock/RYD честно отражают недоступность; download/cancel/error; закрытие и блокировка без оставшихся процессов. Фикстуры отдельно от реального YouTube playback. Парный замер SAME VM/video/quality/hardware decoding со всеми процессами; без рекламных обещаний ускорения. Финальная общая сборка проходит двухчасовой ресурсный сценарий и штатную проверку автообновления. До этого не публиковать.

## Согласованный пакет загрузок без потери выбранного качества

Независимый исполнитель owns новые `youtube_light_download.rs`,
`youtube_light_mux.rs`; `youtube_light_components.rs` разрешён только для
добавления `load_mux`/закрытого `MuxComponents`, существующий extractor API
сохраняется. Root owns IPC, trusted native chooser, CEF job ownership и sleep
protection, новые модули не регистрируют себя и не меняют JS/workflow/Cargo.

- First scope: complete direct WebM VP9/Opus. `Selection<'a>::Audio`,
  `Selection::Video{video,audio}`, `Selection::Muxed` валидируют фактические
  admitted kind/codec/container. Selection заимствует `&MediaFormat` из owned
  worker `Arc<ExtractedVideo>` на весь await, без клонирования signed URL.
  Нет silent360p fallback, перекодирования или
  переименования WebM в Opus/MP3. Неизвестный формат честно отклоняется.
- `Target::new(trusted_directory,filename)` — parent-only, без web deserializer;
  local absolute directory, no symlinks/parent traversal, безопасное WebM имя.
  Save path приходит только после native chooser. Staging создаётся случайным
  отдельным каталогом внутри назначения (0700/isolated ACL), файлы только
  video.webm/audio.webm/output.webm; ни signed URLs, ни JSON на диске.
- `Limits{max_total_bytes,min_free_bytes}`: default8GiB, hard32GiB,
  reserve>=512MiB. Native free-space query: Linux statvfs, Windows
  GetDiskFreeSpaceExW. До записи резервируется место под download+local mux;
  проверки продолжаются во время работы, без увеличения лимитов при ошибке.
- `async download::run(selection,route,target,optional_mux,limits,cancel,progress)`
  запускается ТОЛЬКО на owned CEF background job. До двух concurrent media
  streams, explicit admitted loopback proxy/noenv/nocookiejar/noredirect/retry0,
  identity transfer encoding, 206 Content-Range/length/total/ETag consistency.
  Requests bounded45s, весь download2h; callback progress<=4Hz только
  phase/bytes/total/mux_bytes, false отменяет. Hash/stdio/fsync не на CEF UI.
- `load_mux(directory)` проверяет ВСЕ файлы closure по compiled sealed manifest,
  не runtime components.json; no symlinks/extra files в DLL/SO roots. MuxComponents
  не сериализуется, Debug не раскрывает пути. Recheck до spawn, env_clear,
  private Windows bin PATH или Linux LD_LIBRARY_PATH; никаких fallback PATH.
- `mux::mux` принимает только local regular files в одном protected staging,
  repeats protocol_whitelist=file,pipe before EACH input, fixed Matroska demux,
  explicit maps, `-c copy -f webm -n -nostdin`, no shell/network URLs/options.
  Progress lines<=1024, stdout/stderr<=4MiB, timeout30min, direct-child
  cancellation kill+wait<=5s. Если bundled ffprobe присутствует, отдельный
  bounded local-only probe20s подтверждает ровно VP9 video+Opus audio. Отсутствие
  ffprobe допустимо только если он отсутствует в sealed package.
- Final file hash/size/EBML header проверяются с cooperative cancellation;
  publish — hard_link нового staged файла в final name, no overwrite, затем
  bounded own-files cleanup. Unsupported hardlinks/FS — явный отказ, не опасный
  rename fallback. POSIX directory fsync после publish; Windows file sync до
  atomic link. Existing file никогда не удаляется/не перезаписывается.
- `DownloadError::CleanupUnknown` / `MuxError::CleanupUnknown` ОБЯЗАТЕЛЬНО
  защёлкивают root job protection до подтверждённого owner process exit;
  staged files сохраняются при неизвестном завершении helper. Drop/panic
  будущего — НЕ proof cleanup; coordinator должен await cancel и обрабатывать
  panic как unknown. Обычная ошибка/отмена удаляет только известные own files;
  неизвестное содержимое не удаляется рекурсивно.

Это контракт реализации и границы API, не утверждение нативной готовности.
Нужны CI tests+Windows/Linux actual highquality download/mux/cancel, corrupted
components/route failure/free disk/no overwrite/child exit и отсутствие сетевого
доступа из локального FFmpeg invocation. Full external dependency/source license
inventory остаётся отдельной незавершённой задачей release review.

## Протокол парного ресурсного испытания

Один установленный бинарник и один компьютер: четыре последовательных прогона
«лёгкий → обычный → обычный → лёгкий». Для каждого прогона создаётся свежий
изолированный профиль с одинаковым маршрутом. Video ID, участок воспроизведения,
разрешение, частота кадров, кодек, громкость и фактический режим декодирования
должны совпадать. Одного совпадения настройки аппаратного ускорения недостаточно.

После 30 секунд прогрева — минимум 90 секунд измерений с интервалом 5 секунд.
Учитываются родитель и все принадлежащие ему процессы с проверкой PID и времени
создания, включая помощников извлечения. Отдельно показываются затраты запуска
и извлечения; их нельзя скрывать результатом устойчивого воспроизведения.
CPU указывается в процентах одного ядра, private/USS и сумма RSS — раздельно:
сумма RSS повторно учитывает общие страницы. Публикуются все четыре прогона,
медианы, пиковая память и доля пропущенных кадров, а не лучший результат.

Между прогонами подтверждается завершение всего принадлежащего приложению дерева.
Реклама, остановка буферизации, неизвестный кодек/декодер, несовпадающее качество,
неполное наблюдение процессов или запрос подтверждения YouTube делают сравнение
непринятым. Контролируемый WebM проверяет функции проигрывателя отдельно и не
заменяет этот эксперимент с настоящим YouTube.
