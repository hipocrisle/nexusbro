# Measuring browser performance

`tests/gui/performance_windows.py` measures a real packaged Windows application,
including its CEF and WebView2 descendants. It does not build the application or
substitute its IPC/backend. Use an interactive, disposable Windows CI desktop with
Python `psutil`, `pywinauto`, `comtypes` and the standard Tk installation.

```powershell
python tests/gui/performance_windows.py --exe C:\test-app\nexusbro.exe --out .dev-check\performance-baseline
```

The output directory must not exist. The harness seeds a fresh saved browser
session under that directory and disables updater checks for this isolated
profile. The normal software-rendering default is retained. All document URLs
point to the harness's static HTTP server on loopback. No account, production
profile, subscription, VPN or external test website is used.

The default workload is four saved supers with six tabs each. Startup is measured
before any super is selected; no fixture page may load then. Real UI Automation
mouse input selects each super and each tab. HTTP receipts and the application's
saved session prove that all 24 pages actually loaded. A missing control, failed
navigation or sleeping tab fails the run rather than reporting a smaller workload
as completed.

Saved session size can be scaled independently from the number of pages actually
visited. For example, this seeds 1,000 saved tabs while explicitly waking only 24:

```powershell
python tests/gui/performance_windows.py --exe C:\test-app\nexusbro.exe --out .dev-check\performance-1000 --supers 4 --saved-tabs 250 --tabs 6
```

The harness verifies that all unvisited tabs remain deferred and that no HTTP
request was made for them. Each sample and phase reports `tab_counts` (saved,
live, deferred) from persisted session state; this is distinct from renderer
process count, since Chromium can share renderer processes. The default saved
count equals `--tabs`, preserving the original 4 × 6 baseline workload.

Five phases are recorded: cold lazy restore, warmed active, warmed covered by a
separate native window, minimized, and restored. The fixture is deliberately
static, measuring baseline overhead rather than arbitrary site scripts or video.
This does not explain a user's particular website workload by itself.

Add `--visibility-probe` for a separate diagnostic run investigating whether
native child-window/super hiding reaches Chromium's page lifecycle. This mode
reports actual `document.visibilityState`, `document.hidden`, `document.hasFocus()`
and requestAnimationFrame callback counts/rates for each visited page. No DOM
animation is added, but the rAF loop and local POSTs change the workload: **do not
compare this run's CPU directly with the default static baseline**.

Each page sends metadata every two seconds and on `visibilitychange`. Reports
retain client monotonic time, interval duration/frame delta and server receipt
time/phase in `visibility_reports`. A rate interval can straddle a phase change;
use client intervals and the surrounding reports rather than treating the receipt
phase as the entire interval. Missing periodic reports while hidden may itself
reflect timer throttling; no synthetic zero-rate reports are inserted. The rAF
loop stops after 60,000 callbacks per page, POST bodies are limited to 2 KiB, and
at most 20,000 reports are retained. These POSTs are excluded from page-load hit
counts. Every visited page must supply at least one report for probe completion.

`results.json` retains per-second samples and per-process identities, role,
cumulative CPU seconds, working set, and private bytes. Roles distinguish shell,
CEF browser/renderer/GPU/utility and WebView2. CPU **core percent** counts one busy
logical core as 100%; **machine percent** divides this by the recorded logical CPU
count, matching Task Manager's denominator. Both use observed wall-clock intervals.
Private bytes are committed private memory; summed working sets may double-count
shared pages and must not be presented as unique physical RAM consumption.

New processes have an unknown CPU delta for their first interval. Processes that
exit between observations are counted explicitly; their last unobserved CPU work
is absent, so sampled CPU can undercount rapidly exiting processes. PID reuse is
guarded by process creation time. Raw command lines, secrets and account data are
not written into the report.

Safety bounds: at most 12 supers, 12 visited tabs per super, 5,000 saved tabs total,
128 descendants, 8 GiB summed working
set or 60% of initially available memory (whichever is smaller), and a global
timeout checked during UI waits and sampling. Defaults are much smaller. The
harness kills only its recorded process tree at completion/failure. Do not run
this workload on a production VPN transit host.

`measurement_completed: true` means that the requested workload and every phase
were observed. It is **not** an optimization verdict; `optimized` remains false.
Compare baseline and candidate on the same Windows image, CPU allocation,
workload, phase duration, rendering settings and exact packaged binary provenance.
Keep both raw result files and source/build IDs. Establish thresholds only after
the initial baseline, and retain separate input/render/security acceptance tests.

Status: harness prepared; no Windows performance result is claimed by this file.

## Published 0.2.18 baseline, November 19

Real Windows screen workloads completed on the published installer from build44512453342. Raw samples/screenshots remain in each run's `windows-performance-measurement` artifact; compact reports are under `docs/verification/0.2.19/`.

| Measurement run | Supers | Saved / live / deferred | Warm processes | Peak summed working sets | Peak private committed bytes |
|---|---:|---:|---:|---:|---:|
|44532436734|4|24 / 24 / 0|51|2.883 GB|0.984 GB|
|44532556075|4|1000 / 24 / 976|51|2.895 GB|1.007 GB|
|44533148196|8|1000 / 24 / 976|71|3.899 GB|1.389 GB|

All fixture hit sets matched exactly the visited pages; cold startup made no page requests. These independent runs show the process-count cost of extra live Supers and verify deferred restoration at 1000 saved tabs. CPU/memory timing between separate VMs is not a controlled optimization comparison. Sum of working sets can double-count shared pages; Private Bytes is committed private memory, not resident physical RAM. Candidate comparison remains pending.
