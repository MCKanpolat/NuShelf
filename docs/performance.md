# Local Docker search measurements

This document distinguishes the original service-layer benchmark from the opt-in HTTP regression suite. Their latency numbers are not interchangeable: the HTTP suite uses a release build and also transfers, parses, and checks each response.

## HTTP performance regression suite

The suite in [tests/performance.rs](../tests/performance.rs) is enabled only with the `performance-tests` Cargo feature. It measures a running Axum server over real HTTP rather than calling search directly. The client and server share a Docker container; PostgreSQL runs in a separate container. Production databases and object storage are not used.

### Run

```bash
docker compose -f compose.test.yml --profile performance run --rm --build performance
```

Defaults: **50,000 packages**, **50,202 versions**, **200,000 search documents**, **100 requests per scenario/concurrency**, three warmups, and **1, 8, 32** clients. Eleven scenarios produce 33 phases and 3,300 timed requests. The history fixture contains 201 listed versions and one unlisted version. A preview fixture checks stable/prerelease visibility.

| Scenario | Correctness check |
| --- | --- |
| Exact ID | One package, correct ID/version and version count. |
| ID prefix | 100 matching packages with the expected page size. |
| Full text | Dataset-derived match count and version metadata. |
| Fuzzy fallback | A misspelled ID resolves to the intended package. |
| No matches | Empty results and zero total hits. |
| Pagination | Count remains correct and the offset page is complete. |
| Browse recent | Total count and page size. |
| Long version history | Latest version and 201 visible versions; unlisted version excluded. |
| Stable filter | Latest stable version without the preview. |
| Prerelease filter | Preview selected when requested with SemVer2 support. |
| MCP search | Authenticated JSON-RPC tool call returns the correct structured result. |

### Load model and metrics

Each phase starts workers behind a barrier. Workers send the configured fixed total number of requests in a **closed loop**: a worker waits for its previous request before issuing another. This measures throughput at a given client concurrency; it does not simulate an externally imposed arrival rate or compensate for coordinated omission.

Timing ends after the entire response body is received, decoded, and checked. It includes HTTP routing, database count/page/version work, response transfer, client JSON decoding, and validation. It excludes setup, seeding, warmups, remote-client network latency, TLS, cloud storage operations, and mixed read/write workloads. The Rust test and application are built in release mode.

Percentiles use the nearest-rank method. The report contains p50, p95, p99, maximum latency, requests per second, successes/errors, effective budgets, configuration, environment details, and the PostgreSQL full-text count plan. With 100 samples per phase, p99 is a coarse tail estimate; use larger sample counts for sustained measurements.

### Regression gates

All phases require **zero HTTP, MCP, or response-validation errors**. The following single-client p95 and throughput budgets are configurable workload guardrails:

| Scenarios | Base p95 budget | Minimum requests/s |
| --- | ---: | ---: |
| Exact ID, stable filter, prerelease filter | 500 ms | 10 |
| Prefix, no matches, MCP search | 750 ms | 5 |
| Fuzzy fallback, long history | 1,000 ms | 5 |
| Browse recent | 1,500 ms | 3 |
| Full text, pagination | 2,000 ms | 2 |

Effective p95 budget = base budget × √concurrency × `PERF_BUDGET_MULTIPLIER`. The p99 budget is twice the effective p95 budget. Minimum throughput is divided by the multiplier; it is not multiplied by concurrency. The multiplier does not relax the correctness/zero-error requirements. The actual full-text count plan must use `search_fulltext_idx` for datasets of at least 10,000 packages; no planner setting forces index use.

These initial ceilings catch severe regressions while allowing different Docker/CI hardware. Calibrate stricter budgets on a stable runner before treating them as project performance targets. They are not production latency or capacity guarantees.

### Configuration

| Variable | Default | Range/purpose |
| --- | --- | --- |
| `PERF_PACKAGES` | `50000` | 1,000–200,000 synthetic package IDs. |
| `PERF_REQUESTS` | `100` | 10–10,000 timed requests per phase. |
| `PERF_WARMUPS` | `3` | 1–100 untimed requests per phase. |
| `PERF_CONCURRENCY` | `1,8,32` | Comma-separated client counts, 1–128 and no greater than requests per phase. |
| `PERF_DB_CONNECTIONS` | `20` | PostgreSQL connection pool size, 1–100. |
| `PERF_BUDGET_MULTIPLIER` | `1` | Finite scale factor, 0.1–100, recorded in the report. |
| `PERF_REPORT_DIR` | `/results/performance` in Compose | Mounted as `test-results/performance/` on the host. |

