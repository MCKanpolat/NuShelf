//! Symbol packages (`.snupkg`) and the symbol server.
//!
//! A symbol package carries portable PDBs for an already published package.
//! Each PDB is stored as its own object and addressed by the key debuggers
//! compute from the PDB ID (Simple Symbol Query Protocol), so serving a PDB is
//! one indexed lookup and one object read.
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    package::{child, text},
    registry,
    version::{NuGetVersion, package_id},
};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use object_store::path::Path as ObjectPath;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{Cursor, Read},
};
use uuid::Uuid;

const MAX_ENTRIES: usize = 10_000;
/// PDBs are held in memory while a push is processed, so the total is bounded
/// well below the 512 MiB allowed for ordinary packages.
const MAX_UNCOMPRESSED: u64 = 256 * 1024 * 1024;
/// Extensions nuget.org accepts in a symbol package.
const ALLOWED_EXTENSIONS: &[&str] = &["pdb", "nuspec", "xml", "psmdcp", "rels", "p7s"];
/// Portable PDBs use this literal age in their symbol key.
const PORTABLE_AGE: &str = "ffffffff";

pub struct SymbolFile {
    /// Lowercase file name without directories.
    pub name: String,
    pub key: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}
pub struct ParsedSymbols {
    pub id: String,
    pub display_id: String,
    pub version: NuGetVersion,
    pub files: Vec<SymbolFile>,
}

fn le_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn le_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Computes the symbol server key of a portable PDB: the PDB ID's GUID rendered
/// as a .NET `Guid` (mixed-endian fields), followed by the literal portable age.
pub fn symbol_key(pdb: &[u8]) -> Result<String, &'static str> {
    const NOT_PORTABLE: &str = "Only portable PDBs are supported";
    if pdb.starts_with(b"Microsoft C/C++ MSF") {
        return Err("Windows PDBs are not supported; build with <DebugType>portable</DebugType>");
    }
    // ECMA-335 II.24.2.1 metadata root, followed by the stream directory.
    if le_u32(pdb, 0) != Some(0x424A_5342) {
        return Err(NOT_PORTABLE);
    }
    let version_length = le_u32(pdb, 12).ok_or(NOT_PORTABLE)? as usize;
    let mut at = 16usize
        .checked_add(version_length)
        .and_then(|v| v.checked_add(2))
        .ok_or(NOT_PORTABLE)?;
    let streams = le_u16(pdb, at).ok_or(NOT_PORTABLE)?;
    at += 2;
    for _ in 0..streams.min(32) {
        let offset = le_u32(pdb, at).ok_or(NOT_PORTABLE)? as usize;
        let size = le_u32(pdb, at + 4).ok_or(NOT_PORTABLE)? as usize;
        let name_start = at + 8;
        let name_length = pdb
            .get(name_start..)
            .and_then(|rest| rest.iter().position(|b| *b == 0))
            .ok_or(NOT_PORTABLE)?;
        let name = &pdb[name_start..name_start + name_length];
        if name == b"#Pdb" {
            let id = pdb
                .get(offset..offset.checked_add(size).ok_or(NOT_PORTABLE)?)
                .filter(|stream| stream.len() >= 20)
                .ok_or("The PDB has a malformed #Pdb stream")?;
            return Ok(format!(
                "{:08x}{:04x}{:04x}{}{PORTABLE_AGE}",
                le_u32(id, 0).ok_or(NOT_PORTABLE)?,
                le_u16(id, 4).ok_or(NOT_PORTABLE)?,
                le_u16(id, 6).ok_or(NOT_PORTABLE)?,
                hex::encode(&id[8..16]),
            ));
        }
        // Stream names are NUL-terminated and padded to four bytes.
        at = name_start + (name_length + 1).div_ceil(4) * 4;
    }
    Err("The PDB has no #Pdb stream; only portable PDBs are supported")
}

