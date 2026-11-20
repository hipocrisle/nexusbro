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

## Open owner decision

Stock CEF cannot provide shared cookie/storage login and independent proxies through shared RequestContexts. New override profiles require separate login; inherited tabs keep their original login. Owner has been asked whether this is acceptable. Do not treat elapsed time as an answer or implement a promise of shared login across routes.

Ordinary tabs are not currently part of the cloud Super payload. Do not promise route-choice sync as part of this local feature.

## Next acceptance

The Windows legacy-profile capability probe below is complete. Continue product integration with the measured native effective profile path, then native simultaneous Direct/inherited/VPN, HTTPS CONNECT, downloads, restart, failure/no-Direct-leak, startup browserless/worker admission and unchanged neighboring-tab routes. Keep native capability distinct from product readiness.

## Windows capability result

[44729558840](https://github.com/hipocrisle/nexusbro/actions/runs/44729558840), native source `a7544566a3e1ecb3714ecceda1e0cba4b193edce`, passed the real Windows context probe. Independent contexts routed A/B concurrently and isolated storage; shared contexts routed both to B and shared storage; CLI proxy A prevented preference changes. Legacy CLI/global profile → context preferences preserved persistent HttpOnly server-auth and localStorage at the actual `root/Default`. An additional process with empty global disk cache and explicit RequestContext to that exact legacy directory plus a new override also preserved old auth only in the inherited context, routing A/B simultaneously. No copying or moving cookies, Local State or profiles. Session HttpOnly cookies did not survive engine restart with existing defaults.

This result remains `product_feature_ready:false`: HTTP capability only; HTTPS/CONNECT, service-worker early admission, real VPN/UDP and integration into the application still require acceptance. The owner decision about separate/mandatory-shared auth remains open.
