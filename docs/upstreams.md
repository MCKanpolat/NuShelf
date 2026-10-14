# Upstream sources and package cache

[← Project overview](../README.md)

NuShelf can act as a NuGet V3 proxy: resolve a missing package from configured upstream registries, store its archive in your active storage provider, and serve subsequent requests locally. Locally published package namespaces and upstream cache records remain separate.

## Add nuget.org

1. Sign in to **Manage workspace** with the administrator key.
2. Open **Upstream sources** and select **Add nuget.org**.
3. Keep `https://api.nuget.org/v3/index.json`, the `*` include pattern, and no upstream authentication.
4. Add exclusions for any namespaces that must never be resolved from public sources.
5. Select **Test connection**, then **Add source**.

Sources are configured explicitly; a fresh deployment has no upstreams. Your NuGet clients still use one NuShelf service index. A direct package URL or `dotnet add package ... --version ...` can resolve an uncached package. The gallery includes downloaded, indexed cache entries; when an exact ID is not found in search, **Look up … in upstreams** opens its upstream metadata and fetches the archive on download.

## Multiple registries and routing

Each source has a priority, include patterns, exclude patterns, and an enabled switch.

| Rule | Example | Precedence |
| --- | --- | --- |
| Exact package ID | `Partner.Contracts` | Highest. |
| Package prefix | `Acme.*` | Longest matching prefix wins. |
| Catch-all | `*` | Lowest. |

Only sources matching the most specific rule are considered. Within that group, lower priority numbers are tried first; UUID order breaks ties. An exclusion removes that source from consideration. A 404 for the package ID advances to the next candidate. Authentication, invalid metadata, checksum, and connection failures do not silently select a different registry.

Example:

| Source | Include | Exclude | Priority |
| --- | --- | --- | ---: |
| Company feed | `Acme.*` | — | 10 |
| Partner feed | `Partner.*` | — | 20 |
| nuget.org | `*` | `Acme.*, Partner.*` | 100 |

Once a package ID resolves, it is pinned to that source. Versions are not merged across registries, even when two sources contain the same ID. If that source lacks a particular version, NuShelf returns 404 rather than switching the origin. Routing changes that invalidate a pin require explicit cache clearing.

Local package IDs never fall back to an upstream, including missing local versions. A cached/pinned upstream ID cannot subsequently be published locally until an administrator clears its source cache. These rules prevent accidental changes to an already resolved package's origin.

## Credentials and private reads

Upstream credentials are referenced by an `UPSTREAM_*` environment variable. They are not stored in source rows or returned to the browser. The main Compose application reads `.env` in addition to its explicit deployment settings.

```dotenv
UPSTREAM_COMPANY_TOKEN=<token>
UPSTREAM_COMPANY_PASSWORD=<password>
```

Restart/recreate the application after changing environment secrets. In the source editor select **Bearer token** or **Basic authentication**, enter the environment variable name, and supply a username for Basic auth. Credentials are sent only to the service index's origin; redirects or resource URLs on another origin never receive them.

Authenticated sources default to private when the API's `private` property is omitted. The UI selects **Private source** when an authentication mode is chosen. Explicitly making such a source public is an administrator decision: it publishes the cache to anonymous readers.

A private source can be read by administrators, or by a profile key satisfying all three checks:

- `package:read` permission;
- a profile grant on the source;
- a key prefix matching the package ID.

MCP also requires `mcp:read`. Private archives, metadata, search results, and cache statistics are filtered accordingly. Public gallery statistics do not include private cached packages. Revoked or expired profile keys cannot read cached private content. A private source without grants is administrator-only.

For a NuGet client, use a profile key as the Basic authentication password; the username may be `nushelf`:

```bash
dotnet nuget add source https://packages.example.com/v3/index.json \
  --name NuShelf --username nushelf --password "$PROFILE_API_KEY"
```

Platforms without NuGet password encryption support may require `--store-password-in-clear-text`; manage that configuration as a credential-bearing file. Local packages and public upstream caches retain public read access.

## Cache behavior