fn leaf(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or_default()
}
fn extension(name: &str) -> String {
    leaf(name)
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Validates a symbol package and extracts its PDBs. CPU-bound; call on a
/// blocking worker.
pub fn parse(bytes: &Bytes) -> ApiResult<ParsedSymbols> {
    let bad = |message: String| ApiError::bad(message);
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| ApiError::bad("Upload a valid .snupkg ZIP archive"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(ApiError::bad("Symbol package contains too many entries"));
    }
    let (mut manifest, mut pdbs, mut total) = (None, Vec::new(), 0u64);
    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|_| ApiError::bad("Invalid ZIP entry"))?;
        let name = file.name().to_owned();
        if file.is_dir() {
            continue;
        }
        if name.starts_with(['/', '\\'])
            || name
                .split(['/', '\\'])
                .any(|part| part == ".." || part.contains(':'))
        {
            return Err(bad(format!("Unsafe entry path '{name}'")));
        }
        let extension = extension(&name);
        if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
            return Err(bad(format!(
                "Symbol packages may only contain .pdb, .nuspec, .xml, .psmdcp, .rels and .p7s files (found '{name}')"
            )));
        }
        total = total
            .checked_add(file.size())
            .filter(|t| *t <= MAX_UNCOMPRESSED)
            .ok_or_else(|| ApiError::bad("Uncompressed symbol package exceeds 256 MiB"))?;
        if extension == "nuspec" && !name.contains(['/', '\\']) {
            if manifest.replace(i).is_some() {
                return Err(ApiError::bad(
                    "Symbol package must contain exactly one root .nuspec",
                ));
            }
        } else if extension == "pdb" {
            pdbs.push(i);
        }
    }
    let mut nuspec = Vec::new();
    {
        let mut file = archive
            .by_index(manifest.ok_or_else(|| ApiError::bad("Symbol package has no root .nuspec"))?)
            .map_err(|_| ApiError::bad("Invalid manifest entry"))?;
        if file.size() > 1024 * 1024 {
            return Err(ApiError::bad("Manifest exceeds 1 MiB"));
        }
        (&mut file)
            .take(1024 * 1024 + 1)
            .read_to_end(&mut nuspec)
            .map_err(|_| ApiError::bad("Cannot read package manifest"))?;
    }
    let xml = std::str::from_utf8(&nuspec).map_err(|_| ApiError::bad("Manifest must be UTF-8"))?;
    let doc = roxmltree::Document::parse(xml)
        .map_err(|_| ApiError::bad("Invalid nuspec XML (DTDs are not supported)"))?;
    if doc.root_element().tag_name().name() != "package" {
        return Err(ApiError::bad("Invalid nuspec root"));
    }
    let metadata = child(doc.root_element(), "metadata")
        .ok_or_else(|| ApiError::bad("Manifest has no metadata"))?;
    let display_id = text(metadata, "id");
    let id = package_id(&display_id)?;
    let version = NuGetVersion::parse(&text(metadata, "version"))?;
    let is_symbols = child(metadata, "packageTypes").is_some_and(|types| {
        types.children().any(|t| {
            t.is_element()
                && t.tag_name().name() == "packageType"
                && t.attribute("name")
                    .is_some_and(|n| n.eq_ignore_ascii_case("SymbolsPackage"))
        })
    });
    if !is_symbols {
        return Err(ApiError::bad(
            "The manifest must declare the SymbolsPackage package type; this does not look like a .snupkg",
        ));
    }
    // SDK-built symbol packages carry empty `<group>` elements; only real entries are rejected.
    if child(metadata, "dependencies").is_some_and(|d| {
        d.descendants()
            .any(|n| n.is_element() && n.tag_name().name() == "dependency")
    }) {
        return Err(ApiError::bad("Symbol packages cannot declare dependencies"));
    }
    if pdbs.is_empty() {
        return Err(ApiError::bad("Symbol package contains no .pdb files"));
    }
    let mut files: Vec<SymbolFile> = Vec::new();
    for i in pdbs {
        let mut entry = archive
            .by_index(i)
            .map_err(|_| ApiError::bad("Invalid ZIP entry"))?;
        let path = entry.name().to_owned();
        let name = leaf(&path).to_ascii_lowercase();
        if name.len() > 255 || name.chars().any(char::is_control) {
            return Err(bad(format!("Invalid PDB file name '{path}'")));
        }
        let mut data = Vec::with_capacity(entry.size().min(MAX_UNCOMPRESSED) as usize);
        (&mut entry)
            .take(MAX_UNCOMPRESSED + 1)
            .read_to_end(&mut data)
            .map_err(|_| bad(format!("Cannot read '{path}'")))?;
        if data.len() as u64 > MAX_UNCOMPRESSED {
            return Err(ApiError::bad("Uncompressed symbol package exceeds 256 MiB"));
        }
        let key = symbol_key(&data).map_err(|reason| bad(format!("{path}: {reason}")))?;
        let sha256 = hex::encode(Sha256::digest(&data));
        match files.iter().find(|f| f.name == name && f.key == key) {
            // The same PDB listed under several target frameworks is stored once.
            Some(existing) if existing.sha256 == sha256 => {}
            Some(_) => {
                return Err(bad(format!(
                    "{path}: two different PDBs share the same name and ID"
                )));
            }
            None => files.push(SymbolFile {
                name,
                key,
                bytes: data,
                sha256,
            }),
        }
    }
    Ok(ParsedSymbols {
        id,
        display_id,
        version,
        files,
    })
}

