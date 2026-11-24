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

## Source-built minimal FFmpeg muxer

Desktop packaging now requires `--muxer-package <trusted-same-run-artifact/package>`.
There is no fallback to the former BtbN binary downloads. Extractor-only probes
remain separate and do not acquire a muxer. The caller must first authenticate
same-repository immutable GitHub artifact provenance; runtime JSON alone is not
a binary trust root.

Reviewed FFmpeg source: `a8029b6528fecbcb5d99b331cfc76e6467b9bc77`;
original codeload archive SHA256:
`ce3fa044e0dd97ac7a2e7af710056daa850464ec11bdf804a8033c5eae1ec706`.
The source-built shared recipe disables autodetection, everything and networking,
enables only file/pipe protocols, Matroska input, WebM output, VP9/Opus parsers
and the required `vp9_superframe` bitstream filter. It enables no external codec
libraries, GPL, nonfree or version3. Actual `-L` must identify LGPL2.1-or-later.
Other COPYING files shipped from upstream describe alternative configurations,
not extra permissions or the selected build license.

Before copying or executing anything, the packager checks the independent source
pin, platform, exact recipe flags, native stream-copy/protocol-refusal evidence,
and the closed 14-file hash inventory. It rejects missing, extra, tampered and
symlinked files. Only then do native offline ffmpeg/ffprobe version, buildconf
and license checks run with private bundled library search paths. SourceRecord
contains all installed hashes including verification receipts and ffprobe.

The verified Windows recipe has seven internal FFmpeg DLLs; its actual dumpbin
inventory lists only these and Windows KERNEL32, bcrypt, PSAPI and SHELL32.
Linux packages seven regular SONAME files. CI startup is not a claim about all
supported OS versions or redistribution clearance.

Version metadata: exact upstream `ffbuild/version.sh` gives a source `VERSION`
file precedence over enclosing Git discovery. After verifying/extracting the
unchanged original archive, the recipe writes `VERSION=git-a8029b6528fe`.
This deterministic recipe-only metadata override is recorded in source-receipt;
it is not an invented upstream semantic release. The packager uses the actual
native version receipt hash and matching ffprobe token. Previous experimental
builds exposed the enclosing NexusBro Git hash; that is not an FFmpeg version.

Release workflow must preserve and distribute the exact original source archive,
the actual corresponding build recipe (including VERSION override), config/log
and dependency inventories and all five FFmpeg license files. Native Linux and
Windows evidence from run45459592456 proves stream-copy VP9+Opus and compiled-out
HTTP for that candidate. It does not clear Deno/yt-dlp transitive notices or
publish a release. Root owns final matching-source/notice artifact preservation.
No user media URLs or credentials are used by packaging verification.

## Collected transitive notices in desktop bundles

Full desktop packaging requires `--notices-inventory DIR` from the same run's C3
`youtube-source-inventory` artifact. The extractor-only diagnostic keeps its
existing interface and does not require this argument. No source archive from
that inventory is opened or imported into the product.

The offline packager verifies index schema, every indexed notice's size/SHA256,
the exact closed notice subtree, and the four reviewed source-archive commit/SHA
records against `docs/YOUTUBE-SOURCE-PINS.json`. It validates the complete typed
pending-gap list and its equality in index and gaps documents. Symlinks/reparse
points, traversal/case collisions, unknown or missing notice files, binary
payloads and hash mismatches refuse packaging. Archive records are provenance
checks only: archive contents are not re-read here; same-run artifact identity is
provided by the caller workflow.

Only indexed `notices/**` plain bytes are copied to
`licenses/transitive/**`, removing the leading `notices/` component. The exact
source index and gap document become `licenses/transitive/source-index.json` and
`pending-gaps.json`. Limits: 16MiB notice bytes, 8MiB index, 1MiB gaps, 4096 notice
files. The index retains per-file hashes and original source URLs; the sealed
`components.json` has only their document hashes, counts, mapping and review
state. Its existing 128KiB hard cap is preserved and enforced by the packager.
The roughly329MB corresponding-source inventory is retained separately by CI,
not copied into the browser bundle.

C3 run45470494915 collected1916 notice/metadata files (10,249,202bytes), including
56 empty test fixtures and source-related metadata. These are preserved for
inventory fidelity; neither empty files, SPDX identifiers nor metadata are
represented as licensing grants. `product_notices_cleared` remains false.
Unknown coverage across all Cargo.lock entries is not automatically a missing
requirement for the distributed Windows/Linux runtime: that inventory includes
other platforms, development tools, tests and compile-time macros. Explicit
required missing notices, optional superset entries and unknown linkage remain
distinct review categories.

The exact pinned yt-dlp aggregate THIRD_PARTY_LICENSES.txt is already collected
and shipped, including its Python/native dependency notices. A remaining gap in
matching an exact executable's bundled dependency versions to source provenance
is not a claim that all those notices are missing. Root's release review still
owns the actual distribution/matching-source decision; this packaging step does
not issue blanket clearance or publish a release.
