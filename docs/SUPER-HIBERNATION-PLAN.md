# Whole-Super hibernation — deferred architecture contract

Per-tab route priority and automatic-only UX are the current owner priorities. Do not add production hibernation by reusing the existing stop path.

Read-only audit of the published engine found:

- Every Super always protects its own selected ordinary page and updates its idle timestamp, even when the Super is hidden. All-asleep is not currently reachable.
- Session-cookie persistence is not enabled; live-process cookie wake tests do not prove cookie survival across engine exit.
- Current stop kills the child and clears parent process/channel/window ownership before wait completes. A hibernation wake could start a second profile writer.
- Windows and Linux engine loops have no graceful quit/shutdown/flush protocol.
- Sleeping-tab scroll restoration is held only in an engine-local map and is lost on engine exit.
- Additional app windows may use the same Super. Main-window selection and temporary hiding for dialogs are not an idle authorization signal.
- Windows creates a per-launch Job handle retained until app exit; hibernation needs owned handles and proven process-tree exit.

A future implementation requires parent-owned Running/Preparing/Stopping/Sleeping/Starting + engine generation/profile lease; activity of all windows; authenticated operation ACKs; final local session/scroll snapshot; real cookie/storage flush and CEF shutdown/OS exit before releasing routes or waking. Timeout is not permission to force-kill. No whole-profile merge or pool in this package.

First native Windows diagnostic: isolated data and fixture with persistent + HttpOnly session cookies, localStorage and scroll; server validates actual Cookie headers. Existing save-close/reopen is a force-stop baseline, not graceful success. Compare a separate opt-in graceful prototype, test repeated wake/rapid clicks/one writer and live media/input/download/private/extra-window refusal. Do not enable global session-cookie persistence as a silent privacy workaround.
