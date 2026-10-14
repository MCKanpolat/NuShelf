# Deployment and usage

[← Project overview](../README.md)

This guide covers NuShelf v0.1.0. For transaction boundaries, ownership, and module design, see [architecture](architecture.md). For measured search behavior, see [performance](performance.md).

## Docker setup

Run Docker with Linux containers and Compose support. Rust and PostgreSQL do not need to be installed on the host.

For a fresh checkout:

```bash
cp .env.example .env
```

PowerShell: `Copy-Item .env.example .env`. Preserve an existing `.env` rather than replacing its secrets.

Set independent random values for `ADMIN_API_KEY` and `POSTGRES_PASSWORD`. The administrator key must contain at least 32 characters. The database password appears in a connection URL, so use URL-safe characters, such as the hex encoding of 32 random bytes. `.env` is ignored by Git.

```bash
docker compose up --build -d
docker compose ps
```

| Setting | Default or requirement |
| --- | --- |
| `ADMIN_API_KEY` | Required; at least 32 characters. Used for profile and key administration. |
| `POSTGRES_PASSWORD` | Required by the main Compose deployment. |
| `PUBLIC_URL` | `http://localhost:5000`; externally reachable HTTP(S) origin without a path. |
| `STORAGE_PROVIDER` | `local`; also accepts `s3`, `azure`, or `gcs`. |
| `STORAGE_PATH` | `/data/packages` inside the runtime container. |
| `MAX_PACKAGE_BYTES` | `104857600` (100 MiB). |
| `DATABASE_CONNECTIONS` | `20`. |
| `METRICS_BIND_ADDRESS` | Unset (metrics off). Address of a separate Prometheus listener, e.g. `0.0.0.0:9090`; see [Metrics](#metrics). |

Compose supplies the internal `DATABASE_URL` and the runtime binds to `0.0.0.0:5000` inside its container. The host port defaults to `127.0.0.1:5000`. PostgreSQL is not published to the host.

Migrations run automatically on startup. The `postgres-data` and `packages` volumes persist database and local package files independently. Back up both the database and the selected package storage; storage keys must survive restoration.

### URLs

- Gallery: [http://localhost:5000](http://localhost:5000)
- Management: [http://localhost:5000/manage](http://localhost:5000/manage)
- NuGet source: [http://localhost:5000/v3/index.json](http://localhost:5000/v3/index.json)
- MCP: `http://localhost:5000/mcp`
- Liveness: `GET /health/live`
- Readiness: `GET /health/ready`

Set `PUBLIC_URL` to the external origin for a reverse proxy deployment. NuGet document links are built from that value rather than request headers.

Readiness returns `200` when PostgreSQL answers and the package store can be listed, and `503` otherwise, each within three seconds. The body names the failing check, for example `{"checks":{"database":"ok","storage":"unavailable"}}`; the underlying error is logged rather than returned. A missing bucket or container fails the storage check. S3 needs `s3:ListBucket`, which cache maintenance already uses.

## First profile and API key

1. Open management and sign in with `ADMIN_API_KEY`.
2. Create a publisher profile under **Publisher profiles**.
3. Select the profile and open **API keys & credentials**.
4. Give the key a name, permissions, an optional package prefix, and an expiration.
5. Copy the full key when it is displayed. It cannot be retrieved later.

Profile keys authenticate publishing and MCP requests. The administrator credential manages profiles and keys and cannot publish a package; publication must be attributed to a profile.

| Permission | Action |
| --- | --- |
| `package:push` | Publish packages. |
| `package:unlist` | Unlist or relist owned versions. |
| `package:read` | Read private feeds with membership, and private upstream caches granted to this profile. |
| `package:deprecate` | Deprecate local versions and recommend alternatives. |
| `feeds:manage` | Manage settings and membership when the profile is a feed administrator. |
| `mcp:read` | Call MCP tools. |
| `keys:manage` | Manage the profile name and profile keys. |

The first profile to publish a package owns its ID. Other profiles cannot upload new versions under that ID. A prefix such as `Acme.` further restricts writes. It does not restrict reads of public packages.

A key with `keys:manage` can issue child keys for its own profile. A child cannot gain scopes, broaden a prefix, or outlive its issuer. Keys are independent credentials: revoking a parent does not cascade to children. Management permission allows revocation of any key within the same profile.

Keys are generated with high entropy and stored as SHA-256 digests. List endpoints return only labels and hints. Revocation and expiration are checked on each request. The UI keeps the login credential in tab memory and clears it on sign-out or reload; it does not use persistent browser storage.

## NuGet client configuration

```bash
dotnet nuget add source http://localhost:5000/v3/index.json --name NuShelf
dotnet pack --configuration Release
dotnet nuget push ./bin/Release/Your.Package.1.0.0.nupkg \
  --source http://localhost:5000/v3/index.json \
  --api-key "$PROFILE_API_KEY"
dotnet add package Your.Package --version 1.0.0 \
  --source http://localhost:5000/v3/index.json
```

Set `PROFILE_API_KEY` through your shell or CI secret configuration. Replace the package ID, path, and version with those produced by your project. PowerShell uses `$env:PROFILE_API_KEY` for environment variables and backticks for multiline continuation.

For clients that require explicit local HTTP permission, configure `NuGet.Config`:

```xml
<configuration>
  <packageSources>
    <add key="NuShelf" value="http://localhost:5000/v3/index.json" allowInsecureConnections="true" />
  </packageSources>
</configuration>
```

Use HTTPS for a hosted installation. The **Publish** page accepts a `.nupkg` and a profile key as an alternative to the CLI.

### Listing and versions

Published `(package ID, normalized version)` combinations are immutable. DELETE unlists a version without deleting the archive; POST to the corresponding listing endpoint relists it. Unlisted versions are excluded from search but remain downloadable for existing restores.

Version URLs are normalized, lowercased, and omit build metadata. Display metadata keeps the normalized full version.

## Storage configuration

Metadata and nuspec manifests live in PostgreSQL. The selected provider holds `.nupkg` archives, which are streamed on download.

### Local filesystem

The default `STORAGE_PROVIDER=local` uses the persistent volume mounted at `/data/packages`.

### S3-compatible storage with MinIO

Configure `.env`:

```dotenv
STORAGE_PROVIDER=s3
AWS_BUCKET=nushelf
AWS_REGION=us-east-1
AWS_ENDPOINT=http://minio:9000
AWS_ACCESS_KEY_ID=nushelf-local
AWS_SECRET_ACCESS_KEY=<independent-random-storage-password>
AWS_ALLOW_HTTP=true
AWS_VIRTUAL_HOSTED_STYLE_REQUEST=false
```

```bash
docker compose --profile s3 up --build -d
```

`minio-init` prepares the bucket. Wait for it to exit successfully before using the registry. The fixture is for local development/testing and builds pinned official source releases; see [docker/minio.Dockerfile](../docker/minio.Dockerfile).

### AWS S3

Use your bucket, region, and credential values with `STORAGE_PROVIDER=s3`. Leave `AWS_ENDPOINT` empty for AWS, set `AWS_ALLOW_HTTP=false`, and do not enable the local MinIO profile.

Temporary credentials can include `AWS_SESSION_TOKEN`. For workload credentials, leave explicit credentials empty and configure the environment's AWS credential mechanism. Validate access against your actual bucket. Other S3-compatible providers may require a custom endpoint or different path/virtual-host addressing.

### Azure Blob Storage

```dotenv
STORAGE_PROVIDER=azure
AZURE_STORAGE_ACCOUNT_NAME=<account>
AZURE_STORAGE_ACCOUNT_KEY=<secret>
AZURE_STORAGE_CONTAINER_NAME=<container>
```

Create the container beforehand. The adapter compiles but has not been verified against a live Azure account in the local test suite.

### Google Cloud Storage

```dotenv
STORAGE_PROVIDER=gcs
GOOGLE_BUCKET=<bucket>
GOOGLE_SERVICE_ACCOUNT_PATH=/run/secrets/gcs.json
```

Create the bucket beforehand and mount the service account file read-only into the configured container path using a Compose override. The adapter compiles but has not been verified against a live GCS account in the local test suite.

### Credentials and migration

Storage credentials are deployment secrets, separate from publisher API keys. Management displays the active provider but does not return or edit storage secrets.

Changing `STORAGE_PROVIDER` does not move existing blobs. Copy archives to the target provider under the same storage keys before switching configuration.

## Metrics

Set `METRICS_BIND_ADDRESS` to serve Prometheus metrics at `GET /metrics` on a separate listener, for example `0.0.0.0:9090`. It must use a different port from the registry, and it is not routed through the registry port, so publishing the registry does not expose it. Without the setting nothing is recorded.

| Metric | Type | Labels |
| --- | --- | --- |
| `nushelf_http_requests_total` | counter | `method`, `route`, `status` |
| `nushelf_http_request_duration_seconds` | histogram | `method`, `route`, `status` |
| `nushelf_db_pool_connections` | gauge | `state` (`idle`, `active`) |
| `nushelf_build_info` | gauge | `version` |

`route` is the matched route pattern, such as `/v3/flatcontainer/{id}/{version}/{file}`, so package IDs and versions never become label values. Publishes are `PUT /api/v2/package` or `PUT /api/v2/package/` requests (the .NET client uses the trailing slash) and downloads are `GET /v3/flatcontainer/{id}/{version}/{file}` requests.

## MCP connection

Use a profile key with `mcp:read` and a remote HTTP MCP client. Configuration formats vary; the endpoint and header are illustrated below:

```json
{
  "mcpServers": {
    "nushelf": {
      "url": "http://localhost:5000/mcp",
      "headers": { "Authorization": "Bearer <PROFILE_API_KEY>" }
    }
  }
}
```

Tools: `search_packages`, `get_package`, and `list_versions`. All are read-only and share the registry's search and metadata services.

Stateless Streamable HTTP uses MCP protocol `2025-06-18`:

- POST requests must accept both `application/json` and `text/event-stream`.
- After initialization, send `MCP-Protocol-Version: 2025-06-18`.
- Accepted notifications return 202; JSON request results return 200.
- Authenticated GET/DELETE requests return 405; standalone SSE and resumable sessions are not offered.
- No session ID is stored.
- If an Origin header is present, it must match `PUBLIC_URL`.

## Docker verification

```bash
docker compose -f compose.test.yml build test
docker compose -f compose.test.yml run --rm test
docker compose -f compose.test.yml --profile e2e up --build \
  --abort-on-container-exit --exit-code-from e2e e2e
docker compose -f compose.test.yml --profile ui run --rm --build ui
```

Run the .NET test before the browser test: it produces the browser upload fixture. Tests use the separate `nushelf-test` project and disposable credentials. PostgreSQL integration tests create unique databases, separate from the main application.

For the opt-in HTTP load and performance regression suite, run:

```bash
docker compose -f compose.test.yml --profile performance run --rm --build performance
```

It starts PostgreSQL only, creates its own temporary database, and writes `test-results/performance/results.json` and `results.md`. See [performance configuration](performance.md#http-performance-regression-suite) for dataset size, concurrency, sample count, and regression budgets.

Formatting and static analysis:

```bash
docker compose -f compose.test.yml run --rm --no-deps test cargo fmt --all -- --check
docker compose -f compose.test.yml run --rm --no-deps test cargo clippy --locked --all-features --all-targets -- -D warnings
```

Stop test containers without affecting the normal application:

```bash
docker compose -f compose.test.yml --profile e2e --profile ui down
```

See [screenshot capture instructions](images/README.md) to regenerate documentation images separately from browser regression tests.

## Operational boundaries

- Root-registry reads and discovery are public. Named feeds support private local packages, isolated namespaces and team membership; see [named feeds](feeds.md). Private upstream caches use source grants and `package:read`; see [upstream configuration](upstreams.md).
- Browser sign-in uses API keys; password/SSO, email flows and public self-registration are not implemented. GitHub Actions OIDC is supported for [Trusted Publishing](trusted-publishing.md).
- The V3 API and V2-shaped publication endpoint are implemented; legacy OData V2 feeds and symbol serving are not.
- Storage upload completes before metadata commits. An interruption can leave an unreferenced blob; orphan cleanup is available through opt-in [cache maintenance](cache-maintenance.md).
- Backups need both PostgreSQL and package storage. Provider changes require explicit migration.
- Readiness validates PostgreSQL, not storage permissions. Validate actual cloud accounts separately.
- Configure HTTPS, request/rate limits, and proxy body/timeout limits for hosted environments.
- Download counts represent served GET requests, including retries, rather than unique users. HEAD and nuspec requests do not increment them.