fn conflict(message: &str) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}

/// `PUT /api/v2/symbolpackage`, advertised as `SymbolPackagePublish/4.9.0`.
pub async fn push(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let identity = auth::authenticate(&state, request.headers()).await?;
    identity.require("package:push")?;
    let bytes = registry::read_upload(&state, request).await?;
    let parsed = tokio::task::spawn_blocking(move || parse(&bytes))
        .await
        .map_err(ApiError::internal)??;
    identity.package(&parsed.id)?;
    let version = parsed.version.normalized.clone();
    let mut tx = state.db.begin().await?;
    // The same lock package push takes, so ownership and versions cannot change underneath us.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&parsed.id)
        .execute(&mut *tx)
        .await?;
    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT p.owner_id FROM package_versions v JOIN packages p ON p.id=v.package_id WHERE v.package_id=$1 AND v.version=$2",
    )
    .bind(&parsed.id)
    .bind(&version)
    .fetch_optional(&mut *tx)
    .await?;
    let owner = owner.ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            "Push the package before its symbol package; no package has this ID and version".into(),
        )
    })?;
    if owner != identity.profile_id {
        return Err(ApiError::forbidden("Package belongs to another profile"));
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM symbol_packages WHERE package_id=$1 AND version=$2)",
    )
    .bind(&parsed.id)
    .bind(&version)
    .fetch_one(&mut *tx)
    .await?;
    if exists {
        return Err(conflict(
            "Symbols for this version already exist; delete them first to replace them",
        ));
    }
    let mut written: Vec<(String, &SymbolFile)> = Vec::new();
    let outcome = async {
        for file in &parsed.files {
            let key = format!("{}/{}/{}.pdb", parsed.id, version, Uuid::new_v4());
            tokio::time::timeout(
                std::time::Duration::from_secs(60),
                state
                    .storage
                    .put(&ObjectPath::from(key.as_str()), file.bytes.clone().into()),
            )
            .await
            .map_err(|_| {
                ApiError(
                    StatusCode::GATEWAY_TIMEOUT,
                    "Storage upload timed out".into(),
                )
            })??;
            written.push((key, file));
        }
        sqlx::query("INSERT INTO symbol_packages(package_id,version) VALUES($1,$2)")
            .bind(&parsed.id)
            .bind(&version)
            .execute(&mut *tx)
            .await?;
        for (key, file) in &written {
            sqlx::query("INSERT INTO symbol_files(package_id,version,file_name,symbol_key,storage_key,size,sha256) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(&parsed.id)
                .bind(&version)
                .bind(&file.name)
                .bind(&file.key)
                .bind(key)
                .bind(file.bytes.len() as i64)
                .bind(&file.sha256)
                .execute(&mut *tx)
                .await?;
        }
        Ok::<_, ApiError>(())
    }
    .await;
    if let Err(error) = outcome {
        // Best effort: blobs that never became visible are removed now, and
        // orphan cleanup collects any that this misses.
        for (key, _) in &written {
            let _ = state.storage.delete(&ObjectPath::from(key.as_str())).await;
        }
        return Err(error);
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        axum::Json(json!({"id":parsed.display_id,"version":parsed.version.display,"symbols":parsed.files.len()})),
    )
        .into_response())
}

