# Individual tab routes — implementation contract, not a released feature

Owner priority: each ordinary tab can inherit its Super route, use Direct, or select an existing saved VPN connection. No domain-based substitute and no changing the route of neighboring tabs.

## Existing evidence

Reuse `probe/route-context`, source `5a26686601ec056e2960246558e34273dafb4652`, successful GitHub native probe [44526165115](https://github.com/hipocrisle/nexusbro/actions/runs/44526165115). Independent RequestContexts used proxy A and B concurrently and isolated cookies/localStorage; shared contexts shared storage but both used the last proxy B. A process CLI proxy overrode both contexts and prevented preference changes. This is Linux HTTP capability, explicitly product_feature_ready:false; Windows/HTTPS/service-worker startup are not accepted.

Do not merge its special build.yml launcher into a product branch.

## Profile and routing contract

- Keep the existing Super root and `kesh` unchanged for inherited traffic. Add override caches only inside that root; no directory merging or Windows Local State migration.
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

Extend the existing probe on Windows with legacy root/kesh: seed real persistent + HttpOnly cookies and localStorage, restart without CLI proxy using context preferences, validate actual server Cookie headers and unchanged inherited auth. Measure session cookies honestly. Then native simultaneous Direct/inherited/VPN, HTTPS CONNECT, downloads, restart, failure/no-Direct-leak and unchanged neighboring-tab routes. Keep native capability distinct from product readiness.
