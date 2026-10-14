# NuShelf HTTP performance test results

Generated: 2026-10-13T22:48:11.325124803Z

Dataset: 50000 packages, 50202 versions, 200000 search documents.

Overall: **PASS**. 100 requests per case/concurrency, 3 warmups, 20 database connections.

Latency includes the full HTTP/JSON request and response validation; PostgreSQL runs in a separate Docker container. Warm-cache closed-loop load, nearest-rank percentiles. No production latency guarantee.

| Case | Clients | Requests | p50 ms | p95 ms | p99 ms | Requests/s | Errors | p95 budget ms | Result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| exact_id | 1 | 100 | 5.18 | 5.83 | 6.25 | 188.0 | 0 | 500 | PASS |
| exact_id | 8 | 100 | 7.12 | 14.32 | 20.57 | 998.0 | 0 | 1414 | PASS |
| exact_id | 32 | 100 | 19.15 | 43.14 | 48.84 | 1382.1 | 0 | 2828 | PASS |
| prefix | 1 | 100 | 6.27 | 7.26 | 7.58 | 156.8 | 0 | 750 | PASS |
| prefix | 8 | 100 | 7.15 | 9.37 | 9.63 | 1044.8 | 0 | 2121 | PASS |
| prefix | 32 | 100 | 20.80 | 25.20 | 31.93 | 1419.2 | 0 | 4243 | PASS |
| full_text | 1 | 100 | 54.14 | 65.69 | 76.80 | 17.8 | 0 | 2000 | PASS |
| full_text | 8 | 100 | 70.19 | 84.23 | 85.74 | 110.3 | 0 | 5657 | PASS |
| full_text | 32 | 100 | 178.78 | 313.40 | 324.27 | 157.3 | 0 | 11314 | PASS |
| fuzzy_fallback | 1 | 100 | 7.37 | 7.93 | 8.69 | 133.9 | 0 | 1000 | PASS |
| fuzzy_fallback | 8 | 100 | 11.37 | 14.40 | 15.88 | 689.3 | 0 | 2828 | PASS |
| fuzzy_fallback | 32 | 100 | 23.47 | 30.24 | 39.37 | 1230.8 | 0 | 5657 | PASS |
| no_matches | 1 | 100 | 5.36 | 6.08 | 6.41 | 183.8 | 0 | 750 | PASS |
| no_matches | 8 | 100 | 6.00 | 7.31 | 7.47 | 1251.0 | 0 | 2121 | PASS |
| no_matches | 32 | 100 | 15.03 | 25.96 | 26.25 | 1822.6 | 0 | 4243 | PASS |
| pagination | 1 | 100 | 62.34 | 71.62 | 77.86 | 15.6 | 0 | 2000 | PASS |
| pagination | 8 | 100 | 76.26 | 96.42 | 100.01 | 100.7 | 0 | 5657 | PASS |
| pagination | 32 | 100 | 196.74 | 355.18 | 358.05 | 141.5 | 0 | 11314 | PASS |
| browse_recent | 1 | 100 | 11.72 | 13.16 | 13.67 | 84.1 | 0 | 1500 | PASS |
| browse_recent | 8 | 100 | 15.92 | 18.09 | 20.30 | 483.0 | 0 | 4243 | PASS |
| browse_recent | 32 | 100 | 40.16 | 62.01 | 65.46 | 696.1 | 0 | 8485 | PASS |
| version_history | 1 | 100 | 8.00 | 9.09 | 9.73 | 122.1 | 0 | 1000 | PASS |
| version_history | 8 | 100 | 9.57 | 11.95 | 12.21 | 802.9 | 0 | 2828 | PASS |
| version_history | 32 | 100 | 23.54 | 39.05 | 40.74 | 1129.8 | 0 | 5657 | PASS |
| stable_filter | 1 | 100 | 8.61 | 10.19 | 10.74 | 114.2 | 0 | 500 | PASS |
| stable_filter | 8 | 100 | 9.43 | 12.38 | 12.83 | 780.7 | 0 | 1414 | PASS |
| stable_filter | 32 | 100 | 24.92 | 34.78 | 39.74 | 1164.6 | 0 | 2828 | PASS |
| prerelease_filter | 1 | 100 | 8.19 | 9.02 | 9.43 | 120.7 | 0 | 500 | PASS |
| prerelease_filter | 8 | 100 | 9.41 | 12.12 | 12.38 | 788.6 | 0 | 1414 | PASS |
| prerelease_filter | 32 | 100 | 23.75 | 33.56 | 37.59 | 1174.9 | 0 | 2828 | PASS |
| mcp_search | 1 | 100 | 13.07 | 18.68 | 20.61 | 73.1 | 0 | 750 | PASS |
| mcp_search | 8 | 100 | 14.77 | 20.36 | 21.26 | 491.9 | 0 | 2121 | PASS |
| mcp_search | 32 | 100 | 39.19 | 60.01 | 65.07 | 685.4 | 0 | 4243 | PASS |

Full-text index used: **true**. See `results.json` for configuration, effective p99/throughput budgets, error samples, and the PostgreSQL execution plan.