/// `GET|HEAD /symbols/{file}/{key}/{file}`: the Simple Symbol Query Protocol.
pub async fn download(
    State(state): State<AppState>,
    Path((file, key, file_again)): Path<(String, String, String)>,
    method: Method,
) -> ApiResult<Response> {
    let (file, key) = (file.to_ascii_lowercase(), key.to_ascii_lowercase());
    if file != file_again.to_ascii_lowercase()
        || !file.ends_with(".pdb")
        || key.len() != 40
        || !key.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(ApiError::not_found());
    }
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT f.package_id,f.storage_key,f.size FROM symbol_files f JOIN symbol_packages s USING (package_id,version) WHERE f.file_name=$1 AND f.symbol_key=$2 ORDER BY s.pushed_at,f.package_id LIMIT 20",
    )
    .bind(&file)
    .bind(&key)
    .fetch_all(&state.db)
    .await?;
    // Same visibility rule as package reads: a key limited to a prefix only sees its own packages.
    let (_, storage_key, size) = rows
        .into_iter()
        .find(|(package, _, _)| crate::feeds::check_package(&state, package).is_ok())
        .ok_or_else(ApiError::not_found)?;
    let path = ObjectPath::from(storage_key.as_str());
    let mut response = if method == Method::HEAD {
        state.storage.head(&path).await?;
        Body::empty().into_response()
    } else {
        Body::from_stream(state.storage.get(&path).await?.into_stream()).into_response()
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    headers.insert(header::CONTENT_LENGTH, size.to_string().parse().unwrap());
    // A key identifies exactly one PDB, so public symbols never change.
    headers.insert(
        header::CACHE_CONTROL,
        if state.feed.is_none() {
            "public, max-age=31536000, immutable"
        } else {
            "private, max-age=3600"
        }
        .parse()
        .unwrap(),
    );
    Ok(response)
}

