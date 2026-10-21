# Reverse dependencies

A package page's **Used by** section lists locally published package versions in the current feed which declare a dependency on that package ID. It shows the dependent ID/version, target framework and declared NuGet version range. This is declared package metadata, not a resolved dependency graph or a list of external applications consuming the package. Upstream package metadata is not imported into this index.

`GET /api/packages/<id>/dependents?take=50&skip=0&includeUnlisted=false` returns `totalHits`, `items`, `skip` and `take`. Pages contain up to 200 dependency declarations; a dependent version can have multiple declarations for different frameworks/ranges. Ordering is package ID, descending dependent version, framework and range. The count and page share a repeatable database snapshot. Listed versions are shown by default; `includeUnlisted=true` also includes unlisted versions.

Named-feed access and key prefixes apply to both the queried ID and returned dependent IDs. Root access stays public. Each feed has a separate transactional dependency index; publication, metadata changes and migrations populate it without an asynchronous indexing window.
