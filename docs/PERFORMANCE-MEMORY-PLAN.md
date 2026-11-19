# Memory scaling: next changes and acceptance gates

This is an implementation plan, not a shipped feature. The initial performance candidate changes cosmetic scanning and Windows command scheduling; it does not yet discard visited pages or merge existing profiles.

## Evidence

The published 0.2.18 Windows baseline loaded exactly 24 static pages:

- Four live Supers: 51 processes; approximately 0.984 GB peak private committed memory.
- Eight live Supers: 71 processes; approximately 1.389 GB peak private committed memory.
- Increasing saved tabs from 24 to 1000 with only 24 visited did not increase live processes; 976 remained deferred with no page requests.

These are separate runner VMs, not controlled CPU improvement comparisons. Raw samples and summed working sets are recorded separately under `verification/0.2.19/`.

## First: unload individual inactive ordinary tabs

Keep the existing profile/cache path. Add an explicit lifecycle such as `live → sleep_requested → sleeping → waking`, keyed by browser identity and operation generation. Release resources only after `OnBeforeClose`. Keep the tab's session row and URL; reopening is a new navigation. The existing normal-close event deletes the row and must not be reused unchanged.

A refused close, failed safety check, or unknown state must retain the live browser. Pending events from an old browser must never remove a newly awakened browser. A normal user close during a sleep request must have deterministic behavior.

Before automatic unloading, track trusted editing without recording input values, including forms and contenteditable, navigation generations, playback, camera/microphone capture, active or paused downloads, loading/POST state and open DevTools. Protect active and pinned tabs and private contexts. Absence of a beforeunload handler is not proof that a page has no unsaved state. Unknown status must fail open: keep the page alive.

Acceptance requires real native pages: unchanged cookies/localStorage after wake; back/forward and scroll restoration semantics explicitly tested; useful draft/edit/media/download pages remain live; ordinary safe pages release renderer resources; repeated sleep/wake preserves the row exactly once; lock/startup master password still cover every window. Measure both private bytes and processes after confirmed close, not just a smaller Task Manager working set.

## Second: unload an idle Super after every page is safely asleep

Do not use the existing force-kill path as normal hibernation. Close browsers gracefully, flush storage, wait for engine exit and release route leases only after completion. Never start a second writer over the same profile. On failure preserve the profile and retain an explicit recoverable state.

Existing settings do not enable `persist_session_cookies`; engine restart can therefore change session-cookie behavior. Reusing the same directory alone is not evidence that logins survive. Resolve this privacy/lifetime choice and test real Windows session cookies before automatic Super hibernation.

## Strategic: bounded shared engine pool

A Linux capability probe proved independent proxies and cookies/localStorage in independent RequestContexts, but not Windows migration or memory savings. Shared engines still need separate renderer processes for isolation; expected savings are duplicated browser/GPU/service overhead and must be measured.

CEF requires a common root_cache_path. Existing Supers each use their own root and `kesh` child. Chromium on Windows uses Local State encryption key material; combining directories or choosing a common parent is not an authenticated-cookie migration. First test new synthetic Windows profiles, then consider new Supers in a new format. Keep old profiles on the existing engine until migration has backups, validated decryption, rollback and crash recovery.

A pool also requires per-Super context settings, compound tab/window identities, scoped private/password state, engine leases and bounded crash impact. Existing process globals and one-Child-per-Super ownership cannot be reused unchanged. Proxy setup must complete before navigation, with no silent Direct fallback.

## Sources

- [CEF close lifecycle](https://github.com/chromiumembedded/cef/blob/8078/include/cef_browser.h)
- [CEF cache roots and cookie persistence settings](https://github.com/chromiumembedded/cef/blob/8078/include/internal/cef_types.h)
- [CEF RequestContext isolation](https://github.com/chromiumembedded/cef/blob/8078/include/cef_request_context.h)
- [Chromium Windows DPAPI key provider](https://github.com/chromium/chromium/blob/main/components/os_crypt/async/browser/dpapi_key_provider.cc)