| Item | Behavior |
| --- | --- |
| Service index and version metadata | Configurable TTL, default 300 seconds. |
| Registration metadata | Configurable TTL; page metadata is cached and local URLs replace upstream links. |
| Package archive | Stored until the source cache is explicitly cleared. |
| Missing package ID | Negative cache for 60 seconds. |
| Simultaneous misses | PostgreSQL advisory locks deduplicate metadata and archive fetches across replicas. |
| Cache hit | Served without contacting the origin. |
| Temporary origin outage | When stale metadata is allowed, known metadata and cached archives remain usable; stale version discovery prefers versions with cached archives. |
| Corrupt archive | ID/version checks and available SHA-512 sidecars must pass before the archive is indexed. |

Downloads expose `X-NuShelf-Cache: MISS` on a cache fill and `HIT` on a cache hit, plus the upstream UUID in `X-NuShelf-Upstream`. HEAD and nuspec/hash reads do not increment download counts. A first HEAD/nuspec/hash request may populate the archive cache.

Search results list local packages first, followed by visible cached packages, with sorting within each group. This release does not federate keyword search across every upstream. Metadata-only entries appear through exact-ID lookup; downloaded archives join PostgreSQL search. Listing changes discovered during registration refresh update cached search visibility.

## Manage storage and cleanup

**Package cache** shows the latest 500 cached versions, their origins, cache times, download counts, and sizes. Source totals cover all entries. **Clear cache** removes that source's archives, metadata, negative entries, and package pins. Future restores require the origin again. Local packages are unaffected.

Clear a source before changing its URL or removing it. Disabling a source makes its cache unavailable until re-enabled. Storage cleanup failures are reported separately; interrupted uploads or failed deletions can leave unreferenced objects. Automatic quota eviction/orphan cleanup is not implemented.

## Network boundaries and limits

- HTTPS is required unless **Allow HTTP** is explicitly enabled.
- Private/reserved addresses require **Allow private network endpoints**, including Docker-only origins.
- Every redirect and resource URL is validated; DNS results are pinned for the connection, and implicit environment proxies are disabled.
- A source cannot point back to this registry; discovered self-referencing package resources are rejected.
- Request timeouts are 1–30 seconds. Archive size follows `MAX_PACKAGE_BYTES`; archive work shares the four-operation upload/cache concurrency bound.
- Metadata is capped at 16 MiB per response/assembled catalog, 128 registration pages, and 10,000 versions. Full catalog fetching has a 60-second deadline.
- Package signature validation is not implemented. TLS checks, archive parsing, identity checks, and optional origin checksums are performed.

## Tests

```bash
docker compose -f compose.test.yml build test
docker compose -f compose.test.yml run --rm test
docker compose -f compose.test.yml --profile e2e up --build \
  --abort-on-container-exit --exit-code-from e2e e2e
docker compose -f compose.test.yml --profile ui run --rm --build ui
```

Integration tests cover priority fallback, source pinning, negative cache, checksum/identity rejection, 16 concurrent misses producing one origin download, private grants/Basic/MCP, cached reads during outages, and cleanup for both local disk and S3/MinIO.

The .NET test generates a real package in a separate Docker origin, restores it through NuShelf only, checks a cache HIT and exactly one origin download, simulates an origin 503 outage, clears client caches, then restores and builds again. Browser tests cover source creation/editing/removal, connection checks, cache management, and the existing gallery workflow.

Live verification against nuget.org also fetched `Newtonsoft.Json` 13.0.3 (2,441,966 bytes): the first request reported MISS, the second HIT, and both bodies matched. A manual .NET client script is included at [tests/e2e/live-nuget-client.sh](../tests/e2e/live-nuget-client.sh); run it in the application container's network namespace after configuring nuget.org:

```bash
docker run --rm --network container:nushelf-app-1 \
  -v "$PWD/tests/e2e/live-nuget-client.sh:/live.sh:ro" \
  mcr.microsoft.com/dotnet/sdk:8.0 bash /live.sh
```

Use the actual application container name if your Compose project name differs. This optional live test depends on internet access and is not part of the deterministic CI fixture tests.
