# Manual tab memory unloading: native acceptance

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
library. Automatic unloading is intentionally off; the test does not enable it.

The beforeunload page is armed by a real trusted mouse click without typing. Two
separate unload attempts must produce actual native beforeunload refusal logs and
a visible refusal, retain the live row, and permit the second attempt. No native
dialog result is mocked. Backend guard refusals must also appear in native engine
logs; a retained row alone is not evidence that the request was correctly refused.

Current explicit coverage gap: a user-close while native sleep is pending. It
remains `not_executed` in JSON. A `completed: true` result covers only the listed
implemented scenarios and must not be presented as evidence for that lifecycle
race; it needs independent acceptance before claiming the full lifecycle contract.

Safety limits: one owned super, eight seeded tabs plus one explicit private tab,
at most 96 processes, 6 GiB summed private commit during measurements and a bounded
overall timeout. Only descendants of the test's root process are killed, with
process-creation-time checks. No production profile, account, proxy or VPN is used.
