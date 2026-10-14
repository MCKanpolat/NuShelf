# Rich package pages

Package pages render embedded Markdown README files, PNG/JPEG icons, source repository links and frameworks found under `lib/` and `ref/`. These framework labels describe included assets; they are not a full framework compatibility resolver.

README and icon references follow the nuspec `readme` and `icon` elements. Both slash conventions are normalized; absolute paths and parent traversal are rejected. Referenced files must exist and are limited to 1 MiB each. README text must be UTF-8. Icon content must have a PNG or JPEG signature; SVG is not accepted.

Markdown is parsed and sanitized on the server. Scripts, event handlers, unsafe link schemes, styles and embedded images are removed. Links use `noopener noreferrer`. External README images are currently omitted. Metadata and archive bytes remain immutable after publication.

Local packages and already cached upstream packages load their embedded assets when a detail page opens. For uncached upstream packages, **Load package README and icon** explicitly fetches and caches the archive. Existing packages are lazily indexed when their embedded assets are first requested. Private feed/source access is enforced on the asset and icon endpoints.

APIs: `GET /api/packages/<id>/<version>/assets` returns sanitized `readmeHtml`, `iconUrl`, `frameworks` and `repository`; `GET /api/packages/<id>/<version>/icon` serves the image. Named feeds prefix these paths with `/feeds/<name>`.
