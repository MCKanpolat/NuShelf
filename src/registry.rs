use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    package,
    version::{NuGetVersion, package_id},
};
use axum::{
    Json,
    body::{Body, Bytes},
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use object_store::path::Path as ObjectPath;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(FromRow, Clone)]
pub struct PackageRow {
    pub package_id: String,
    pub display_id: String,
    pub version: String,
    pub display_version: String,
    pub title: String,
    pub description: String,
    pub authors: String,
    pub tags: String,
    pub project_url: Option<String>,
    pub license_expression: Option<String>,
    pub dependencies: Value,
    pub listed: bool,
    pub prerelease: bool,
    pub semver2: bool,
    pub storage_key: String,
    pub size: i64,
    pub sha512: String,
    pub nuspec: Vec<u8>,
    pub downloads: i64,
    pub total_downloads: i64,
    pub published_at: DateTime<Utc>,
    pub owner_name: String,
}
const ROW_SELECT: &str = "SELECT v.*, p.display_id, p.downloads AS total_downloads, o.name AS owner_name FROM package_versions v JOIN packages p ON p.id=v.package_id JOIN profiles o ON o.id=p.owner_id";

pub fn content_url(state: &AppState, p: &PackageRow) -> String {
    format!(
        "{}/v3/flatcontainer/{}/{}/{}.{}.nupkg",
        state.config.public_url, p.package_id, p.version, p.package_id, p.version
    )
}
pub fn catalog(state: &AppState, p: &PackageRow) -> Value {
    let leaf = format!(
        "{}/v3/registration/{}/{}.json",
        state.config.public_url, p.package_id, p.version
    );
    let groups: Vec<Value> = p
        .dependencies
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| {
            let mut g = g.clone();
            if let Some(deps) = g["dependencies"].as_array_mut() {
                for d in deps {
                    if let Some(id) = d["id"].as_str() {
                        d["registration"] = json!(format!(
                            "{}/v3/registration/{}/index.json",
                            state.config.public_url,
                            id.to_ascii_lowercase()
                        ));
                    }
                }
            }
            g
        })
        .collect();
    json!({"@id":leaf,"id":p.display_id,"version":p.display_version,"title":p.title,"description":p.description,"authors":p.authors,"tags":p.tags.split_whitespace().collect::<Vec<_>>(),"listed":p.listed,"published":p.published_at,"projectUrl":p.project_url,"licenseExpression":p.license_expression,"dependencyGroups":groups,"packageContent":content_url(state,p)})
}
fn embedded_leaf(state: &AppState, p: &PackageRow) -> Value {
    json!({"@id":format!("{}/v3/registration/{}/{}.json",state.config.public_url,p.package_id,p.version),"catalogEntry":catalog(state,p),"packageContent":content_url(state,p)})
}
pub async fn service_index(State(state): State<AppState>) -> Json<Value> {
    let base = &state.config.public_url;
    Json(json!({"version":"3.0.0","resources":[
        {"@id":format!("{base}/v3/flatcontainer/"),"@type":"PackageBaseAddress/3.0.0"},
        {"@id":format!("{base}/v3/registration/"),"@type":"RegistrationsBaseUrl/3.6.0"},
        {"@id":format!("{base}/v3/search"),"@type":"SearchQueryService/3.5.0"},
        {"@id":format!("{base}/v3/autocomplete"),"@type":"SearchAutocompleteService/3.5.0"},
        {"@id":format!("{base}/api/v2/package"),"@type":"PackagePublish/2.0.0"},
        {"@id":format!("{base}/packages/{{id}}/{{version}}"),"@type":"PackageDetailsUriTemplate/5.1.0"}
    ]}))
}
pub async fn push(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let identity = auth::authenticate(&state, request.headers()).await?;
    identity.require("package:push")?;
    // NuGet clients send multipart/form-data; raw application/octet-stream is also accepted.
    let multipart = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.starts_with("multipart/form-data"));
    let bytes = if multipart {
        let mut multipart = Multipart::from_request(request, &state)
            .await
            .map_err(|_| ApiError::bad("Invalid multipart upload"))?;
        let field = multipart
            .next_field()
            .await
            .map_err(|_| ApiError::bad("Cannot read upload"))?
            .ok_or_else(|| ApiError::bad("Package file is missing"))?;
        let bytes = field.bytes().await.map_err(|_| {
            ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Upload exceeds the configured limit or is malformed".into(),
            )
        })?;
        if multipart
            .next_field()
            .await
            .map_err(|_| ApiError::bad("Invalid multipart upload"))?
            .is_some()
        {
            return Err(ApiError::bad("Upload exactly one package"));
        }
        bytes
    } else {
        Bytes::from_request(request, &state).await.map_err(|_| {
            ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Upload exceeds the configured limit".into(),
            )
        })?
    };
    if bytes.is_empty() || bytes.len() > state.config.max_package_bytes {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Package is empty or exceeds the configured limit".into(),
        ));
    }
    let parse_bytes = bytes.clone();
    let parsed = tokio::task::spawn_blocking(move || package::parse(&parse_bytes))
        .await
        .map_err(ApiError::internal)??;
    identity.package(&parsed.id)?;
    let mut tx = state.db.begin().await?;
    // Serialize ownership and version changes across replicas, including first publication.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&parsed.id)
        .execute(&mut *tx)
        .await?;
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT owner_id FROM packages WHERE id=$1")
        .bind(&parsed.id)
        .fetch_optional(&mut *tx)
        .await?;
    if owner.is_some_and(|o| o != identity.profile_id) {
        return Err(ApiError::forbidden("Package belongs to another profile"));
    }
    let pinned: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upstream_metadata WHERE package_id=$1)")
            .bind(&parsed.id)
            .fetch_one(&mut *tx)
            .await?;
    if pinned {
        return Err(ApiError(StatusCode::CONFLICT,"This package ID is pinned to an upstream; an administrator must clear that cache before local publication".into()));
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM package_versions WHERE package_id=$1 AND version=$2)",
    )
    .bind(&parsed.id)
    .bind(&parsed.version.normalized)
    .fetch_one(&mut *tx)
    .await?;
    if exists {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This package version already exists; versions are immutable".into(),
        ));
    }
    let storage_key = format!(
        "{}/{}/{}.nupkg",
        parsed.id,
        parsed.version.normalized,
        Uuid::new_v4()
    );
    let digest = STANDARD.encode(Sha512::digest(&bytes));
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        state.storage.put(
            &ObjectPath::from(storage_key.as_str()),
            bytes.clone().into(),
        ),
    )
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::GATEWAY_TIMEOUT,
            "Storage upload timed out".into(),
        )
    })??;
    // A crash between blob upload and commit may leave an orphan blob, never a visible partial package.
    sqlx::query(
        "INSERT INTO packages(id,display_id,owner_id) VALUES($1,$2,$3) ON CONFLICT(id) DO NOTHING",
    )
    .bind(&parsed.id)
    .bind(&parsed.display_id)
    .bind(identity.profile_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO package_versions(package_id,version,display_version,version_sort,prerelease,semver2,title,description,authors,tags,project_url,license_expression,dependencies,nuspec,storage_key,size,sha512) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)")
        .bind(&parsed.id).bind(&parsed.version.normalized).bind(&parsed.version.display).bind(&parsed.version.sort_key).bind(parsed.version.prerelease).bind(parsed.version.semver2)
        .bind(&parsed.title).bind(&parsed.description).bind(&parsed.authors).bind(&parsed.tags).bind(&parsed.project_url).bind(&parsed.license_expression)
        .bind(&parsed.dependencies).bind(&parsed.nuspec).bind(&storage_key).bind(bytes.len() as i64).bind(digest).execute(&mut *tx).await?;
    sqlx::query("SELECT refresh_search($1)")
        .bind(&parsed.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":parsed.display_id,"version":parsed.version.display})),
    )
        .into_response())
}
pub async fn unlist(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    set_listing(&state, &headers, &id, &version, false).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn relist(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    set_listing(&state, &headers, &id, &version, true).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn set_listing(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    version: &str,
    listed: bool,
) -> ApiResult<()> {
    let id = package_id(id)?;
    let version = NuGetVersion::parse(version)?.normalized;
    let identity = if auth::is_admin(state, headers) {
        None
    } else {
        let who = auth::authenticate(state, headers).await?;
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
    let result =
        sqlx::query("UPDATE package_versions SET listed=$3 WHERE package_id=$1 AND version=$2")
            .bind(&id)
            .bind(&version)
            .bind(listed)
            .execute(&mut *tx)
            .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    sqlx::query("SELECT refresh_search($1)")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn versions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = package_id(&id)?;
    let rows: Vec<String> = sqlx::query_scalar("SELECT version FROM package_versions WHERE package_id=$1 ORDER BY version_sort COLLATE \"C\"").bind(id).fetch_all(&state.db).await?;
    if rows.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(Json(json!({"versions":rows})))
}
pub async fn find_version(state: &AppState, id: &str, version: &str) -> ApiResult<PackageRow> {
    let id = package_id(id)?;
    let version = NuGetVersion::parse(version)?.normalized;
    sqlx::query_as::<_, PackageRow>(&format!(
        "{ROW_SELECT} WHERE v.package_id=$1 AND v.version=$2"
    ))
    .bind(id)
    .bind(version)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(ApiError::not_found)
}
pub async fn all_versions(state: &AppState, id: &str) -> ApiResult<Vec<PackageRow>> {
    let id = package_id(id)?;
    let rows = sqlx::query_as::<_, PackageRow>(&format!(
        "{ROW_SELECT} WHERE v.package_id=$1 ORDER BY v.version_sort COLLATE \"C\""
    ))
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(rows)
}
pub async fn download(
    State(state): State<AppState>,
    Path((id, version, file)): Path<(String, String, String)>,
    method: Method,
) -> ApiResult<Response> {
    let p = find_version(&state, &id, &version).await?;
    if file == format!("{}.nuspec", p.package_id) {
        return Ok((
            [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
            p.nuspec,
        )
            .into_response());
    }
    if file == format!("{}.{}.nupkg.sha512", p.package_id, p.version) {
        return Ok(([(header::CONTENT_TYPE, "text/plain")], p.sha512).into_response());
    }
    if file != format!("{}.{}.nupkg", p.package_id, p.version) {
        return Err(ApiError::not_found());
    }
    let object_path = ObjectPath::from(p.storage_key.as_str());
    let mut response = if method == Method::HEAD {
        state.storage.head(&object_path).await?;
        Body::empty().into_response()
    } else {
        let result = state.storage.get(&object_path).await?;
        // Count served package requests; retries count again. HEAD and nuspec requests do not count.
        let mut tx = state.db.begin().await?;
        sqlx::query("UPDATE packages SET downloads=downloads+1 WHERE id=$1")
            .bind(&p.package_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE package_versions SET downloads=downloads+1 WHERE package_id=$1 AND version=$2",
        )
        .bind(&p.package_id)
        .bind(&p.version)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Body::from_stream(result.into_stream()).into_response()
    };
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, p.size.to_string().parse().unwrap());
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "private, max-age=3600".parse().unwrap(),
    );
    Ok(response)
}
pub async fn registration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let rows = all_versions(&state, &id).await?;
    let index_url = format!(
        "{}/v3/registration/{}/index.json",
        state.config.public_url, rows[0].package_id
    );
    let pages: Vec<Value> = rows.chunks(64).map(|chunk| json!({
        "@id":format!("{index_url}#page/{}/{}",chunk[0].version,chunk.last().unwrap().version),"parent":index_url,
        "count":chunk.len(),"lower":chunk[0].version,"upper":chunk.last().unwrap().version,
        "items":chunk.iter().map(|p| embedded_leaf(&state,p)).collect::<Vec<_>>()
    })).collect();
    Ok(Json(
        json!({"@id":index_url,"count":pages.len(),"items":pages}),
    ))
}
pub async fn registration_leaf(
    State(state): State<AppState>,
    Path((id, file)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let version = file.strip_suffix(".json").ok_or_else(ApiError::not_found)?;
    let p = find_version(&state, &id, version).await?;
    Ok(Json(
        json!({"@id":format!("{}/v3/registration/{}/{}.json",state.config.public_url,p.package_id,p.version),"listed":p.listed,"published":p.published_at,"packageContent":content_url(&state,&p),"registration":format!("{}/v3/registration/{}/index.json",state.config.public_url,p.package_id)}),
    ))
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub skip: i64,
    pub take: Option<i64>,
    #[serde(default)]
    pub prerelease: bool,
    pub sem_ver_level: Option<String>,
    pub sort: Option<String>,
    pub id: Option<String>,
}
const SEARCH_FILTER: &str = "d.channel=$1 AND (d.search_vector @@ websearch_to_tsquery('simple',$2) OR d.package_id LIKE $3 ESCAPE '\\')";
const FUZZY_FILTER: &str = "d.channel=$1 AND d.package_id % $2 AND $3::text IS NOT NULL";
const BROWSE_FILTER: &str = "d.channel=$1 AND $2::text IS NOT NULL AND $3::text IS NOT NULL";
fn like_pattern(q: &str) -> String {
    format!(
        "{}%",
        q.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
pub fn channel(query: &SearchQuery) -> i16 {
    (if query.prerelease { 1 } else { 0 })
        + (if query.sem_ver_level.as_deref().is_some_and(|v| {
            NuGetVersion::parse(v)
                .is_ok_and(|v| v.sort_key >= NuGetVersion::parse("2.0.0").unwrap().sort_key)
        }) {
            2
        } else {
            0
        })
}
pub async fn search_value(state: &AppState, query: &SearchQuery) -> ApiResult<Value> {
    let q = query.q.trim().to_ascii_lowercase();
    if q.len() > 256
        || query.skip < 0
        || query.skip > 100_000
        || query.take.is_some_and(|t| !(0..=100).contains(&t))
    {
        return Err(ApiError::bad(
            "Search allows a 256-character query, skip 0–100000 and take 0–100",
        ));
    }
    let channel = channel(query);
    let pattern = like_pattern(&q);
    // REPEATABLE READ keeps the count and page consistent while concurrent pushes commit.
    let mut tx = state.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let mut filter = if q.is_empty() {
        BROWSE_FILTER
    } else {
        SEARCH_FILTER
    };
    let mut total: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM search_documents d WHERE {filter}"
    ))
    .bind(channel)
    .bind(&q)
    .bind(&pattern)
    .fetch_one(&mut *tx)
    .await?;
    // Fuzzy matching is a fallback, so shared ID prefixes do not drown exact results
    // in thousands of similar package names or force trigram work on every search.
    if total == 0 && q.len() >= 3 {
        filter = FUZZY_FILTER;
        total = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM search_documents d WHERE {filter}"
        ))
        .bind(channel)
        .bind(&q)
        .bind(&pattern)
        .fetch_one(&mut *tx)
        .await?;
    }
    let order = match query.sort.as_deref() {
        Some("recent") => "d.published_at DESC, d.package_id",
        Some("downloads") => "p.downloads DESC, d.package_id",
        Some("name") => "d.package_id",
        Some("relevance") | None => {
            "(d.package_id=$2) DESC, ts_rank_cd(d.search_vector,websearch_to_tsquery('simple',$2)) DESC, similarity(d.package_id,$2) DESC, p.downloads DESC, d.package_id"
        }
        _ => {
            return Err(ApiError::bad(
                "Sort must be relevance, recent, downloads or name",
            ));
        }
    };
    let rows: Vec<Value> = sqlx::query_scalar(&format!(r#"
        SELECT jsonb_build_object('id',p.display_id,'version',v.display_version,'title',v.title,'description',v.description,
            'authors',ARRAY[v.authors],'tags',regexp_split_to_array(v.tags,'\s+'),'totalDownloads',p.downloads,'owners',ARRAY[o.name],
            'registration',$6 || '/v3/registration/' || p.id || '/index.json','projectUrl',v.project_url,'published',v.published_at,
            'versions',(SELECT jsonb_agg(jsonb_build_object('version',vv.display_version,'downloads',vv.downloads,'@id',$6 || '/v3/registration/' || p.id || '/' || vv.version || '.json') ORDER BY vv.version_sort COLLATE "C")
                FROM package_versions vv WHERE vv.package_id=p.id AND vv.listed AND ($7 OR NOT vv.prerelease) AND ($8 OR NOT vv.semver2)))
        FROM search_documents d JOIN packages p ON p.id=d.package_id
        JOIN package_versions v ON v.package_id=d.package_id AND v.version=d.version JOIN profiles o ON o.id=p.owner_id
        WHERE {filter} ORDER BY {order} LIMIT $4 OFFSET $5
    "#)).bind(channel).bind(&q).bind(pattern).bind(query.take.unwrap_or(20)).bind(query.skip).bind(&state.config.public_url).bind(query.prerelease).bind(channel>=2).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(
        json!({"@context":{"@vocab":"http://schema.nuget.org/schema#"},"totalHits":total,"data":rows}),
    )
}
pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(search_value(&state, &query).await?))
}
pub async fn autocomplete(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    if let Some(id) = &query.id {
        let rows = match all_versions(&state, id).await {
            Ok(rows) => rows,
            Err(e) if e.0 == StatusCode::NOT_FOUND => Vec::new(),
            Err(e) => return Err(e),
        };
        let channel = channel(&query);
        let versions: Vec<&String> = rows
            .iter()
            .filter(|p| {
                p.listed && (query.prerelease || !p.prerelease) && (channel >= 2 || !p.semver2)
            })
            .map(|p| &p.version)
            .collect();
        return Ok(Json(json!({"totalHits":versions.len(),"data":versions})));
    }
    let result = search_value(&state, &query).await?;
    Ok(Json(
        json!({"totalHits":result["totalHits"],"data":result["data"].as_array().unwrap().iter().map(|p| p["id"].clone()).collect::<Vec<_>>()}),
    ))
}
pub async fn details_value(state: &AppState, id: &str) -> ApiResult<Value> {
    let rows = all_versions(state, id).await?;
    let latest = rows
        .iter()
        .rev()
        .find(|p| p.listed && !p.prerelease)
        .or_else(|| rows.iter().rev().find(|p| p.listed))
        .unwrap_or_else(|| rows.last().unwrap());
    let mut result = catalog(state, latest);
    result["owner"] = json!(latest.owner_name);
    result["totalDownloads"] = json!(latest.total_downloads);
    result["versions"] = json!(rows.iter().rev().map(|p| json!({"version":p.display_version,"normalizedVersion":p.version,"listed":p.listed,"prerelease":p.prerelease,"published":p.published_at,"downloads":p.downloads,"size":p.size,"dependencies":p.dependencies,"downloadUrl":content_url(state,p)})).collect::<Vec<_>>());
    Ok(result)
}
pub async fn details(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(details_value(&state, &id).await?))
}
pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let result: Value = sqlx::query_scalar("SELECT jsonb_build_object('packages',(SELECT count(DISTINCT package_id) FROM search_documents),'versions',(SELECT count(*) FROM package_versions WHERE listed),'downloads',(SELECT COALESCE(sum(downloads),0) FROM packages),'profiles',(SELECT count(*) FROM profiles))").fetch_one(&state.db).await?;
    Ok(Json(result))
}
pub async fn owned_packages(
    State(state): State<AppState>,
    Path(profile): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    if !auth::is_admin(&state, &headers) {
        let who = auth::authenticate(&state, &headers).await?;
        if who.profile_id != profile {
            return Err(ApiError::forbidden("This profile belongs to another owner"));
        }
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',p.display_id,'downloads',p.downloads,'versions',(SELECT jsonb_agg(jsonb_build_object('version',display_version,'listed',listed,'downloads',downloads,'published',published_at) ORDER BY version_sort DESC) FROM package_versions WHERE package_id=p.id)) FROM packages p WHERE owner_id=$1 ORDER BY p.id").bind(profile).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
