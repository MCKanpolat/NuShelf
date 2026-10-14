pub mod cache;
pub mod http;
pub mod sources;
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    registry,
    version::{NuGetVersion, package_id},
};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sources::Reader;

pub async fn versions_value(state: &AppState, reader: &Reader, id: &str) -> ApiResult<Value> {
    match registry::versions(State(state.clone()), Path(id.into())).await {
        Ok(Json(value)) => Ok(value),
        Err(e) if e.0 == StatusCode::NOT_FOUND => {
            let (meta, _) = cache::metadata(state, reader, id).await?;
            Ok(json!({"versions":meta.versions}))
        }
        Err(e) => Err(e),
    }
}
pub async fn versions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        versions_value(&state, &Reader::from_headers(&state, &headers).await?, &id).await?,
    ))
}
pub async fn download(
    State(state): State<AppState>,
    Path((id, version, file)): Path<(String, String, String)>,
    method: Method,
    headers: HeaderMap,
) -> ApiResult<Response> {
    match registry::download(
        State(state.clone()),
        Path((id.clone(), version.clone(), file.clone())),
        method.clone(),
    )
    .await
    {
        Ok(value) => return Ok(value),
        Err(e) if e.0 == StatusCode::NOT_FOUND => {}
        Err(e) => return Err(e),
    }
    let id = package_id(&id)?;
    let version = NuGetVersion::parse(&version)?.normalized;
    if cache::local_exists(&state, &id).await? {
        return Err(ApiError::not_found());
    }
    let nupkg = format!("{id}.{version}.nupkg");
    if ![
        nupkg.as_str(),
        format!("{id}.nuspec").as_str(),
        format!("{nupkg}.sha512").as_str(),
    ]
    .contains(&file.as_str())
    {
        return Err(ApiError::not_found());
    }
    let reader = Reader::from_headers(&state, &headers).await?;
    let (cached, source, hit) = cache::ensure(&state, &reader, &id, &version).await?;
    let mut response = if file == format!("{id}.nuspec") {
        (
            [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
            cached.nuspec,
        )
            .into_response()
    } else if file == format!("{nupkg}.sha512") {
        ([(header::CONTENT_TYPE, "text/plain")], cached.sha512).into_response()
    } else {
        let path = object_store::path::Path::from(cached.storage_key.as_str());
        let mut response = if method == Method::HEAD {
            state.storage.head(&path).await?;
            Body::empty().into_response()
        } else {
            let content = state.storage.get(&path).await?;
            sqlx::query("UPDATE upstream_cache SET downloads=downloads+1 WHERE package_id=$1 AND version=$2").bind(&id).bind(&version).execute(&state.db).await?;
            Body::from_stream(content.into_stream()).into_response()
        };
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            "application/octet-stream".parse().unwrap(),
        );
        response.headers_mut().insert(
            header::CONTENT_LENGTH,
            cached.size.to_string().parse().unwrap(),
        );
        response
    };
    response.headers_mut().insert(
        "x-nushelf-cache",
        if hit { "HIT" } else { "MISS" }.parse().unwrap(),
    );
    response
        .headers_mut()
        .insert("x-nushelf-upstream", source.id.to_string().parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    Ok(response)
}
fn index_document(state: &AppState, id: &str, entries: &[Value]) -> Value {
    let url = format!(
        "{}/v3/registration/{id}/index.json",
        state.config.public_url
    );
    let pages:Vec<_>=entries.chunks(64).map(|group|{
        let lower=NuGetVersion::parse(group[0]["version"].as_str().unwrap()).unwrap().normalized;
        let upper=NuGetVersion::parse(group.last().unwrap()["version"].as_str().unwrap()).unwrap().normalized;
        json!({"@id":format!("{url}#page/{lower}/{upper}"),"parent":url,"count":group.len(),"lower":lower,"upper":upper,"items":group.iter().map(|entry|json!({"@id":entry["@id"],"catalogEntry":entry,"packageContent":entry["packageContent"]})).collect::<Vec<_>>()})
    }).collect();
    json!({"@id":url,"count":pages.len(),"items":pages})
}
pub async fn registration(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    match registry::registration(State(state.clone()), Path(id.clone())).await {
        Ok(value) => Ok(value),
        Err(e) if e.0 == StatusCode::NOT_FOUND => {
            let reader = Reader::from_headers(&state, &headers).await?;
            let id = package_id(&id)?;
            let (entries, _) = cache::catalog(&state, &reader, &id).await?;
            Ok(Json(index_document(&state, &id, &entries)))
        }
        Err(e) => Err(e),
    }
}
pub async fn registration_leaf(
    State(state): State<AppState>,
    Path((id, file)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    match registry::registration_leaf(State(state.clone()), Path((id.clone(), file.clone()))).await
    {
        Ok(value) => Ok(value),
        Err(e) if e.0 == StatusCode::NOT_FOUND => {
            let reader = Reader::from_headers(&state, &headers).await?;
            let id = package_id(&id)?;
            let version =
                NuGetVersion::parse(file.strip_suffix(".json").ok_or_else(ApiError::not_found)?)?
                    .normalized;
            let (entries, _) = cache::catalog(&state, &reader, &id).await?;
            let entry = entries
                .iter()
                .find(|e| {
                    NuGetVersion::parse(e["version"].as_str().unwrap())
                        .unwrap()
                        .normalized
                        == version
                })
                .ok_or_else(ApiError::not_found)?;
            Ok(Json(
                json!({"@id":entry["@id"],"listed":entry["listed"],"published":entry["published"],"packageContent":entry["packageContent"],"registration":format!("{}/v3/registration/{id}/index.json",state.config.public_url)}),
            ))
        }
        Err(e) => Err(e),
    }
}
pub async fn details_value(state: &AppState, reader: &Reader, id: &str) -> ApiResult<Value> {
    match registry::details_value(state, id).await {
        Ok(value) => return Ok(value),
        Err(e) if e.0 == StatusCode::NOT_FOUND => {}
        Err(e) => return Err(e),
    }
    let id = package_id(id)?;
    let (entries, source) = cache::catalog(state, reader, &id).await?;
    let latest = entries
        .iter()
        .rev()
        .find(|e| {
            e["listed"] == true
                && !NuGetVersion::parse(e["version"].as_str().unwrap())
                    .unwrap()
                    .prerelease
        })
        .or_else(|| entries.iter().rev().find(|e| e["listed"] == true))
        .unwrap_or_else(|| entries.last().unwrap());
    let cached: Vec<cache::Cached> =
        sqlx::query_as("SELECT * FROM upstream_cache WHERE package_id=$1")
            .bind(&id)
            .fetch_all(&state.db)
            .await?;
    let find = |version: &str| cached.iter().find(|c| c.version == version);
    let mut result = latest.clone();
    result["owner"] = json!(source.name);
    result["totalDownloads"] = json!(cached.iter().map(|c| c.downloads).sum::<i64>());
    result["upstream"] = json!({"id":source.id,"name":source.name,"private":source.private});
    result["cacheStatus"] = json!(if find(
        &NuGetVersion::parse(latest["version"].as_str().unwrap())
            .unwrap()
            .normalized
    )
    .is_some()
    {
        "cached"
    } else {
        "metadataOnly"
    });
    result["versions"]=json!(entries.iter().rev().map(|e|{
        let v=NuGetVersion::parse(e["version"].as_str().unwrap()).unwrap();let item=find(&v.normalized);
        json!({"version":v.display,"normalizedVersion":v.normalized,"listed":e["listed"],"prerelease":v.prerelease,"published":e["published"],"downloads":item.map(|c|c.downloads).unwrap_or(0),"size":item.map(|c|c.size),"cached":item.is_some(),"dependencies":e["dependencyGroups"].as_array().cloned().unwrap_or_default(),"downloadUrl":cache::content(state,&id,&v.normalized)})
    }).collect::<Vec<_>>());
    Ok(result)
}
pub async fn details(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        details_value(&state, &Reader::from_headers(&state, &headers).await?, &id).await?,
    ))
}

