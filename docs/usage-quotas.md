# Feed usage and publication quotas

**Manage → Feed usage & quota** reports committed local package IDs, versions, archive bytes and extracted symbol bytes. Each named feed and the root registry has separate counters and limits. Workspace administrators can manage every scope; named-feed administrators need `feeds:manage` and a management key without a package prefix. Readers and publishers cannot inspect aggregate usage or change limits.

Limits default to zero (unlimited):

- `maxBytes`: total locally published archive and extracted-symbol bytes.
- `maxPackages`: locally owned package IDs, including IDs with only unlisted versions.
- `maxVersions`: local versions, including prereleases and unlisted versions.

Upstream caches, mirrors, metadata, database size, unreferenced objects and pending physical deletion are excluded. The screen reports cache bytes separately; cache cleanup remains governed by its existing soft-quota policy. These publication limits measure committed references, not total bucket capacity.

## Enforcement

Publication checks capacity before writing blobs and holds the usage lock until its transaction finishes. Database triggers enforce the same limits and maintain counters on insert/delete/size changes, including non-HTTP writes. Concurrent pushes cannot exceed a configured limit. Exceeding a limit returns **409 Publication quota exceeded** without publishing a partial package.

Symbol uploads consume bytes but do not create another package version. Deleting symbols releases their committed usage; delayed object cleanup can still leave physical storage occupied for the grace period. Ownership transfers and unlisting do not release quota. Existing archives remain immutable and downloadable.

A limit can be lowered below current usage. Existing content is retained; an operation which increases a dimension beyond its configured limit is rejected. Quotas never delete packages to recover space. Raising a limit allows subsequent publication. Settings use a revision number to reject stale updates with 409.

## History and API

The worker samples each enabled feed every five minutes, keeping the latest sample per UTC day for up to 365 days. The API/UI show the latest 90 days; today's value is a sample, while the current usage counters are transactional. There is no invented historical data before installation.

- `GET /api/usage`: current `usage`, separate `cache` totals, and daily `history`.
- `PUT /api/usage` with `maxBytes`, `maxPackages`, `maxVersions` and the current `revision`: change limits (204).

All limits are nonnegative integers up to JavaScript's maximum safe integer. Use `/feeds/<name>/api/usage` for a named feed. The **Feeds & teams** screen links feed administrators to their scoped usage page.
