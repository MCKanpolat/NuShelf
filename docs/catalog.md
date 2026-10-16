# NuGet Catalog

The V3 service index advertises `Catalog/3.0.0` at `/v3/catalog/index.json`. Named feeds advertise their own scoped catalog URLs. GET and HEAD are supported on every document.

The catalog describes packages **published into this registry/feed**. Read-through upstream caches retain their own publication origins: a cache fill or eviction is not a local publication event. Use [upstream discovery and mirroring](upstreams.md#live-discovery-and-mirrors) to enumerate external origins.

| Document | Content |
| --- | --- |
| `/v3/catalog/index.json` | Page URLs, page counts, latest commit ID and timestamp. |
| `/v3/catalog/pages/<page>.json` | Up to 500 event positions, chronological leaf references and their package identities. |
| `/v3/catalog/leaves/<event-id>.json` | An immutable snapshot or deletion tombstone. |

Discover URLs through the preceding document rather than constructing them. Empty catalogs have an empty `items` array and the Unix epoch as their initial timestamp.

## Events and consistency

Publishing, unlisting, relisting, deprecating and clearing deprecation produce `PackageDetails`. Unlisting keeps the archive available and is never represented as deletion. Privileged database deletion of a package version produces `PackageDelete`; the ordinary NuGet DELETE endpoint continues to unlist.

Details include the package identity, full version/build metadata, original display version, description, authors, tags, dependencies, license/project metadata, listing state, deprecation, SHA-512 hash, byte size, creation and publication times. Unlisted snapshots use the conventional `1900-01-01T00:00:00Z` publication value. Relisting records a new publication time. Dependency registration and content URLs refer to the current registry/feed. The independent NuGet Audit resource supplies vulnerability data.

Database triggers write events inside the same transaction as the package mutation. A failed publication or rolled-back metadata update cannot leave an event behind. A transaction-scoped head row lock serializes catalog writers, and timestamps increase strictly even if the database clock moves backwards. Commit IDs are unique event UUIDs. Consumers never see a later timestamp before an earlier catalog-writing transaction commits.

Catalog payloads reject UPDATE and DELETE. Download counters and repeated listing/deprecation requests that do not change state do not generate events. Pages occupy fixed sequence ranges; rolled-back writes can leave sequence gaps, so a sealed page may contain fewer than 500 entries. After a newer page appears, prior pages remain unchanged. Leaf responses are generated from the saved snapshot, so later package changes cannot alter old history.

The migration bootstraps one snapshot for each existing local version. It preserves current state and original creation times; events that occurred before installation of the catalog cannot be reconstructed. Migration application is idempotent through the existing root and named-feed migration runners.

## Consuming the catalog

1. Read the index and choose pages whose `commitTimeStamp` is greater than the saved cursor.
2. Read those pages, then their leaves in timestamp order.
3. Apply each snapshot/tombstone idempotently.
4. Save the greatest successfully applied timestamp from the catalog.

Save a server timestamp rather than generating a cursor from the consumer's clock. Advance only after all earlier leaves have been applied. The durable upstream mirror follows this protocol when its source advertises Catalog.

Private named feeds enforce membership and `package:read` before serving any catalog document. Prefix-scoped keys see only matching events, counts and leaf URLs. Direct access to another feed's leaf returns 404. Responses use private revalidation, representation ETags, conditional GET/HEAD and credential-aware `Vary` headers.
