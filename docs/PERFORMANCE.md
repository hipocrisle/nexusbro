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