Example with a smaller dataset (Bash):

```bash
PERF_PACKAGES=10000 PERF_REQUESTS=64 PERF_CONCURRENCY=1,8 \
  docker compose -f compose.test.yml --profile performance run --rm --build performance
```

PowerShell:

```powershell
$env:PERF_PACKAGES = '10000'
$env:PERF_REQUESTS = '64'
$env:PERF_CONCURRENCY = '1,8'
docker compose -f compose.test.yml --profile performance run --rm --build performance
```

The test requires the `nushelf_test` connection, creates a uniquely named `nushelf_perf_<random ID>` database, and drops it after ordinary successes or failures. Forced termination can leave that isolated database behind. Regular tests do not run this suite unless its Cargo feature is explicitly enabled.

### Reports and CI

Reports are written to ignored local artifacts:

- `test-results/performance/results.json`: machine-readable measurements and query plan.
- `test-results/performance/results.md`: readable results table.

A completed local 50,000-package run is preserved as [Markdown](performance/local-docker-results.md) and [JSON](performance/local-docker-results.json). All 33 phases passed with 3,300 timed requests and zero errors. At 32 clients, p95 was 44.84 ms for exact IDs, 304.04 ms for full text, and 53.53 ms for MCP search. This is a single synthetic local run, not a production capacity estimate.

The suite also verifies its own measurement path with a deliberately slow HTTP server (valid responses must fail a strict latency budget) and an incorrect HTTP 200 response (must fail preflight). The nearest-rank percentile calculation has deterministic checks. Report files are marked incomplete before database setup so a database setup/preflight failure cannot leave an earlier PASS artifact in place.

The suite was rerun after upstream integration: [results](performance/upstream-integration-results.md), [JSON](performance/upstream-integration-results.json). All 33 phases again passed without errors. This workload exercises local search with an empty upstream cache; it does not measure large cached datasets or internet latency.

The separate [CI job](../.github/workflows/ci.yml) uses 10,000 packages, 64 requests, and 1/8 clients. It applies the same gates and uploads reports even when a budget fails. That CI job is configured in the repository; local runs do not verify GitHub execution.

```bash
docker compose -f compose.test.yml --profile performance down
```

## Original service-layer benchmark

Synthetic dataset: **50,000 packages**, one version each, **200,000 search documents** across four channels. PostgreSQL 17 and the Rust search service run inside Docker on the development machine. One caller, three warmups and thirty timed requests per case; each request includes the count, a page of up to twenty packages, and version lists. Rust benchmark uses the dev build. External HTTP, TLS and browser rendering are excluded.

| Search case | Matching packages | Median | p95 |
| --- | ---: | ---: | ---: |
| Exact ID `bench.package025000` | 1 | 4.93 ms | 5.26 ms |
| Full text `serialization` | 5,000 | 55.80 ms | 61.41 ms |
| Prefix `bench.package0499` | 100 | 6.06 ms | 6.42 ms |
| Browse by recent publication | 50,000 | 10.48 ms | 11.84 ms |

These results describe this synthetic local run, not a production latency guarantee. They do not cover cold caches, concurrency, large version histories, remote databases, fuzzy fallback or long offsets. General searches with many matches require more ranking work than exact/prefix searches.

The full-text count query used a `BitmapOr` of `search_fulltext_idx` and `search_prefix_idx`, followed by a bitmap heap scan. PostgreSQL reported **4.936 ms** execution time for that plan. This verifies that the expected GIN/prefix indexes are used on this dataset.

Exact/full-text/prefix matching runs first. Trigram similarity is consulted only when it finds no matches. This keeps shared package name prefixes from broadening an exact query into thousands of fuzzy hits.

Reproduce in a disposable test database, without changing the main application database:

```sh
docker compose -f compose.test.yml build test
docker compose -f compose.test.yml run --rm test cargo run --locked --example search_benchmark
```

The benchmark creates a database named `nushelf_bench_<random ID>`, seeds it, runs `ANALYZE`, prints measurements and `EXPLAIN (ANALYZE, BUFFERS)`, then drops that database on success. If interrupted, an isolated benchmark database may remain for inspection.
