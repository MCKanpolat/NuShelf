# Deprecating package versions

**Manage → My packages → Deprecate** marks a local version as no longer maintained, affected by critical bugs, or deprecated for another reason. Add an optional explanation and alternative package/version range. Clearing deprecation restores normal metadata.

Deprecation does not unlist or delete a version. The archive stays immutable and existing restores continue. NuShelf exposes NuGet-compatible `catalogEntry.deprecation` in registration metadata and a warning in the gallery. NuGet clients can discover it using `dotnet list package --deprecated`. Custom messages may not be displayed by every client.

Owners need `package:deprecate`, a matching key prefix and publisher membership for named feeds. Workspace administrators can manage any local version. Upstream deprecation is mirrored from registration metadata; NuShelf does not edit upstream records.

APIs: `PUT /api/packages/<id>/<version>/deprecation` with `reasons` (`Legacy`, `CriticalBugs`, `Other`), optional `message`, and optional `alternatePackage` (`id`, `range`); `DELETE` clears it. Messages are limited to 2000 characters. Alternative ranges use NuGet interval syntax or `*`. Named feeds prefix the paths with `/feeds/<name>`.
