# Browser task ownership verification

On a GitHub runner: `cargo test --manifest-path tests/browser_tasks_harness/Cargo.toml`.

Tests run production ownership/aggregation logic against unrelated instances,
PID reuse, parent cycles, missing ancestors, nested roots, unknown memory and
stable creation identities. They do not prove native Windows/Linux API behavior.

Native acceptance must compare the running instance against OS measurements,
open/close a Super, include an unrelated parallel instance and verify it is
excluded, and exercise process exit between snapshots. CPU rates must use
per-process identity/time deltas rather than subtracting aggregate group CPU
counters: exited/replaced processes otherwise produce misleading rates.