const VISIBILITY: &str = "s.enabled AND upstream_route_allowed(d.package_id,s.id) AND NOT EXISTS(SELECT 1 FROM packages p WHERE p.id=d.package_id) AND (NOT s.private OR $4 OR ($5::uuid IS NOT NULL AND starts_with(d.package_id,$6) AND EXISTS(SELECT 1 FROM upstream_grants g WHERE g.source_id=s.id AND g.profile_id=$5)))";
pub async fn search_value(
    state: &AppState,
    reader: &Reader,
    query: &registry::SearchQuery,
) -> ApiResult<Value> {
    let mut result = registry::search_value(state, query).await?;
    let local_count = result["totalHits"].as_i64().unwrap();
    let remaining = query.take.unwrap_or(20) - result["data"].as_array().unwrap().len() as i64;
    let offset = (query.skip - local_count).max(0);
    let q = query.q.trim().to_ascii_lowercase();
    let pattern = format!(
        "{}%",
        q.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let channel = registry::channel(query);
    let from = "upstream_search_documents d JOIN upstream_metadata m USING(package_id) JOIN upstream_sources s ON s.id=m.source_id";
    let primary = if q.is_empty() {
        "$2::text IS NOT NULL AND $3::text IS NOT NULL"
    } else {
        "d.search_vector @@ websearch_to_tsquery('simple',$2) OR d.package_id LIKE $3 ESCAPE '\\'"
    };
    let mut filter = format!("d.channel=$1 AND ({primary}) AND {VISIBILITY}");
    let mut count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {from} WHERE {filter}"))
        .bind(channel)
        .bind(&q)
        .bind(&pattern)
        .bind(reader.admin)
        .bind(reader.profile)
        .bind(&reader.prefix)
        .fetch_one(&state.db)
        .await?;
    if count == 0 && q.len() >= 3 {
        filter =
            format!("d.channel=$1 AND d.package_id % $2 AND $3::text IS NOT NULL AND {VISIBILITY}");
        count = sqlx::query_scalar(&format!("SELECT count(*) FROM {from} WHERE {filter}"))
            .bind(channel)
            .bind(&q)
            .bind(&pattern)
            .bind(reader.admin)
            .bind(reader.profile)
            .bind(&reader.prefix)
            .fetch_one(&state.db)
            .await?;
    }
    result["totalHits"] = json!(local_count + count);
    if remaining > 0 && count > offset {
        let order = match query.sort.as_deref() {
            Some("name") => "d.package_id",
            Some("downloads") => "c.downloads DESC,d.package_id",
            Some("recent") => "d.cached_at DESC,d.package_id",
            _ => {
                "(d.package_id=$2) DESC,ts_rank_cd(d.search_vector,websearch_to_tsquery('simple',$2)) DESC,similarity(d.package_id,$2) DESC,d.package_id"
            }
        };
        let data:Vec<Value>=sqlx::query_scalar(&format!(r#"SELECT c.catalog || jsonb_build_object('totalDownloads',(SELECT COALESCE(sum(downloads),0) FROM upstream_cache WHERE package_id=d.package_id),'authors',ARRAY[d.authors],'tags',regexp_split_to_array(d.tags,'\s+'),'owners',ARRAY[s.name],'upstream',jsonb_build_object('id',s.id,'name',s.name,'private',s.private),'cacheStatus','cached','registration',$7||'/v3/registration/'||d.package_id||'/index.json','versions',(SELECT jsonb_agg(jsonb_build_object('version',v.catalog->>'version','downloads',v.downloads,'@id',$7||'/v3/registration/'||d.package_id||'/'||v.version||'.json') ORDER BY v.version_sort) FROM upstream_cache v WHERE v.package_id=d.package_id AND ($10 OR NOT v.prerelease) AND ($11 OR NOT v.semver2) AND COALESCE((v.catalog->>'listed')::boolean,true))) FROM {from} JOIN upstream_cache c ON c.package_id=d.package_id AND c.version=d.version WHERE {filter} ORDER BY {order} LIMIT $8 OFFSET $9"#))
            .bind(channel).bind(&q).bind(pattern).bind(reader.admin).bind(reader.profile).bind(&reader.prefix).bind(&state.config.public_url).bind(remaining).bind(offset).bind(query.prerelease).bind(channel>=2).fetch_all(&state.db).await?;
        result["data"].as_array_mut().unwrap().extend(data);
    }
    Ok(result)
}
pub async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<registry::SearchQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        search_value(
            &state,
            &Reader::from_headers(&state, &headers).await?,
            &query,
        )
        .await?,
    ))
}
pub async fn autocomplete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<registry::SearchQuery>,
) -> ApiResult<Json<Value>> {
    let reader = Reader::from_headers(&state, &headers).await?;
    if let Some(id) = &query.id {
        if cache::local_exists(&state, &package_id(id)?).await? {
            return registry::autocomplete(State(state), Query(query)).await;
        }
        let (entries, _) = cache::catalog(&state, &reader, id).await?;
        let channel = registry::channel(&query);
        let versions: Vec<_> = entries
            .iter()
            .filter(|e| e["listed"] == true)
            .filter_map(|e| NuGetVersion::parse(e["version"].as_str().unwrap()).ok())
            .filter(|v| (query.prerelease || !v.prerelease) && (channel >= 2 || !v.semver2))
            .map(|v| v.normalized)
            .collect();
        return Ok(Json(json!({"totalHits":versions.len(),"data":versions})));
    }
    let result = search_value(&state, &reader, &query).await?;
    Ok(Json(
        json!({"totalHits":result["totalHits"],"data":result["data"].as_array().unwrap().iter().map(|e|e["id"].clone()).collect::<Vec<_>>()}),
    ))
}
pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let Json(mut result) = registry::stats(State(state.clone())).await?;
    let cache:Value=sqlx::query_scalar("SELECT jsonb_build_object('packages',count(DISTINCT c.package_id),'versions',count(*),'downloads',COALESCE(sum(c.downloads),0)) FROM upstream_cache c JOIN upstream_metadata m USING(package_id) JOIN upstream_sources s ON s.id=m.source_id WHERE s.enabled AND NOT s.private AND upstream_route_allowed(c.package_id,s.id) AND NOT EXISTS(SELECT 1 FROM packages p WHERE p.id=c.package_id) AND COALESCE((c.catalog->>'listed')::boolean,true)").fetch_one(&state.db).await?;
    for key in ["packages", "versions", "downloads"] {
        result[key] = json!(result[key].as_i64().unwrap_or(0) + cache[key].as_i64().unwrap_or(0));
    }
    result["cachedPackages"] = cache["packages"].clone();
    result["cachedVersions"] = cache["versions"].clone();
    Ok(Json(result))
}
