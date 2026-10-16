# NuGet OData V2

Configure `https://registry.example.com/api/v2` as a NuGet package source. Named feeds use `https://registry.example.com/feeds/<name>/api/v2` and enforce the same membership, API key scopes, and package prefix rules as V3.

The service document and `$metadata` describe an Atom/XML feed with streamed package content. Reads support GET and HEAD:

- `Packages`, `Packages/$count`, and `Packages(Id='Example.Core',Version='1.0.0')`
- `FindPackagesById()?id='Example.Core'` and its `/$count` variant
- `Search()?searchTerm='json'&includePrerelease=false` and its `/$count` variant
- `GetUpdates()?packageIds='Example.Core'&versions='1.0.0'&includePrerelease=false&includeAllVersions=false`
- `package/<id>/<version>` for archives, including unlisted exact versions

Push, unlist and relist retain their existing endpoints. Package IDs and exact versions are normalized. Unlisted versions remain available by ID/version; Search and GetUpdates include listed versions only. SemVer 2 packages are hidden from discovery unless `semVerLevel=2.0.0` is specified. An explicit entity key can always retrieve its requested SemVer 2 version.

## Query options

`$filter` supports the advertised property names, string/boolean/integer/datetime literals, `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `and`, `or`, `not`, parentheses, `tolower`, `substringof`, `startswith` and `endswith`. Single quotes inside OData string literals are doubled. Unsupported syntax and properties return 400; literals never become SQL fragments.

`$orderby` accepts comma-separated property names with `asc` or `desc`. `$select` chooses properties; identity fields remain present. `$inlinecount=allpages` includes the total before pagination. `$skip` and `$top` accept 0–100000. Each response contains at most 100 entries; follow the Atom `rel="next"` link until it disappears. The server preserves filters and the remaining `$top`. Count and page read one repeatable PostgreSQL snapshot.

GetUpdates accepts one `|`-separated version and optional NuGet version constraint per package ID, with at most 100 IDs. It returns the newest matching update, or every matching version with `includeAllVersions=true`. Constraints are applied before choosing the newest version. `targetFramework` (Search) and `targetFrameworks` (GetUpdates) accept short or .NET framework names. Compatibility covers the .NET Framework, .NET Standard and .NET Core/.NET base framework families; other framework names match exactly. Packages without declared framework restrictions remain visible. Platform-specific compatibility beyond exact matching is left to NuGet's restore compatibility check.

Local packages take precedence over upstream packages. Cached upstream versions appear only when their source routing and grants permit the reader. FindPackagesById and entity lookup can discover uncached upstream metadata; archives are fetched through the same validated read-through cache as V3. Package signatures, PDB formats and browser sign-in are independent of this protocol.
