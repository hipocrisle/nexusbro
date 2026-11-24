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
