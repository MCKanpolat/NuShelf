# Restore diagnostics

Open **Manage workspace → Restore diagnostics** with an API key. Supply a package ID and exact version. The report evaluates the current caller and feed; administrator reports describe administrator access, not another publisher's permissions.

`GET /api/diagnostics/restore?id=Acme.Core&version=1.0.0&probe=false` validates package/key prefixes, local namespace ownership, include/exclude specificity, source priority, origin pins, private upstream grants, cached archives and negative-cache expiry. Restricted private sources are reported without disclosing names, rules or identifiers. Unlisted local versions remain downloadable by exact version. Local missing versions never fall back upstream.

With `probe=true`, strict origin service-index/version-index reads use the same safe HTTP client as restores. Sources are visited in resolver order; a new package's 404 can advance to the next candidate, while access/network errors and pinned origins stop resolution. Reads do not persist pins, metadata, misses or archives. The report is a point-in-time explanation, not an execution trace of a client restore. Cached metadata/stale fallback can differ from live origin results; neither an indexed version nor a cache row proves the archive is currently readable. The outer request timeout bounds the whole diagnosis.

The generated NuGet.Config uses this feed as the sole package source with explicit `*` source mapping. It contains no secrets. Configure private-feed credentials separately. Existing client global-cache entries and local client configuration are outside server visibility; the screen does not claim to diagnose them.
