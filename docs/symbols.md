# Symbol packages

NuShelf accepts symbol packages (`.snupkg`) and serves their portable PDBs, so debuggers can step into your packages' source.

## Publish symbols

Build the symbol package next to the package and push the `.nupkg`; the NuGet client sends the `.snupkg` too, using the same API key:

```bash
dotnet pack -c Release -p:IncludeSymbols=true -p:SymbolPackageFormat=snupkg
dotnet nuget push bin/Release/Your.Package.1.0.0.nupkg \
  --source https://nuget.example.com/v3/index.json \
  --api-key "$PROFILE_API_KEY"
```

The client finds the endpoint through the `SymbolPackagePublish/4.9.0` entry in the service index (`PUT /api/v2/symbolpackage`). `package:push` and the key's package prefix apply as for packages, and the package version must already exist and belong to the same profile. Named feeds work the same way against their own source URL and require the publisher or administrator role.

| Status | Meaning |
| --- | --- |
| `201` | Symbols stored. |
| `400` | Invalid symbol package: unsupported file types, a Windows PDB, no PDB, missing `SymbolsPackage` package type, or `<dependency>` entries declared (the empty dependency groups the .NET SDK writes are fine). |
| `403` | The package belongs to another profile or is outside the key's prefix. |
| `404` | No package with this ID and version exists yet. Push the `.nupkg` first. |
| `409` | Symbols for this version already exist. Delete them first to replace them. `dotnet nuget push --skip-duplicate` treats this as a skip. |

Only **portable PDBs** are supported (`<DebugType>portable</DebugType>`, the .NET SDK default). Allowed entries are `.pdb`, `.nuspec`, `.xml`, `.psmdcp`, `.rels` and `.p7s`. NuShelf does not check that a PDB matches a DLL in the package; only the package's owner can attach symbols.

## Use the symbol server

| Source | Symbol server URL |
| --- | --- |
| Root registry | `https://nuget.example.com/symbols` |
| Named feed | `https://nuget.example.com/feeds/<name>/symbols` |

Add the URL as a symbol server in Visual Studio (Tools → Options → Debugging → Symbols), Rider (Settings → Build, Execution, Deployment → Debugger → Symbol Servers) or `dotnet-symbol --server-path`. Root-registry symbols are public, like root-registry packages. In a private feed, send an API key with `package:read`; debuggers that only support Basic authentication can use any username with the API key as the password. A key limited to a package prefix only sees symbols for packages inside that prefix.

Responses use immutable caching for the root registry because a key identifies exactly one PDB. Symbol downloads are not counted as package downloads. HTTP range requests and compressed `.pd_` variants are not supported; debuggers fall back to a full download.

The gallery shows a **Symbols** badge for versions that have symbols, and `GET /api/packages/{id}` includes `symbols: {files, bytes}` per version.

## Replace or remove symbols

Symbols are immutable once stored. To correct a wrong symbol package, delete the symbols and push again:

```bash
curl -X DELETE https://nuget.example.com/api/packages/your.package/1.0.0/symbols \
  -H "Authorization: Bearer $PROFILE_API_KEY"
```

This needs the administrator key or the owner's key with `package:unlist` and a matching prefix. The stored files are removed from the symbol server immediately and deleted from storage after the cleanup delay, so a debugger that is mid-download is not interrupted. Unlisting a package version keeps its symbols, so older builds stay debuggable.

## Operations

- Both operations appear in the [audit log](events.md) as `symbols.published` and `symbols.deleted` and can trigger webhooks.
- PDBs are ordinary storage objects: [backups](deployment.md#backup-and-restore) and provider migration cover them, and the `symbol_*` tables travel in the PostgreSQL dump.
- Cache maintenance's orphan cleanup also collects `.pdb` objects left behind by an interrupted push.
- Symbol pushes use the same upload concurrency, size limit (`MAX_PACKAGE_BYTES` applies to the `.snupkg`) and timeouts as package pushes. The uncompressed PDBs of one symbol package may total at most 256 MiB.
- Packages fetched through an upstream proxy have no symbols here; point the debugger at the upstream's symbol server for those.
