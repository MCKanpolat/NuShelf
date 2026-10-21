# Storage integrity scans

**Manage → Storage integrity** checks committed references in the current root registry or named feed. Workspace administrator credentials are required. Automatic scanning is disabled initially; enable a 1–8760-hour interval or queue a manual scan. Disabling a schedule does not cancel a running scan. Disabled feeds pause background work until re-enabled.

A scan snapshots locally published `.nupkg` archives, cached upstream archives and extracted portable PDBs. It streams each object, checking byte length and SHA-512 (base64) for archives or SHA-256 (hex) for PDBs. It does not buffer entire archives, contact upstream registries, repair files, delete files, quarantine packages or validate cryptographic package signatures.

## Results

| State | Meaning |
| --- | --- |
| `pending` | Object has not been checked yet. |
| `ok` | Length and hash match. |
| `missing` | Storage reports the referenced object does not exist. |
| `mismatch` | Length or hash differs from committed metadata. |
| `unavailable` | Storage failed or the 60-second per-object bound elapsed; this does not prove corruption. |
| `obsolete` | The snapshotted reference was removed or changed before its turn. |

Database row locks coordinate replicas and keep a checked reference alive while reading, preventing a concurrent cache eviction or symbol deletion from producing a false missing result. Each object result commits independently; interrupted work rolls back and resumes on the next worker pass. One scan per feed can be active; duplicate manual requests return its ID. New publications after snapshot creation are covered by a later scan. The worker processes bounded batches with at most four feeds concurrently.

The last 30 completed scan inventories plus the active inventory are retained. The management page shows the latest 20 runs and paginates issues. Inspect and correct storage problems through your backup/restore procedures, then run another scan to verify the repair.

## API

- `GET /api/integrity`: schedule and recent run summaries/counters.
- `PUT /api/integrity` with `enabled` and `intervalHours`: update schedule (204).
- `POST /api/integrity/runs`: queue a manual scan (202, returns `id`).
- `GET /api/integrity/runs/<uuid>?take=100&skip=0&showAll=false`: issue details; `showAll=true` also includes pending, matching and obsolete references. Maximum page size is 200.

Named-feed APIs use `/feeds/<name>` as usual. Storage credentials, provider errors and internal object keys are not returned by these endpoints.
