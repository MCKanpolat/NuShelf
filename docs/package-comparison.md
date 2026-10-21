# Package comparison

Open **Manage workspace → Package comparison** with an API key. Enter a package ID, a baseline version and a strictly newer target. The package gallery's **Compare versions** link preselects the ID. Both archives must already be locally published or cached in this feed. Comparison does not download missing upstream packages. Exact unlisted versions can be compared.

The durable queue supports ten active requests per feed, deduplicates active pairs, and retains thirty finished comparisons per package. Workers claim jobs with database leases and retry interrupted claims up to three times. One analysis runs per server process; a claim expires after five minutes. Each run is bounded to 210 seconds; each APICompat direction has a 60-second limit. Deploy replica counts and resource limits to match analysis capacity.

Archives are checked against their stored byte count and SHA-512, capped at the lower of MAX_PACKAGE_BYTES and 256 MiB per archive, and inspected on a blocking worker. ZIP expansion is capped at 512 MiB and 10,000 entries. Unsafe/duplicate paths and symlinks are rejected before API analysis. File SHA-256 changes, metadata/dependencies, lib/ref framework assets, runtime folders and compressed sizes are compared.

Microsoft.DotNet.ApiCompat.Tool reads assembly metadata in a child process; package code is not executed. It runs in both directions: baseline-to-target findings identify potential breaks, while target-to-baseline findings help expose added APIs. Structured suppression entries include diagnostic ID, target and left/right assets; full diagnostic output is retained because asset/framework issues are not necessarily API suppression entries. Reports are informational. An empty finding list is not a guarantee of runtime compatibility, and missing dependencies/tool errors must be reviewed. Same-major forward API findings trigger a SemVer review notice. The report does not enforce publishing policy.

## Deployment

The production Docker image contains the .NET 10 runtime and Microsoft.DotNet.ApiCompat.Tool 10.0.100. Native deployments need the tool and a compatible .NET runtime:

```sh
dotnet tool install Microsoft.DotNet.ApiCompat.Tool --version 10.0.100 --tool-path /opt/apicompat
export APICOMPAT_PATH=/opt/apicompat/apicompat
```

Without APICOMPAT_PATH, `apicompat` is resolved on PATH. Tool startup, malformed input, storage failures and timeouts mark the job failed; NuShelf does not manufacture a compatibility result. Temporary files are removed after each run. Give the container sufficient temporary disk and RAM; the archive pair and bounded analyzer output are materialized during analysis. No external package restore is performed by the comparison service.

## API and isolation

- `POST /api/packages/{id}/comparisons`: `{ "baselineVersion": "1.0.0", "targetVersion": "1.1.0" }`, returns HTTP 202 with job `id`.
- `GET /api/packages/{id}/comparisons`: latest thirty jobs.
- `GET /api/packages/{id}/comparisons/{job}`: status, error or report. Poll until completed/failed.

Named feeds expose the same routes below `/feeds/{name}`. Requests honor current feed membership, key prefixes and private upstream grants. Detailed report reads reauthorize both archive references; purged/replaced archives and revoked grants cannot expose old report contents. Administrator reports use administrator privileges.
