use super::{
    http,
    sources::{self, Reader, SELECT_SOURCE, Source},
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    package,
    version::{NuGetVersion, package_id},
};
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone, FromRow)]
pub struct Metadata {
    pub package_id: String,
    pub source_id: Uuid,
    pub versions: Value,
    pub catalog: Option<Value>,
    pub expires_at: DateTime<Utc>,
    pub catalog_checked_at: Option<DateTime<Utc>>,
}
#[derive(Clone, FromRow)]
pub struct Cached {
    pub package_id: String,
    pub version: String,
    pub catalog: Value,
    pub nuspec: Vec<u8>,
    pub storage_key: String,
    pub size: i64,
    pub sha512: String,
    pub downloads: i64,
    pub cached_at: DateTime<Utc>,
}
const META: &str = "SELECT * FROM upstream_metadata WHERE package_id=$1";
const CACHE: &str = "SELECT * FROM upstream_cache WHERE package_id=$1 AND version=$2";
pub async fn local_exists(state: &AppState, id: &str) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM packages WHERE id=$1)")
            .bind(id)
            .fetch_one(&state.db)
            .await?,
    )
}
fn choose(mut candidates: Vec<Source>, pinned: Option<Uuid>) -> ApiResult<Vec<Source>> {
    if let Some(id) = pinned {
        candidates.retain(|s| s.id == id);
        if candidates.is_empty() {
            return Err(ApiError(StatusCode::CONFLICT,"Pinned upstream no longer matches the routing policy; clear its cache or restore its settings".into()));
        }
    }
    if candidates.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(candidates)
}
pub async fn pinned(state: &AppState, reader: &Reader, id: &str) -> ApiResult<(Metadata, Source)> {
    let meta = sqlx::query_as::<_, Metadata>(META)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let source = choose(sources::candidates(state, id).await?, Some(meta.source_id))?.remove(0);
    reader.check(&source, id)?;
    Ok((meta, source))
}
async fn source_lock(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended($1,0))")
        .bind(format!("source:{id}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub async fn metadata(
    state: &AppState,
    reader: &Reader,
    input: &str,
) -> ApiResult<(Metadata, Source)> {
    let id = package_id(input)?;
    if local_exists(state, &id).await? {
        return Err(ApiError::not_found());
    }
    if let Ok((meta, source)) = pinned(state, reader, &id).await
        && meta.expires_at > Utc::now()
    {
        return Ok((meta, source));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='180s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM packages WHERE id=$1)")
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
    if owned {
        return Err(ApiError::not_found());
    }
    let old = sqlx::query_as::<_, Metadata>(META)
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?;
    let rows = sqlx::query_as::<_, Source>(&format!(
        "{SELECT_SOURCE} WHERE enabled ORDER BY priority,id"
    ))
    .fetch_all(&mut *tx)
    .await?;
    for source in choose(
        sources::select_candidates(rows, &id),
        old.as_ref().map(|m| m.source_id),
    )? {
        reader.check(&source, &id)?;
        source_lock(&mut tx, source.id).await?;
        if let Some(meta) = &old
            && meta.expires_at > Utc::now()
        {
            return Ok((meta.clone(), source));
        }
        let missed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upstream_misses WHERE source_id=$1 AND package_id=$2 AND expires_at>now())").bind(source.id).bind(&id).fetch_one(&mut *tx).await?;
        if missed && old.is_none() {
            continue;
        }
        let fetched = async {
            let index = http::index(&source).await?;
            let base = http::resource_for(state, &index, "PackageBaseAddress")?;
            let versions = http::json(&source, &format!("{base}{id}/index.json")).await?;
            Ok::<_, ApiError>((index, versions))
        }
        .await;
        let (index, result) = match fetched {
            Ok(value) => value,
            Err(e) if source.stale_on_error && http::transient(&e) && old.is_some() => {
                let mut meta = old.clone().unwrap();
                let available: Vec<String> = sqlx::query_scalar(
                    "SELECT version FROM upstream_cache WHERE package_id=$1 ORDER BY version_sort",
                )
                .bind(&id)
                .fetch_all(&mut *tx)
                .await?;
                if !available.is_empty() {
                    meta.versions = json!(available);
                }
                return Ok((meta, source));
            }
            Err(e) => return Err(e),
        };
        let Some(result) = result else {
            if let Some(meta) = &old {
                return Ok((meta.clone(), source));
            }
            sqlx::query("INSERT INTO upstream_misses(source_id,package_id,expires_at) VALUES($1,$2,now()+interval '60 seconds') ON CONFLICT(source_id,package_id) DO UPDATE SET expires_at=EXCLUDED.expires_at").bind(source.id).bind(&id).execute(&mut *tx).await?;
            continue;
        };
        let list = result["versions"]
            .as_array()
            .ok_or_else(|| http::failure("Invalid upstream version index"))?;
        if list.is_empty() || list.len() > 10000 {
            return Err(http::failure(
                "Upstream version index is empty or too large",
            ));
        }
        let mut versions = Vec::new();
        for value in list {
            let parsed = NuGetVersion::parse(
                value
                    .as_str()
                    .ok_or_else(|| http::failure("Invalid upstream version"))?,
            )
            .map_err(|_| http::failure("Invalid upstream version"))?;
            versions.push((parsed.sort_key, parsed.normalized));
        }
        versions.sort();
        versions.dedup_by(|a, b| a.1 == b.1);
        let versions = json!(versions.into_iter().map(|(_, v)| v).collect::<Vec<_>>());
        let expiry = Utc::now() + Duration::seconds(source.metadata_ttl_seconds as i64);
        sqlx::query("INSERT INTO upstream_metadata(package_id,source_id,versions,expires_at) VALUES($1,$2,$3,$4) ON CONFLICT(package_id) DO UPDATE SET versions=EXCLUDED.versions,checked_at=now(),expires_at=EXCLUDED.expires_at")
            .bind(&id).bind(source.id).bind(&versions).bind(expiry).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE upstream_sources SET service_index=$2,index_checked_at=now() WHERE id=$1",
        )
        .bind(source.id)
        .bind(&index)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM upstream_misses WHERE source_id=$1 AND package_id=$2")
            .bind(source.id)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok((
            Metadata {
                package_id: id,
                source_id: source.id,
                versions,
                catalog: old.and_then(|m| m.catalog),
                expires_at: expiry,
                catalog_checked_at: None,
            },
            source,
        ));
    }
    // Persist negative hits even when every candidate returned 404.
    tx.commit().await?;
    Err(ApiError::not_found())
}
pub fn content(state: &AppState, id: &str, version: &str) -> String {
    format!(
        "{}/v3/flatcontainer/{id}/{version}/{id}.{version}.nupkg",
        state.config.public_url
    )
}
pub fn clean_catalog(state: &AppState, id: &str, entry: &Value) -> ApiResult<Value> {
    let version = NuGetVersion::parse(
        entry["version"]
            .as_str()
            .ok_or_else(|| http::failure("Registration entry has no version"))?,
    )
    .map_err(|_| http::failure("Invalid registration version"))?;
    if package_id(entry["id"].as_str().unwrap_or(id))
        .map_err(|_| http::failure("Invalid registration ID"))?
        != id
    {
        return Err(http::failure("Registration package ID does not match"));
    }
    let mut result = json!({"id":entry["id"].as_str().unwrap_or(id),"version":version.display,"@id":format!("{}/v3/registration/{id}/{}.json",state.config.public_url,version.normalized),"packageContent":content(state,id,&version.normalized),"listed":entry["listed"].as_bool().unwrap_or(true)});
    for key in [
        "authors",
        "description",
        "title",
        "tags",
        "published",
        "licenseExpression",
        "language",
        "minClientVersion",
        "requireLicenseAcceptance",
    ] {
        if let Some(value) = entry.get(key) {
            result[key] = value.clone();
        }
    }
    if let Some(raw) = entry.get("deprecation")
        && let Ok(input) = serde_json::from_value::<crate::deprecation::Input>(raw.clone())
        && let Ok(value) = crate::deprecation::value(input)
    {
        result["deprecation"] = value;
    }
    for key in ["projectUrl", "licenseUrl"] {
        if let Some(value) = entry[key].as_str()
            && url::Url::parse(value).is_ok_and(|u| {
                matches!(u.scheme(), "https" | "http")
                    && u.username().is_empty()
                    && u.password().is_none()
            })
        {
            result[key] = json!(value);
        }
    }
    if let Some(tags) = result["tags"].as_str() {
        result["tags"] = json!(tags.split_whitespace().collect::<Vec<_>>());
    }
    if let Some(groups) = entry["dependencyGroups"].as_array() {
        let mut groups = groups.clone();
        for group in &mut groups {
            if let Some(deps) = group["dependencies"].as_array_mut() {
                for dep in deps {
                    let dep_id = package_id(dep["id"].as_str().unwrap_or_default())
                        .map_err(|_| http::failure("Invalid upstream dependency"))?;
                    dep["registration"] = json!(format!(
                        "{}/v3/registration/{dep_id}/index.json",
                        state.config.public_url
                    ));
                }
            }
        }
        result["dependencyGroups"] = json!(groups);
    }
    Ok(result)
}
pub async fn catalog(
    state: &AppState,
    reader: &Reader,
    input: &str,
) -> ApiResult<(Vec<Value>, Source)> {
    let (meta, source) = metadata(state, reader, input).await?;
    if let Some(entries) = &meta.catalog
        && meta
            .catalog_checked_at
            .is_some_and(|t| t + Duration::seconds(source.metadata_ttl_seconds as i64) > Utc::now())
    {
        return Ok((entries.as_array().cloned().unwrap_or_default(), source));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='180s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("upstream:catalog:{}", meta.package_id))
        .execute(&mut *tx)
        .await?;
    source_lock(&mut tx, source.id).await?;
    let current = sqlx::query_as::<_, Metadata>(META)
        .bind(&meta.package_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(entries) = &current.catalog
        && current
            .catalog_checked_at
            .is_some_and(|t| t + Duration::seconds(source.metadata_ttl_seconds as i64) > Utc::now())
    {
        return Ok((entries.as_array().cloned().unwrap_or_default(), source));
    }
    let fetched = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let index = http::index(&source).await?;
        let base = http::resource_for(state, &index, "RegistrationsBaseUrl")?;
        let document = http::json(&source, &format!("{base}{}/index.json", meta.package_id))
            .await?
            .ok_or_else(ApiError::not_found)?;
        let pages = document["items"]
            .as_array()
            .ok_or_else(|| http::failure("Invalid registration index"))?;
        if pages.len() > 128 {
            return Err(http::failure("Registration has too many pages"));
        }
        let mut entries = Vec::new();
        let mut size = 0;
        for page in pages {
            let page = if page["items"].is_array() {
                page.clone()
            } else {
                http::json(
                    &source,
                    page["@id"]
                        .as_str()
                        .ok_or_else(|| http::failure("Missing registration page URL"))?,
                )
                .await?
                .ok_or_else(|| http::failure("Registration page missing"))?
            };
            for leaf in page["items"]
                .as_array()
                .ok_or_else(|| http::failure("Invalid registration page"))?
            {
                let entry = clean_catalog(state, &meta.package_id, &leaf["catalogEntry"])?;
                size += entry.to_string().len();
                if size > http::METADATA_LIMIT || entries.len() >= 10000 {
                    return Err(http::failure("Registration metadata exceeds its limit"));
                }
                entries.push(entry);
            }
        }
        entries.sort_by_key(|e| {
            NuGetVersion::parse(e["version"].as_str().unwrap())
                .unwrap()
                .sort_key
        });
        if entries.is_empty() {
            return Err(ApiError::not_found());
        }
        Ok::<_, ApiError>(entries)
    })
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::GATEWAY_TIMEOUT,
            "Registration fetch timed out".into(),
        )
    })?;
    let entries = match fetched {
        Ok(entries) => entries,
        Err(e) if source.stale_on_error && http::transient(&e) && meta.catalog.is_some() => {
            return Ok((
                meta.catalog
                    .unwrap()
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
                source,
            ));
        }
        Err(e) => return Err(e),
    };
    sqlx::query(
        "UPDATE upstream_metadata SET catalog=$2,catalog_checked_at=now() WHERE package_id=$1",
    )
    .bind(&meta.package_id)
    .bind(json!(entries))
    .execute(&mut *tx)
    .await?;
    // Keep discovery visibility synchronized for already cached archives.
    for entry in &entries {
        let version = NuGetVersion::parse(entry["version"].as_str().unwrap())
            .unwrap()
            .normalized;
        sqlx::query("UPDATE upstream_cache SET catalog=$3 WHERE package_id=$1 AND version=$2")
            .bind(&meta.package_id)
            .bind(version)
            .bind(entry)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("SELECT refresh_upstream_search($1)")
        .bind(&meta.package_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((entries, source))
}
pub async fn ensure(
    state: &AppState,
    reader: &Reader,
    input: &str,
    input_version: &str,
) -> ApiResult<(Cached, Source, bool)> {
    let id = package_id(input)?;
    let version = NuGetVersion::parse(input_version)?.normalized;
    if let Some(cached) = sqlx::query_as::<_, Cached>(CACHE)
        .bind(&id)
        .bind(&version)
        .fetch_optional(&state.db)
        .await?
    {
        let (_, source) = pinned(state, reader, &id).await?;
        return Ok((cached, source, true));
    }
    let (meta, source) = metadata(state, reader, &id).await?;
    if !meta
        .versions
        .as_array()
        .is_some_and(|v| v.contains(&json!(version)))
    {
        return Err(ApiError::not_found());
    }
    let (catalog_entries, _) = catalog(state, reader, &id).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='180s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("upstream:archive:{id}/{version}"))
        .execute(&mut *tx)
        .await?;
    source_lock(&mut tx, source.id).await?;
    let still_pinned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM upstream_metadata WHERE package_id=$1 AND source_id=$2)",
    )
    .bind(&id)
    .bind(source.id)
    .fetch_one(&mut *tx)
    .await?;
    if !still_pinned {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cache metadata changed; retry the request".into(),
        ));
    }
    if let Some(cached) = sqlx::query_as::<_, Cached>(CACHE)
        .bind(&id)
        .bind(&version)
        .fetch_optional(&mut *tx)
        .await?
    {
        return Ok((cached, source, true));
    }
    let _slot = state
        .upload_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Cache downloads are busy; retry shortly".into(),
            )
        })?;
    let index = http::index(&source).await?;
    let base = http::resource_for(state, &index, "PackageBaseAddress")?;
    let url = format!("{base}{id}/{version}/{id}.{version}.nupkg");
    let bytes = http::bytes(&source, &url, state.config.max_package_bytes)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let digest = Sha512::digest(&bytes);
    let sha512 = STANDARD.encode(digest);
    if let Some(expected) = http::bytes(&source, &format!("{url}.sha512"), 1024).await? {
        let expected = std::str::from_utf8(&expected)
            .map_err(|_| http::failure("Invalid upstream checksum"))?
            .trim();
        if STANDARD
            .decode(expected)
            .map_err(|_| http::failure("Invalid upstream checksum"))?
            .as_slice()
            != digest.as_slice()
        {
            return Err(http::failure("Upstream package checksum does not match"));
        }
    }
    let data = bytes.clone();
    let parsed = tokio::task::spawn_blocking(move || package::parse(&data))
        .await
        .map_err(ApiError::internal)?
        .map_err(|_| http::failure("Invalid upstream package archive"))?;
    if parsed.id != id || parsed.version.normalized != version {
        return Err(http::failure(
            "Upstream archive ID/version does not match the request",
        ));
    }
    let entry = json!({"id":parsed.display_id,"version":parsed.version.display,"title":parsed.title,"description":parsed.description,"authors":parsed.authors,"tags":parsed.tags.split_whitespace().collect::<Vec<_>>(),"projectUrl":parsed.project_url,"licenseExpression":parsed.license_expression,"dependencyGroups":parsed.dependencies,"listed":true,"published":Utc::now()});
    let entry = clean_catalog(state, &id, &entry)?;
    let entry = catalog_entries
        .iter()
        .find(|e| {
            NuGetVersion::parse(e["version"].as_str().unwrap_or_default())
                .is_ok_and(|v| v.normalized == version)
        })
        .cloned()
        .unwrap_or_else(|| {
            let mut entry = entry;
            entry["listed"] = json!(false);
            entry
        });
    let storage_key = format!(
        "upstream/{}/{id}/{version}/{}.nupkg",
        source.id,
        Uuid::new_v4()
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.storage.put(
            &object_store::path::Path::from(storage_key.as_str()),
            bytes.clone().into(),
        ),
    )
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::GATEWAY_TIMEOUT,
            "Cache storage write timed out".into(),
        )
    })??;
    sqlx::query("INSERT INTO upstream_cache(package_id,version,version_sort,prerelease,semver2,title,description,authors,tags,catalog,nuspec,storage_key,size,sha512) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)")
        .bind(&id).bind(&version).bind(parsed.version.sort_key).bind(parsed.version.prerelease).bind(parsed.version.semver2).bind(parsed.title).bind(parsed.description).bind(parsed.authors).bind(parsed.tags).bind(&entry).bind(parsed.nuspec).bind(storage_key).bind(bytes.len() as i64).bind(sha512).execute(&mut *tx).await?;
    sqlx::query("UPDATE upstream_cache SET assets=$3 WHERE package_id=$1 AND version=$2")
        .bind(&id)
        .bind(&version)
        .bind(parsed.assets)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT refresh_upstream_search($1)")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let cached = sqlx::query_as::<_, Cached>(CACHE)
        .bind(&id)
        .bind(&version)
        .fetch_one(&state.db)
        .await?;
    Ok((cached, source, false))
}
