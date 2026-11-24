# YouTube helper distribution inventory

Pinned input: [YOUTUBE-COMPONENTS.json](YOUTUBE-COMPONENTS.json). Only the native
Windows x64 and Linux x64 isolated GitHub runners package executable assets.
The packaging script verifies SHA256 before opening the Deno ZIP and copies
only its exact `deno.exe`/`deno` entry. It never uses extractall or runs a helper
before verification. Version invocations occur offline with a restricted env.

yt-dlp2026.11.23 source is Unlicense; the selected official PyInstaller assets
are combined GPLv3-or-later distributions. The exact upstream tag's
THIRD_PARTY_LICENSES.txt includes the bundled-component texts (Python, EJS and
other dependencies); that pinned245018byte notice and the source LICENSE are
copied into every output package. Do not label the executable Unlicense-only.
Matching source: https://github.com/yt-dlp/yt-dlp/tree/2026.11.23 . Release source
and dependency source availability does not by itself complete NexusBro's own
corresponding-source distribution obligations; root owns release review.

Deno2.9.8 root LICENSE.md is MIT. Its Cargo.lock selects v8 crate150.4.0
(crates.io checksum42a978ff11f15b24e5c05a7123cf2b68f41e763546699781a924ef4e2cf43a49).
The corresponding official rusty_v8 tag points to denoland/v8 gitlink
ac1e23989121713ca642f6650b34deff7b686896. The script copies pinned Deno MIT,
rusty_v8 MIT, exact native V8 LICENSE and the pinned Deno Cargo.lock inventory.
The V8 LICENSE explicitly distinguishes externally maintained libraries; these
root notices are **not** a fabricated complete transitive dependency aggregate.
Deno's exact parser source `libs/cli_parser/src/flags.rs` exposed no license
subcommand during this audit; no guessed `deno license` invocation is used.

**Publication review remains open:** complete the exact Deno/V8 external and
other runtime dependency notices/source obligations before publishing. Generated
components.json states `redistribution_review=pending_deno_transitive_notice_inventory`.
Packaging a SHA-verified candidate is not a license clearance or a public
release. No owner approval flag, silent blanket MIT claim or runtime downloader
is introduced by this package.

Generated files under desktop/src-tauri/binaries/youtube-light:

- yt-dlp.exe + deno.exe on Windows; yt-dlp + deno on Linux;
- components.json with actual per-executable SHA256, verified asset/archive
  SHA256, versions, exact upstream/source URLs and copied notice hashes;
- licenses/ containing five pinned notice files plus DENO-Cargo.lock.

Trust pins were obtained from official GitHub release/contents metadata and
small license/source texts. No executable asset was downloaded or run on DE-1.
The script's --self-test uses only tiny local ZIPs and directory replacement.
Workflow invocation, build.rs embedding, runtime verification and installer
resource inclusion remain root's integration responsibilities.

## FFmpeg local high-quality mux component

The manifest now pins dated FFmpeg-Builds release
`autobuild-2026-11-24-12-41`, not floating latest. Both native x64 shared builds
identify as `n8.1.3-41-ga8029b6528`; matching FFmpeg source revision is
`a8029b6528fecbcb5d99b331cfc76e6467b9bc77`, recipe revision
`b9ebe81995b58cd4e3177a6c0ad5dc2392f7f7fd`.

The recipe's lgpl defaults enable **version3**, so these are LGPLv3 builds,
not an assumed LGPL2.1 bundle. Packaging copies the archive LICENSE.txt plus
pinned exact-source COPYING.LGPLv3, COPYING.GPLv3 and LICENSE.md. The recipe
excludes x264/x265 for lgpl; actual verified executable -buildconf is also
required to enable shared/version3 and exclude enable-gpl, enable-nonfree,
libx264/libx265/libfdk-aac. Its -L must report Lesser GPL version3. Version,
buildconf and license receipts are retained and hashed; ffprobe must also start
with exactly the pinned version.

Output `ffmpeg/bin` contains only ffmpeg/ffprobe and the Windows DLL closure;
Linux `ffmpeg/lib` contains the archive's shared SO closure. Each included file
has its own SHA256 in components.ffmpeg.files. The SHA-verified archive is parsed
without extractall: at most4096 entries,512MiB declared/materialized contents,
256 selected files,256MiB/file. Archive escapes, duplicate paths, ZIP symlinks,
hardlinks and special files are rejected. Linux library symlinks are resolved
only through the closed in-archive lib graph, with bounded depth/cycle checks,
and materialized as regular byte-identical files. No actual symlinks are
installed. Binary metadata checks run only after archive verification on native
isolated GitHub runners, with a restricted environment and private bundled
library search path. No external media URLs or application credentials are used.

**Limits:** native startup verifies this bundled DLL/SO set works with the CI
OS; it does not certify the minimum supported OS or complete all transitive
library/source redistribution obligations. components.ffmpeg records a pending
external-library notice/matching-source preservation review. Keep the exact
source archive/build recipe, FFmpeg notices and additional dependency notices
with the distributed component; do not claim that every dependency is LGPLv3.
The script does not silently invent a license clearance.

Upstream retains daily assets only14days. Preserve the verified input archives,
generated candidate component directory, source manifest and matching sources
in existing private CI artifacts before expiration. This script deliberately
creates **no** public release, tag, updater publication or deployment. The root
workflow must upload `.dev-check/youtube-component-upstream/` as a private GHA
artifact: the script preserves the verified input archive there and records
its relative path in components.ffmpeg.ci_archive_artifact. This directory is
outside installer resources; temporary packaging staging is removed.

This component is intended for **local files only** after native downloads of
both selected high-quality streams through the actual route. Root invocation
must repeat `-protocol_whitelist file,pipe` before each input, use fixed demuxer
and mapping, `-nostdin`, stream-copy and bounded progress/errors. Shared builds
are network-capable in general; packaging metadata checks do not demonstrate
that future application invocations are restricted. No360p fallback or quality
claim is implied by component packaging.
