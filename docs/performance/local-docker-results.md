# NuShelf HTTP performance test results

Generated: 2026-10-13T21:54:09.769361853Z

Dataset: 50000 packages, 50202 versions, 200000 search documents.

Overall: **PASS**. 100 requests per case/concurrency, 3 warmups, 20 database connections.

Latency includes the full HTTP/JSON request and response validation; PostgreSQL runs in a separate Docker container. Warm-cache closed-loop load, nearest-rank percentiles. No production latency guarantee.

| Case | Clients | Requests | p50 ms | p95 ms | p99 ms | Requests/s | Errors | p95 budget ms | Result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| exact_id | 1 | 100 | 4.49 | 5.10 | 5.92 | 219.0 | 0 | 500 | PASS |
| exact_id | 8 | 100 | 5.49 | 15.58 | 18.68 | 1216.5 | 0 | 1414 | PASS |
| exact_id | 32 | 100 | 13.87 | 44.84 | 51.53 | 1549.5 | 0 | 2828 | PASS |
| prefix | 1 | 100 | 5.48 | 7.15 | 7.56 | 175.6 | 0 | 750 | PASS |
| prefix | 8 | 100 | 6.15 | 8.24 | 8.60 | 1194.1 | 0 | 2121 | PASS |
| prefix | 32 | 100 | 17.08 | 28.41 | 30.58 | 1556.6 | 0 | 4243 | PASS |
| full_text | 1 | 100 | 56.36 | 68.41 | 85.17 | 17.1 | 0 | 2000 | PASS |
| full_text | 8 | 100 | 84.20 | 102.01 | 111.92 | 91.9 | 0 | 5657 | PASS |
| full_text | 32 | 100 | 200.88 | 304.04 | 387.96 | 142.0 | 0 | 11314 | PASS |
| fuzzy_fallback | 1 | 100 | 6.78 | 7.44 | 8.16 | 144.6 | 0 | 1000 | PASS |
| fuzzy_fallback | 8 | 100 | 7.47 | 10.35 | 10.88 | 976.4 | 0 | 2828 | PASS |
| fuzzy_fallback | 32 | 100 | 20.45 | 28.05 | 30.67 | 1391.4 | 0 | 5657 | PASS |
| no_matches | 1 | 100 | 4.70 | 5.39 | 5.67 | 208.1 | 0 | 750 | PASS |
| no_matches | 8 | 100 | 6.94 | 8.32 | 8.97 | 1111.8 | 0 | 2121 | PASS |
| no_matches | 32 | 100 | 17.89 | 25.46 | 29.02 | 1602.5 | 0 | 4243 | PASS |
| pagination | 1 | 100 | 59.80 | 73.85 | 85.43 | 16.2 | 0 | 2000 | PASS |
| pagination | 8 | 100 | 72.06 | 87.03 | 94.88 | 107.2 | 0 | 5657 | PASS |
| pagination | 32 | 100 | 205.56 | 304.68 | 332.14 | 136.6 | 0 | 11314 | PASS |
| browse_recent | 1 | 100 | 10.90 | 12.34 | 12.86 | 90.8 | 0 | 1500 | PASS |
| browse_recent | 8 | 100 | 15.44 | 18.29 | 18.74 | 491.6 | 0 | 4243 | PASS |
| browse_recent | 32 | 100 | 48.67 | 73.76 | 88.53 | 562.3 | 0 | 8485 | PASS |
| version_history | 1 | 100 | 7.25 | 7.82 | 8.06 | 136.7 | 0 | 1000 | PASS |
| version_history | 8 | 100 | 9.70 | 11.60 | 11.77 | 813.8 | 0 | 2828 | PASS |
| version_history | 32 | 100 | 26.73 | 36.81 | 41.82 | 1054.9 | 0 | 5657 | PASS |
| stable_filter | 1 | 100 | 6.63 | 8.56 | 9.52 | 145.6 | 0 | 500 | PASS |
| stable_filter | 8 | 100 | 9.70 | 11.72 | 12.97 | 818.7 | 0 | 1414 | PASS |
| stable_filter | 32 | 100 | 19.95 | 29.86 | 32.27 | 1393.0 | 0 | 2828 | PASS |
| prerelease_filter | 1 | 100 | 6.52 | 6.97 | 7.96 | 151.3 | 0 | 500 | PASS |
| prerelease_filter | 8 | 100 | 7.21 | 9.62 | 10.38 | 1008.7 | 0 | 1414 | PASS |
| prerelease_filter | 32 | 100 | 20.65 | 28.33 | 31.81 | 1346.4 | 0 | 2828 | PASS |
| mcp_search | 1 | 100 | 12.02 | 12.88 | 14.16 | 82.2 | 0 | 750 | PASS |
| mcp_search | 8 | 100 | 14.26 | 18.78 | 19.54 | 528.2 | 0 | 2121 | PASS |
| mcp_search | 32 | 100 | 38.86 | 53.53 | 57.07 | 761.1 | 0 | 4243 | PASS |

Full-text index used: **true**. See `results.json` for configuration, effective p99/throughput budgets, error samples, and the PostgreSQL execution plan.
