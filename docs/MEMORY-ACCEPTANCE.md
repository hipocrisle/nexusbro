# Manual and automatic memory unloading: native acceptance

Prepared harness: `tests/gui/memory_windows.py`, using the same extracted Windows
application/CEF paths and Python packages as `performance_windows.py`:

```powershell
python tests/gui/memory_windows.py --exe C:\fixture\nexusbro.exe --cef C:\fixture\cef --out .dev-check\memory-native
```

This is a test specification and executable harness, **not a claim that a Windows
run has passed**. It uses a fresh isolated profile and a local HTTP fixture. Only
saved session JSON is seeded before startup; commands are actual UIA mouse/keyboard
input through the browser's tab menu, warning dialog and download library. No
backend IPC/CDP response, sleep result or memory number is mocked.

The primary page allocates and writes a 192 MiB Uint8Array. Receipts confirm the
actual allocation and the test first establishes real back-navigation history.
The inactive page is unloaded only after a visible warning explicitly describes
reload, unsaved state/history loss and preservation of site data. Cancel must keep
it live. Confirmation must eventually retain exactly one sleeping row, with
history reset. Total process-tree private bytes, measured as the median of five
samples, must decrease by at least 64 MiB after native closure. This is committed
private memory, not summed working sets or a forced working-set trim. Per-process
identities, roles and both memory metrics remain in the JSON artifact.

Selecting the same row must really reload the fixture, preserve cookie and
localStorage markers, retain exactly one row and clear previous back/forward
history. The minimum drop is configurable within bounds, but must not be lowered
merely to make a failed run green.

Native guards cover active, pinned, private, loading, actual unsaved keyboard input
and DevTools tabs, and both
an in-progress and a paused real download. A protected menu may be disabled or an
actual confirmed backend request must visibly refuse while the row stays live.
The download uses an owned loopback stream and is cancelled through the real
library. Automatic unloading remains off during these manual scenarios.

The beforeunload page is armed by a real trusted mouse click without typing. Two
separate unload attempts must produce actual native safety-refusal logs and
a visible refusal, retain the live row, and permit the second attempt. Early
renderer detection is valid evidence; the refusal need not occur specifically in
the later native beforeunload callback. No dialog result is mocked. Backend guard refusals must also appear in native engine
logs; a retained row alone is not evidence that the request was correctly refused.

The manual phase explicitly seeds automatic saving OFF in the isolated profile.
After manual acceptance, the harness enables `Экономить память неактивных вкладок`
through actual settings UI in `Вкладки и поиск`, saves it and verifies the stored
normal policy remains 30 minutes. The owned child receives
`NEXUSBRO_TEST_MEMORY_IDLE_SECONDS=8` alongside `NEXUSBRO_TEST_DATA`; the application
must honor this shortened clock only with both test variables. No production
configuration, persistent environment or user profile is changed.

A separate auto-eligible page allocates and touches the same large heap, without
forms, iframe, media, navigation history or sessionStorage. Physical wheel input
scrolls it before switching away. Without any manual unload command, it must
reach an actual native automatic request, two-phase verification, graceful close,
and a retained sleeping row; native logs and stored state must agree. Summed
process-tree private commit must fall by at least the configured threshold.
Previously tested ordinary pages receive explicit keep preferences through the
real tab menu, so unrelated background closures do not manufacture that decrease.
Selecting that exact row must reload it and restore actual cookies, localStorage
and scroll position (64-pixel tolerance), verified by a trusted page snapshot.
The pre-unload scroll receipt comes directly from the physical wheel's actual
scroll event. No snapshot button is clicked before auto, because the conservative
early hook intentionally protects any trusted pointer/click interaction. The
post-wake snapshot uses a physical click after the automatic scenario is complete.

Media/resource exclusions are genuine runtime fixtures, armed by physical clicks:

- HTMLAudioElement plays a quiet 30-second looping PCM WAV from loopback. Actual
  currentTime progress before/after the automatic idle period is compared with
  elapsed wall time modulo duration; a second physical snapshot also proves the
  playback clock is advancing.
- A second HTMLAudioElement is started and explicitly paused. It remains live
  and paused, with its actual playback position retained.
- A WebAudio oscillator starts in a real running AudioContext. Its currentTime
  delta must continue tracking wall time across the idle period.
- A local RTCPeerConnection is instantiated with no ICE servers and no offer or
  connection. The conservative resource guard must retain it and its real state.

All these rows must remain live **before** inspection clicks, and their original
page URLs must not reload during the idle interval. They use no timer-heartbeat
substitute for audio progress. A keep preference is set through the actual tab
menu and must persist; pinned and private pages also remain retained. Capture
with a fake device/interface is explicitly `not_executed_no_fake_capture_interface`;
no real microphone/camera permission is requested or claimed tested.

Retention alone is insufficient: while the keeper page remains active, every
media/resource fixture must produce an actual native automatic-probe refusal
containing a media/API-specific reason and browser identity. HTML audio requires
media/HTMLMediaElement evidence, WebAudio requires AudioContext or the native
media/connection API tracker, and RTC requires its peer/connection tracker.
Generic trusted-interaction protection does not satisfy this assertion. The
harness waits at most 45 seconds for all four proofs and records the actual
reason/browser IDs in `automatic_media_refusal_proofs`, then still verifies real
audio-clock continuation. Removing media instrumentation while leaving pointer
protection in place must therefore fail this test.

Current explicit coverage gap: a user-close while native sleep is pending. It
remains `not_executed` in JSON. A `completed: true` result covers only the listed
implemented scenarios and must not be presented as evidence for that lifecycle
race; it needs independent acceptance before claiming the full lifecycle contract.

Safety limits: one owned super, fourteen seeded tabs plus one explicit private tab,
at most 96 processes, 6 GiB summed private commit during measurements and a bounded
ten-minute overall timeout. Only descendants of the test's root process are killed, with
process-creation-time checks. No production profile, account, proxy or VPN is used.
