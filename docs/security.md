# NuGet Audit and advisory mirroring

Open **Manage → Package security** to enable hourly synchronization from nuget.org, synchronize immediately, and see the last successful update. Synchronization is disabled by default. Data stays in PostgreSQL and is shared by the root registry and named feeds.

NuShelf advertises `VulnerabilityInfo/6.7.0` only after a successful import or synchronization. The index and all page URLs point to NuShelf. Clients can audit without direct internet access after data is mirrored. Snapshots are immutable, hash-addressed and retained for seven days after replacement. A failed sync preserves the last successful snapshot and records an error; absence of a reported advisory does not establish that a package is safe.

```xml
<configuration>
  <packageSources><add key="NuShelf" value="https://packages.example.com/v3/index.json" /></packageSources>
  <auditSources><add key="NuShelf" value="https://packages.example.com/v3/index.json" /></auditSources>
</configuration>
```

Private feeds require the same NuGet credentials for audit URLs. The package page displays known advisories and their affected version ranges, highlighting those affecting the selected version. This is advisory lookup, not binary malware scanning. License blocking and quarantine policies are future work.

## API and offline import

Workspace administrator credentials protect `GET/PATCH /api/audit`, `POST /api/audit/sync`, and `POST /api/audit/import`. Offline import accepts a NuGet vulnerability-page JSON dictionary of lowercase package IDs, with `severity` (0–3), HTTPS `url`, and NuGet `versions` ranges. An empty import uses `[]`. This replaces the current snapshot atomically; import your complete data set. Imports are limited to 16 MiB. Synchronization reads only HTTPS resources on the fixed official NuGet hosts, follows no redirects, limits each page to 64 MiB and the job to 60 seconds, and needs at least two database connections. Upstream credentials are never used for audit synchronization.

`GET /api/audit/packages/<id>` lists known advisories with ranges. Public root reads are anonymous; named-feed reads follow feed permissions and key prefixes.

Reference: [NuGet VulnerabilityInfo specification](https://learn.microsoft.com/en-us/nuget/api/vulnerability-info).