/// `DELETE /api/packages/{id}/{version}/symbols`: lets an owner correct a wrong
/// symbol package. Blobs go through the delayed storage cleanup queue so a
/// debugger that is mid-download is not cut off.
pub async fn delete(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let id = package_id(&id)?;
    let version = NuGetVersion::parse(&version)?.normalized;
    let identity = if auth::is_admin(&state, &headers) {
        None
    } else {
        let who = auth::authenticate(&state, &headers).await?;
        who.require("package:unlist")?;
        who.package(&id)?;
        Some(who)
    };
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM packages WHERE id=$1")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if identity.is_some_and(|who| who.profile_id != owner) {
        return Err(ApiError::forbidden("Package belongs to another profile"));
    }
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT storage_key FROM symbol_files WHERE package_id=$1 AND version=$2",
    )
    .bind(&id)
    .bind(&version)
    .fetch_all(&mut *tx)
    .await?;
    let removed = sqlx::query("DELETE FROM symbol_packages WHERE package_id=$1 AND version=$2")
        .bind(&id)
        .bind(&version)
        .execute(&mut *tx)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    for key in keys {
        sqlx::query("INSERT INTO storage_cleanup(storage_key) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(key)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Per-version symbol counts for a package, keyed by normalized version.
pub async fn summary(state: &AppState, id: &str) -> ApiResult<HashMap<String, Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT version,count(*),COALESCE(sum(size),0)::bigint FROM symbol_files WHERE package_id=$1 GROUP BY version",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, files, bytes)| (version, json!({"files":files,"bytes":bytes})))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds the smallest structure `symbol_key` reads: a metadata root with
    /// one `#Pdb` stream holding the given 20-byte PDB ID.
    pub fn portable_pdb(id: [u8; 20]) -> Vec<u8> {
        let version = b"PDB v1.0\0\0\0\0";
        let mut root = Vec::new();
        root.extend(0x424A_5342u32.to_le_bytes());
        root.extend([1, 0, 1, 0]);
        root.extend(0u32.to_le_bytes());
        root.extend((version.len() as u32).to_le_bytes());
        root.extend(version);
        root.extend(0u16.to_le_bytes());
        root.extend(1u16.to_le_bytes());
        let directory_len = 8 + 8; // offset, size, "#Pdb\0" padded to 8
        let offset = (root.len() + directory_len) as u32;
        root.extend(offset.to_le_bytes());
        root.extend(24u32.to_le_bytes());
        root.extend(b"#Pdb\0\0\0\0");
        root.extend(id);
        root.extend([0, 0, 0, 0]);
        root
    }
    fn id(seed: u8) -> [u8; 20] {
        let mut id = [0u8; 20];
        for (i, b) in id.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8);
        }
        id
    }
    fn snupkg(files: &[(&str, &[u8])], nuspec: &str) -> Bytes {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("Acme.Json.nuspec", options).unwrap();
        writer.write_all(nuspec.as_bytes()).unwrap();
        for (name, data) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        Bytes::from(writer.finish().unwrap().into_inner())
    }
    const NUSPEC: &str = "<package><metadata><id>Acme.Json</id><version>1.0.0</version><packageTypes><packageType name=\"SymbolsPackage\"/></packageTypes></metadata></package>";

    #[test]
    fn key_uses_dotnet_guid_byte_order_and_the_portable_age() {
        // First 16 bytes 00..0f as a .NET Guid: 03020100-0504-0706-0809-0a0b0c0d0e0f.
        let pdb = portable_pdb(id(0));
        assert_eq!(
            symbol_key(&pdb).unwrap(),
            "03020100050407060809\
             0a0b0c0d0e0fffffffff"
        );
    }
    #[test]
    fn rejects_windows_and_malformed_pdbs() {
        assert!(
            symbol_key(b"Microsoft C/C++ MSF 7.00\r\n\x1aDS")
                .unwrap_err()
                .contains("Windows")
        );
        assert!(symbol_key(b"not a pdb").is_err());
        let mut truncated = portable_pdb(id(1));
        truncated.truncate(40);
        assert!(symbol_key(&truncated).is_err());
        let mut no_pdb_stream = portable_pdb(id(1));
        no_pdb_stream[40..44].copy_from_slice(b"#Xyz");
        assert!(symbol_key(&no_pdb_stream).is_err());
    }
    #[test]
    fn extracts_pdbs_and_dedupes_identical_target_frameworks() {
        let pdb = portable_pdb(id(7));
        // The .NET SDK emits empty dependency groups in symbol packages.
        let sdk_nuspec = NUSPEC.replace(
            "</metadata>",
            "<dependencies><group targetFramework=\"net8.0\" /></dependencies></metadata>",
        );
        assert!(parse(&snupkg(&[("a.pdb", &pdb)], &sdk_nuspec)).is_ok());
        let package = snupkg(
            &[
                ("lib/net8.0/Acme.Json.pdb", &pdb),
                ("lib/net6.0/Acme.Json.pdb", &pdb),
                ("lib/net6.0/Other.PDB", &portable_pdb(id(9))),
            ],
            NUSPEC,
        );
        let parsed = parse(&package).unwrap();
        assert_eq!(
            (parsed.id.as_str(), parsed.version.normalized.as_str()),
            ("acme.json", "1.0.0")
        );
        let names: Vec<_> = parsed.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["acme.json.pdb", "other.pdb"]);
    }
    #[test]
    fn rejects_invalid_symbol_packages() {
        let pdb = portable_pdb(id(3));
        let reject = |package: Bytes, expected: &str| {
            let error = parse(&package).err().expect("package must be rejected");
            assert!(
                error.1.contains(expected),
                "{} should contain {expected}",
                error.1
            );
        };
        reject(snupkg(&[("lib/a.dll", b"MZ")], NUSPEC), "may only contain");
        reject(snupkg(&[("../evil.pdb", &pdb)], NUSPEC), "Unsafe");
        reject(snupkg(&[], NUSPEC), "no .pdb");
        reject(
            snupkg(
                &[("a.pdb", &pdb)],
                "<package><metadata><id>Acme.Json</id><version>1.0.0</version></metadata></package>",
            ),
            "SymbolsPackage",
        );
        reject(
            snupkg(
                &[("a.pdb", &pdb)],
                "<package><metadata><id>Acme.Json</id><version>1.0.0</version><packageTypes><packageType name=\"SymbolsPackage\"/></packageTypes><dependencies><dependency id=\"X\" version=\"1.0.0\"/></dependencies></metadata></package>",
            ),
            "dependencies",
        );
        reject(snupkg(&[("a.pdb", b"junk")], NUSPEC), "portable");
        let mut other = portable_pdb(id(3));
        other.extend([1, 2, 3]);
        reject(
            snupkg(&[("x/a.pdb", &pdb), ("y/a.pdb", &other)], NUSPEC),
            "same name and ID",
        );
        reject(Bytes::from_static(b"not a zip"), "valid .snupkg");
    }
}
