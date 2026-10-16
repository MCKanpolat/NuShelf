# Cache quotas, protection and cleanup

**Manage → Package cache** provides policy settings, a dry-run preview, cleanup execution, and per-version protection. Automatic maintenance is disabled by default and runs every ten minutes when enabled. Root and named feeds have independent policies.

- A byte quota is a **soft limit** on cached archives; zero means unlimited. Least recently accessed archives are removed first when over quota.
- An idle-days rule removes archives that have not been downloaded for that period; zero disables this rule.
- Protected versions and archives cached less than an hour ago are never automatically evicted. Locally published packages are never subject to cache eviction.
- Eviction removes cached search documents but keeps origin pins and registration metadata. A subsequent restore fetches the archive from the same origin.
- Removed archives are queued for deletion with a one-hour grace period for in-flight downloads. Storage failures are retried; existing referenced objects are preserved.
- Orphan discovery considers unreferenced `.nupkg` objects older than 24 hours, within the current feed's storage scope. The root scan excludes named-feed storage. Avoid running orphan cleanup while manually migrating or restoring storage.

Cleanup is bounded to 500 cache evictions and 500 queued deletions per run. Protected or recent archives can keep usage above quota; the preview reports this. Orphan scans process storage in batches. Back up PostgreSQL and package storage together. Cleanup deletes files after the grace period, so preview the policy before enabling it.

Administrator APIs: `GET/PATCH /api/cache/policy`, `POST /api/cache/maintenance` with `{ "dryRun": true, "orphans": true }`, and `PUT /api/cache/<id>/<version>/protection` with `{ "protected": true }`. Named-feed routes use `/feeds/<name>` before these paths.

Mirrored upstream archives have independent retention and are excluded from automatic quota/idle eviction. Clearing manual protection does not remove mirror retention. Pause a mirror before clearing its source cache. See [upstream mirrors](upstreams.md#live-discovery-and-mirrors).
