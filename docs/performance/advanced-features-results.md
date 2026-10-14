# NuShelf HTTP performance test results

Generated: 2026-10-14T08:06:45.266409411Z

Dataset: 50000 packages, 50202 versions, 200000 search documents.

Overall: **PASS**. 100 requests per case/concurrency, 3 warmups, 20 database connections.

Latency includes the full HTTP/JSON request and response validation; PostgreSQL runs in a separate Docker container. Warm-cache closed-loop load, nearest-rank percentiles. No production latency guarantee.

| Case | Clients | Requests | p50 ms | p95 ms | p99 ms | Requests/s | Errors | p95 budget ms | Result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| exact_id | 1 | 100 | 5.27 | 7.18 | 7.53 | 178.3 | 0 | 500 | PASS |
| exact_id | 8 | 100 | 8.42 | 13.13 | 17.33 | 892.9 | 0 | 1414 | PASS |
| exact_id | 32 | 100 | 18.62 | 59.05 | 65.05 | 1186.9 | 0 | 2828 | PASS |
| prefix | 1 | 100 | 6.42 | 7.15 | 7.67 | 153.9 | 0 | 750 | PASS |
| prefix | 8 | 100 | 7.44 | 9.31 | 9.45 | 1023.6 | 0 | 2121 | PASS |
| prefix | 32 | 100 | 18.50 | 29.40 | 31.20 | 1479.3 | 0 | 4243 | PASS |
| full_text | 1 | 100 | 55.42 | 63.96 | 66.80 | 17.6 | 0 | 2000 | PASS |
| full_text | 8 | 100 | 77.29 | 93.96 | 97.64 | 99.2 | 0 | 5657 | PASS |
| full_text | 32 | 100 | 184.86 | 313.59 | 335.14 | 150.3 | 0 | 11314 | PASS |
| fuzzy_fallback | 1 | 100 | 7.57 | 9.88 | 11.45 | 127.1 | 0 | 1000 | PASS |
| fuzzy_fallback | 8 | 100 | 9.15 | 11.50 | 12.37 | 817.6 | 0 | 2828 | PASS |
| fuzzy_fallback | 32 | 100 | 22.22 | 35.95 | 38.27 | 1161.4 | 0 | 5657 | PASS |
| no_matches | 1 | 100 | 5.46 | 6.37 | 6.50 | 179.8 | 0 | 750 | PASS |
| no_matches | 8 | 100 | 7.63 | 8.69 | 9.34 | 1035.8 | 0 | 2121 | PASS |
| no_matches | 32 | 100 | 15.50 | 28.35 | 28.91 | 1637.6 | 0 | 4243 | PASS |
| pagination | 1 | 100 | 63.49 | 72.06 | 79.41 | 15.5 | 0 | 2000 | PASS |
| pagination | 8 | 100 | 86.15 | 113.47 | 119.54 | 89.0 | 0 | 5657 | PASS |
| pagination | 32 | 100 | 212.92 | 334.96 | 340.99 | 136.6 | 0 | 11314 | PASS |
| browse_recent | 1 | 100 | 14.60 | 20.44 | 23.21 | 64.4 | 0 | 1500 | PASS |
| browse_recent | 8 | 100 | 23.03 | 29.10 | 30.77 | 331.4 | 0 | 4243 | PASS |
| browse_recent | 32 | 100 | 72.42 | 107.74 | 125.58 | 390.2 | 0 | 8485 | PASS |
| version_history | 1 | 100 | 8.03 | 9.30 | 10.54 | 123.4 | 0 | 1000 | PASS |
| version_history | 8 | 100 | 10.49 | 13.48 | 15.98 | 728.0 | 0 | 2828 | PASS |
| version_history | 32 | 100 | 23.99 | 37.00 | 42.27 | 1166.1 | 0 | 5657 | PASS |
| stable_filter | 1 | 100 | 8.72 | 9.81 | 11.13 | 112.9 | 0 | 500 | PASS |
| stable_filter | 8 | 100 | 10.45 | 13.82 | 15.36 | 711.0 | 0 | 1414 | PASS |
| stable_filter | 32 | 100 | 31.43 | 53.63 | 58.60 | 853.8 | 0 | 2828 | PASS |
| prerelease_filter | 1 | 100 | 8.60 | 9.51 | 10.59 | 114.5 | 0 | 500 | PASS |
| prerelease_filter | 8 | 100 | 9.98 | 12.71 | 13.15 | 763.1 | 0 | 1414 | PASS |
| prerelease_filter | 32 | 100 | 27.67 | 43.15 | 43.93 | 1042.4 | 0 | 2828 | PASS |
| mcp_search | 1 | 100 | 13.00 | 14.15 | 15.73 | 75.8 | 0 | 750 | PASS |
| mcp_search | 8 | 100 | 15.76 | 21.08 | 28.05 | 484.0 | 0 | 2121 | PASS |
| mcp_search | 32 | 100 | 39.72 | 58.04 | 62.12 | 692.1 | 0 | 4243 | PASS |

Full-text index used: **true**. See `results.json` for configuration, effective p99/throughput budgets, error samples, and the PostgreSQL execution plan.
