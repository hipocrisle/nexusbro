# Individual tab routes — implementation contract, not a released feature

Owner priority: each ordinary tab can inherit its Super route, use Direct, or select an existing saved VPN connection. No domain-based substitute and no changing the route of neighboring tabs.

## Existing evidence

Reuse `probe/route-context`, source `5a26686601ec056e2960246558e34273dafb4652`, successful GitHub native probe [44526165115](https://github.com/hipocrisle/nexusbro/actions/runs/44526165115). Independent RequestContexts used proxy A and B concurrently and isolated cookies/localStorage; shared contexts shared storage but both used the last proxy B. A process CLI proxy overrode both contexts and prevented preference changes. That original run was Linux HTTP capability, explicitly product_feature_ready:false. The later Windows capability result is recorded below; HTTPS/service-worker admission and product integration remain unaccepted.

Do not merge its special build.yml launcher into a product branch.

## Profile and routing contract

- Keep the existing Super root and the **native effective legacy cache path** unchanged for inherited traffic. Do not assume `kesh`: CEF156 ignores a child Settings.cache_path and uses its default global profile. The Windows probe measured `root/Default`. Discover/canonicalize the native path under the owned unchanged root, then reopen that exact path; never move Default to kesh or copy Windows Local State. Add override caches only inside that root.
- `RouteChoice = Inherit | Direct | Connection{id}` in local ordinary-tab state. Persist IDs, never subscription URLs, credentials, local ports or keys. Missing/removed IDs fail closed, without implicit Direct fallback.
- The CLI proxy must not defeat context preferences. Each context is blocked from navigation until its actual proxy preference is accepted; test startup workers/restored caches before product use.
- Reuse existing connection/tunnel/filtering infrastructure with bounded leases. Direct still uses the local filtering proxy; do not spawn one Xray per tab.
- Changes are transactional: engine/window/tab/browser identity and route generation; failure keeps the prior selection and route. Do not interrupt media, drafts or downloads silently.
- All windows of one Super use the same route registry. Private route contexts stay in memory and separate from ordinary contexts.
- Translation must resolve the actual tab route; clearing site data must cover route contexts. Preserve password origin and native Browser ownership guards.

## Approved profile policy (20 November 2026)

The owner explicitly approved one storage profile per effective route within each Super. Tabs using the same effective route share cookies/storage; different Supers remain isolated. Different routes can hold different accounts. A new login when moving an existing tab to another route is an intentional safeguard against exposing an account through another exit. Never copy authentication between route profiles. Inherited legacy storage remains intact. A failed selected VPN must fail closed, without falling back to Direct. Route changes must explain the profile change before applying.

Ordinary tabs are not currently part of the cloud Super payload. Do not promise route-choice sync as part of this local feature.

## Next acceptance

The Windows legacy-profile capability probe below is complete. Continue product integration with the measured native effective profile path, then native simultaneous Direct/inherited/VPN, HTTPS CONNECT, downloads, restart, failure/no-Direct-leak, startup browserless/worker admission and unchanged neighboring-tab routes. Keep native capability distinct from product readiness.

## Windows capability result

[44729558840](https://github.com/hipocrisle/nexusbro/actions/runs/44729558840), native source `a7544566a3e1ecb3714ecceda1e0cba4b193edce`, passed the real Windows context probe. Independent contexts routed A/B concurrently and isolated storage; shared contexts routed both to B and shared storage; CLI proxy A prevented preference changes. Legacy CLI/global profile → context preferences preserved persistent HttpOnly server-auth and localStorage at the actual `root/Default`. An additional process with empty global disk cache and explicit RequestContext to that exact legacy directory plus a new override also preserved old auth only in the inherited context, routing A/B simultaneously. No copying or moving cookies, Local State or profiles. Session HttpOnly cookies did not survive engine restart with existing defaults.

This result remains `product_feature_ready:false`: HTTP capability only; HTTPS/CONNECT, service-worker early admission, real VPN/UDP and integration into the application still require acceptance. The owner has explicitly accepted separate authentication per route; product integration is now authorized.

## Product integration file ownership and transaction contract

- Parent: `lib.rs`, `sessiya.rs`, `tab_routes.rs`, new `tab_route_parent.rs`, proxy task lifecycle.
- Native: `cef_vkladki.rs`, `cef_dvizhok.rs`, new `cef_route_context.rs`.
- UI: `ObychnyeVkladki.tsx` and dedicated route selector component/styles.
- Network leases: new `route_network.rs`, no changes to other packages.

The trusted UI calls `tab_route_choices({id})` (only sanitized names/IDs) and `tab_route_set({id,tabId,route,expectedGeneration})`. Applied selection remains authoritative native session state. Parent preparation is single-flight per tab transaction, followed by exact engine/window/tab/browser/generation checks. Native must refuse unsafe replacement of media/drafts/downloads and preserve the existing route on failure. Requests time out visibly; timeout is not proof of application or rollback.

`NativeRoute` contains a 64-character lowercase hexadecimal effective-path key and strictly validated `http://127.0.0.1:PORT`, never remote credentials or bypass rules. Inherited and explicit choices for the same effective path resolve to one key/profile. A pre-spawn bootstrap file contains the inherited prepared route; restored override routes resolve lazily before navigation. Native `route-needed`/`route-result` events identify window label, tab ID, attempt generation and the old browser ID (zero if not created). Parent replies with `RouteApply` or `RouteFailed`. Attempt generations cannot be reused after a failure.

Leases remain until the owning engine has exited because service workers and downloads can outlive visible tabs. The first implementation bounds each Super/engine to eight effective routes including inherited, with global caps on VPN children/startup. It reuses one tunnel per Super/effective path, never per tab. Established CONNECT tasks belong to their proxy listener and are aborted on shutdown. Translation resolves actual native route key and identity, without an unknown-key fallback to the Super route. Data clear/statistics must include all owned route contexts.

These are implementation contracts, not passed product acceptance. No new product build is authorized until all route components are integrated.

Speculative preloading and DNS hints are disabled per RequestContext using registered `net.network_prediction_options=2`, before network admission. See the exact version's [registered preference](https://raw.githubusercontent.com/chromium/chromium/156.0.8078.88/chrome/browser/preloading/preloading_prefs.cc), [disabled enum](https://raw.githubusercontent.com/chromium/chromium/156.0.8078.88/chrome/browser/preloading/preloading_prefs.h) and [proxy-aware preresolver](https://raw.githubusercontent.com/chromium/chromium/156.0.8078.88/content/browser/preloading/preconnect/preconnect_manager_impl.cc). CanSet/Set failure denies admission rather than silently ignoring the policy.

The first native smoke's ERR_PROXY_CONNECTION_FAILED exposed a wildcard resolver rule that also mapped the owned numeric loopback proxy. That failed run is preserved. The global resolver rewrite was subsequently removed: Direct WebRTC must retain explicit STUN/TURN hostname resolution. HTTP/HTTPS use the configured fixed proxy without a DIRECT alternative; VPN route contexts prohibit non-proxied WebRTC UDP, while actual Direct contexts retain ordinary WebRTC policy. Native positive/negative hostname-STUN witnesses and a stopped configured proxy are required in addition to ordinary HTTP/HTTPS route receipts. No external STUN server or production DNS change is involved.
