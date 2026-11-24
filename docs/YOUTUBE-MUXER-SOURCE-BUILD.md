# Минимальный собственный muxer: prototype contract

Это отдельный CI prototype, существующие product component pins и packager не меняются.
Источник: FFmpeg commit a8029b6528fecbcb5d99b331cfc76e6467b9bc77, primary GitHub codeload archive.
Архив ограничен64MiB, SHA256 вычисляется ДО разбора. Пустой source_sha256 означает bootstrap receipt ONLY; artifact содержит source, hash и флаг product_trust_approved=false. После review SHA передаётся явно, mismatch failclosed. Commit закреплён в исходнике script, произвольные source URLs невозможны.

Product build: native MSVC+MSYS2 shell на Windows, native GCC на Linux. Только FFmpeg internal shared libraries, Matroska demux, WebM mux, VP9/Opus parsers, stream copy, ffprobe и file/pipe. Нет external codec dependencies, encoding/decoding, network, NASM, autodetect, GPL/nonfree/version3. Shared FFmpeg LGPL-2.1-or-later: архив сохраняет все upstream COPYING и LICENSE.md, source archive и точные recipe/config.log. Это не юридическое заключение о полноте всей поставки: Windows MSVC/UCRT и Linux libc/toolchain остаются отдельными runtime dependencies.

Reusable workflow: caller сначала завершает youtube-media-fixture job, затем uses youtube-muxer.yml с media_artifact (default youtube-controlled-media). Артефакт того же run должен содержать video.webm (VP9), audio.webm (Opus), SHA256SUMS. Проверка сравнивает exact hashes перед запуском. Эти файлы создаются existing CI generator, не product binary. workflow_dispatch без предварительного same-run fixture честно откажет на download, не скачивает произвольные samples.

Native acceptance prototype: оба local files → FFmpeg stream copy с повторённым protocol_whitelist=file,pipe и fixed matroska demux → WebM; ffprobe должен вернуть ровно VP9 video + Opus audio. Fixed HTTP input должен отказать Protocol not found. Наличие streams, buildconf, license, protocols/muxers/demuxers/encoders/decoders receipts сохраняются. Дальнейшая product интеграция требует проверить этот inventory, отсутствие лишних codecs и loader dependencies; один успешный smoke этого не заменяет.

Output layout совместим с будущим sealed loader: package/ffmpeg/bin/{ffmpeg,ffprobe,.dll}, Linux package/ffmpeg/lib/lib*.so.MAJOR, package/ffmpeg/licenses, package/components.json (ffmpeg record, per-file SHA/size, source/build evidence). Linux aliases материализуются только ABI SONAME, без symlink/linker/full-version duplicates. Windows dumpbin /dependents для каждого executable/DLL и Linux ldd сохраняются; private env PATH/LD_LIBRARY_PATH указывает только на package. Неразрешённая DLL приводит к native startup failure; MSVC runtime на runner не является доказательством наличия runtime на чистой машине. Перед product reuse нужен чистый Windows startup receipt либо явный разрешённый redist package/prerequisite.

No publish/release, no existing manifest mutation, no DE-1 builds. GHA artifact retention14days — долгосрочное соответствующее source хранение root должен оформить перед distribution, не считать временный artifact постоянным source offer.

Primary references:
- https://github.com/FFmpeg/FFmpeg/tree/a8029b6528fecbcb5d99b331cfc76e6467b9bc77
- https://github.com/FFmpeg/FFmpeg/blob/a8029b6528fecbcb5d99b331cfc76e6467b9bc77/doc/platform.texi (MSVC+MSYS2 official sequence)
- https://github.com/FFmpeg/FFmpeg/blob/a8029b6528fecbcb5d99b331cfc76e6467b9bc77/configure
