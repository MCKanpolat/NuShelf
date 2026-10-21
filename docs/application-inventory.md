# Application inventory and vulnerability impact

**Manage workspace → Application inventory** connects CI-reported application releases to exact NuGet dependency versions. Each feed has its own applications, releases, components and dependency graph. It does not infer application usage from package downloads.

## Access

Create a profile key with `inventory:write` for CI imports and `inventory:read` for browsing, exporting, comparing releases and viewing impact. Keys need an unrestricted package prefix because an application inventory can contain unrelated package namespaces. Existing keys do not acquire these scopes automatically. A write-only key can register/import through the API without reading other inventories; the browser requires read permission.

Applications belong to the registering profile. Workspace administrators can assign an existing profile with `ownerProfileId`; named-feed owners must be publishers/administrators in that feed. Feed administrators with `feeds:manage` and the appropriate inventory scope can manage all applications in their feed. Other profiles see only their own applications. Feed membership and key revocation are checked on every request. Administrator-created applications with no owner are administrator-only in root, and visible to authorized managers in named feeds. App ownership is independent of package ownership and does not grant package downloads.

## Import contract

Register an application, then upload a CycloneDX JSON SBOM for each release:

```http
POST /api/applications
Authorization: Bearer <INVENTORY_KEY>
Content-Type: application/json

{"name":"Billing API"}
```

```http
POST /api/applications/<application-id>/releases
Authorization: Bearer <INVENTORY_KEY>
Content-Type: application/json

{
  "version": "2026.10.21.1",
  "repositoryUrl": "https://github.com/example/billing",
  "commitSha": "0123456789012345678901234567890123456789",
  "sbom": {
    "bomFormat": "CycloneDX",
    "specVersion": "1.6",
    "version": 1,
    "metadata": {"component":{"type":"application","bom-ref":"billing","name":"Billing API"}},
    "components": [{"type":"library","bom-ref":"json","name":"Newtonsoft.Json","version":"13.0.3","purl":"pkg:nuget/Newtonsoft.Json@13.0.3"}],
    "dependencies": [{"ref":"billing","dependsOn":["json"]},{"ref":"json","dependsOn":[]}]
  }
}
```

Use `/feeds/<name>/api/applications` for a named feed. A release version is immutable while its inventory exists: replaying the same serialized SBOM and repository/commit fields returns the existing ID; different input returns 409. SHA-256 covers NuShelf's deterministic JSON serialization of the SBOM, not original file formatting. Repository and commit values are caller-reported metadata, not provenance attestations. No repository or URL in the document is fetched.

The importer accepts the inventory subset of CycloneDX JSON 1.4–1.7: positive document `version`, metadata root component, nested `components`, NuGet PURLs, and `dependencies[].ref/dependsOn`. All components need names; existing `bom-ref` values must be unique. Missing component references receive internal IDs. Exact NuGet versions come from PURL or component version and must agree when both exist. NuGet IDs/versions are normalized using the registry parser. Other ecosystems remain in the original document but are not assessed against NuGet advisories. The metadata root represents the application and is not counted as an assessed NuGet dependency.

This is not a full CycloneDX schema validator. Unknown extension fields are retained. Declared dependency references must resolve within this document; external BOM-Link dependency references are rejected. Documents without a root or dependency edges can still provide version inventory, but relationships without a root path are marked `unrooted`. Cycles are handled without recursion. Paths show one shortest declared route; they do not establish runtime code reachability. The importer does not resolve NuGet version ranges or manufacture missing dependency edges.

Requests are limited to 8 MiB including the envelope; the browser limits source files to 7.5 MiB. Import admission uses the existing upload semaphore before reading the body. Limits per document: 5,000 components including the root, nesting depth 32, 20,000 unique dependency edges. Each feed supports 100 applications and each application supports 100 release inventories. Limits fail explicitly; inventories are never silently pruned. Owners/managers can explicitly delete obsolete release inventories; this removes inventory data, not package archives.

## CI upload

Generate a CycloneDX JSON SBOM from the application's resolved restore graph using your selected SBOM generator. Upload the complete application inventory, including transitive packages, after restore/build. Keep the inventory key in CI secrets; do not commit it. For example, with `jq` and `curl`:

```sh
jq -n --arg version "$RELEASE_VERSION" --arg commit "$COMMIT_SHA" \
  --slurpfile sbom ./bom.json \
  '{version:$version,commitSha:$commit,sbom:$sbom[0]}' > inventory-request.json

curl --fail-with-body --silent --show-error \
  -H "Authorization: Bearer $NUSHELF_INVENTORY_KEY" \
  -H 'Content-Type: application/json' \
  --data-binary @inventory-request.json \
  "$NUSHELF_URL/api/applications/$APPLICATION_ID/releases"
```

Use a feed-scoped base URL for NUSHELF_URL when appropriate. The SBOM is a reported dependency inventory, not evidence that a release is currently deployed in an environment. Keep obsolete deployment inventories updated/removed through your CI process.

## Reports

- `GET /api/applications`: applications visible to this caller.
- `GET /api/applications/{app}/releases`: latest 100 imports, ordered by import time.
- `GET /api/applications/{app}/releases/{release}?take=100&after=<bom-ref>`: components, direct/transitive/unrooted relationships, one shortest named dependency path and matching advisories. Pages contain 1–200 components. `nextAfter` advances pagination; paths longer than 64 nodes are shortened and flagged.
- `GET /api/applications/{app}/releases/{release}/sbom`: original reported document with JSON formatting normalized.
- `GET /api/applications/{app}/diff?baseline=<release-id>&target=<release-id>`: added/removed NuGet IDs and changed exact-version sets, including multiple resolved versions of an ID.
- `DELETE /api/applications/{app}/releases/{release}`: explicitly remove an inventory and its graph.
- `GET /api/applications/impact?latestOnly=true&take=100`: affected applications/releases/components using the current mirrored advisory snapshot. Latest means most recently imported, not highest SemVer or currently deployed. Set `latestOnly=false` to include historical inventories.

Impact pages scan up to 200 candidate components whose package IDs have advisory entries, then evaluate exact NuGet version ranges. A page can contain no affected components but still return `nextCursor`; continue until it is null. Cursors are tied to the advisory hash; a changed snapshot returns 409 and requires restarting pagination. Reports return the hash and creation time. They update when a new advisory snapshot becomes current, without reimporting inventories. No automatic notification subscription is created.

Missing advisory data is reported as unavailable. A populated snapshot with zero matches does not prove safety, freshness, inventory completeness or exploitability. Only NuGet advisories in the existing mirror are evaluated; vulnerability claims embedded in an uploaded SBOM do not override that source. Release inventory/graph and advisory reads use a consistent database snapshot within each report request.

Registration, imports and deletions enter the existing audit log and signed webhook infrastructure as `application.created`, `inventory.imported`, and `inventory.deleted`. Event bodies contain IDs/status, not SBOM documents or credentials. Back up PostgreSQL to preserve inventories; they do not add objects to package storage.
