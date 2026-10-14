# Documentation screenshots

These PNGs are real Chromium captures of the running NuShelf application. They are committed documentation assets, independent of ignored test output.

| Image | Screen |
| --- | --- |
| [home.png](home.png) | Search-led homepage, registry statistics, and sample packages. |
| [search.png](search.png) | JSON search, sorting, and prerelease controls. |
| [package.png](package.png) | Package metadata, dependency groups, and version history. |
| [management.png](management.png) | Publisher profile and three independently scoped API keys. |
| [owned-packages.png](owned-packages.png) | Per-version package visibility controls. |
| [mobile.png](mobile.png) | Responsive gallery at a 390-pixel viewport. |
| [upstream-sources.png](upstream-sources.png) | Source editor, routing controls and connection testing. |
| [upstream-cache.png](upstream-cache.png) | Cached archives and their upstream origins. |

The `Acme.*` packages and `Acme Engineering` profile are disposable examples created through the application API. They illustrate registry metadata and are not released .NET libraries. The upstream screenshots come from Docker browser tests with a generated .NET proxy fixture, captured by `tests/ui/upstreams.spec.js` after the real .NET test. Statistics come from actual fixture records. Only masked API key hints are visible; full credentials are not shown.

## Regenerate in Docker

Use a fresh isolated test stack so other fixtures do not appear in the gallery:

```bash
docker compose -f compose.test.yml --profile e2e --profile ui down
docker compose -f compose.test.yml --profile ui build ui
docker compose -f compose.test.yml --profile ui up -d app
docker compose -f compose.test.yml --profile ui run --rm --no-deps ui node screenshots.mjs
```

The [capture script](../../tests/ui/screenshots.mjs) publishes sample archives, creates example credentials, waits for each real UI screen, captures desktop/mobile views, and checks for browser errors and mobile overflow. Output goes to ignored `test-results/documentation/`.

Review the captures, then copy the six PNGs into `docs/images/`. The main application database and credentials are not used. Stop the test stack afterward:

```bash
docker compose -f compose.test.yml --profile e2e --profile ui down
```
