use super::{
    access,
    model::{ApplicationInput, ReleaseInput},
    sbom,
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn label(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 100 || value.chars().any(char::is_control) {
        return Err(ApiError::bad(
            "Names and release versions need 1–100 printable characters",
        ));
    }
    Ok(value.to_owned())
}
pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let access = access::require(&state, &headers, false).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',a.id,'name',a.name,'ownerId',a.owner_id,'createdAt',a.created_at,'releaseCount',(SELECT count(*) FROM inventory_releases r WHERE r.application_id=a.id)) FROM inventory_applications a WHERE $1 OR owner_id=$2 ORDER BY name LIMIT 100").bind(access.manager).bind(access.profile).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ApplicationInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let access = access::require(&state, &headers, true).await?;
    let name = label(&input.name)?;
    let id = Uuid::new_v4();
    let owner = input.owner_profile_id.or(access.profile);
    if !access.manager && owner != access.profile {
        return Err(ApiError::forbidden(
            "Only administrators can assign another application owner",
        ));
    }
    if let Some(profile) = owner {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.profiles WHERE id=$1) AND ($2::uuid IS NULL OR EXISTS(SELECT 1 FROM public.feed_members WHERE profile_id=$1 AND feed_id=$2 AND role IN ('publisher','administrator')))").bind(profile).bind(state.feed).fetch_one(&state.db).await?;
        if !valid {
            return Err(ApiError::bad(
                "Application owner needs an existing publisher profile and membership in this feed",
            ));
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(current_schema()||':inventory-applications',0))").execute(&mut *tx).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM inventory_applications")
        .fetch_one(&mut *tx)
        .await?;
    if count >= 100 {
        return Err(ApiError::bad(
            "This feed has reached its 100 application limit",
        ));
    }
    let inserted=sqlx::query("INSERT INTO inventory_applications(id,name,owner_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(id).bind(&name).bind(owner).execute(&mut *tx).await?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Application name already exists in this feed".into(),
        ));
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({"id":id,"name":name}))))
}
pub async fn releases(
    State(state): State<AppState>,
    Path(application): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    access::require(&state, &headers, false)
        .await?
        .application(&state, application)
        .await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'version',version,'repositoryUrl',repository_url,'commitSha',commit_sha,'sbomHash',sbom_hash,'componentCount',component_count,'nugetCount',nuget_count,'edgeCount',edge_count,'createdAt',created_at) FROM inventory_releases WHERE application_id=$1 ORDER BY created_at DESC,id DESC LIMIT 100").bind(application).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn import(
    State(state): State<AppState>,
    Path(application): Path<Uuid>,
    headers: HeaderMap,
    Json(mut input): Json<ReleaseInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    access::require(&state, &headers, true)
        .await?
        .application(&state, application)
        .await?;
    input.version = label(&input.version)?;
    if let Some(raw) = &input.repository_url {
        let url = url::Url::parse(raw).map_err(|_| ApiError::bad("Invalid repository URL"))?;
        if raw.len() > 2048
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ApiError::bad(
                "Repository URL must be HTTPS without credentials, query or fragment",
            ));
        }
    }
    if input
        .commit_sha
        .as_ref()
        .is_some_and(|s| ![40, 64].contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(ApiError::bad(
            "Commit SHA must contain 40 or 64 hexadecimal characters",
        ));
    }
    let document = input.sbom.clone();
    let inventory = tokio::task::spawn_blocking(move || sbom::parse(&document))
        .await
        .map_err(ApiError::internal)??;
    let hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&input.sbom).map_err(ApiError::internal)?,
    ));
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT id FROM inventory_applications WHERE id=$1 FOR UPDATE")
        .bind(application)
        .fetch_one(&mut *tx)
        .await?;
    let existing:Option<(Uuid,String,Option<String>,Option<String>)>=sqlx::query_as("SELECT id,sbom_hash,repository_url,commit_sha FROM inventory_releases WHERE application_id=$1 AND version=$2").bind(application).bind(&input.version).fetch_optional(&mut *tx).await?;
    if let Some((id, old, repository, commit)) = existing {
        if old != hash || repository != input.repository_url || commit != input.commit_sha {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Release inventories are immutable; use a new release version".into(),
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(json!({"id":id,"version":input.version,"hash":hash,"unchanged":true})),
        ));
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_releases WHERE application_id=$1")
            .bind(application)
            .fetch_one(&mut *tx)
            .await?;
    if count >= 100 {
        return Err(ApiError::bad(
            "This application has reached its 100 release limit; remove an obsolete inventory first",
        ));
    }
    let id = Uuid::new_v4();
    let nuget = inventory
        .components
        .iter()
        .filter(|c| c.package_id.is_some())
        .count();
    sqlx::query("INSERT INTO inventory_releases(id,application_id,version,repository_url,commit_sha,sbom_hash,document,root_ref,component_count,nuget_count,edge_count) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)").bind(id).bind(application).bind(&input.version).bind(input.repository_url).bind(input.commit_sha).bind(&hash).bind(input.sbom).bind(inventory.root).bind(inventory.components.len() as i32).bind(nuget as i32).bind(inventory.edges.len() as i32).execute(&mut *tx).await?;
    let refs: Vec<_> = inventory
        .components
        .iter()
        .map(|c| c.reference.clone())
        .collect();
    let names: Vec<_> = inventory
        .components
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let purls: Vec<_> = inventory
        .components
        .iter()
        .map(|c| c.purl.clone())
        .collect();
    let ids: Vec<_> = inventory
        .components
        .iter()
        .map(|c| c.package_id.clone())
        .collect();
    let versions: Vec<_> = inventory
        .components
        .iter()
        .map(|c| c.version.clone())
        .collect();
    sqlx::query("INSERT INTO inventory_components(release_id,bom_ref,name,purl,package_id,version) SELECT $1,u.* FROM unnest($2::text[],$3::text[],$4::text[],$5::text[],$6::text[]) AS u").bind(id).bind(refs).bind(names).bind(purls).bind(ids).bind(versions).execute(&mut *tx).await?;
    let (parents, children): (Vec<_>, Vec<_>) = inventory.edges.into_iter().unzip();
    sqlx::query("INSERT INTO inventory_edges(release_id,parent_ref,child_ref) SELECT $1,u.* FROM unnest($2::text[],$3::text[]) AS u").bind(id).bind(parents).bind(children).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id":id,"version":input.version,"hash":hash,"componentCount":inventory.components.len(),"nugetCount":nuget}),
        ),
    ))
}
pub async fn export(
    State(state): State<AppState>,
    Path((application, release)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    access::require(&state, &headers, false)
        .await?
        .application(&state, application)
        .await?;
    let doc: Option<Value> = sqlx::query_scalar(
        "SELECT document FROM inventory_releases WHERE id=$1 AND application_id=$2",
    )
    .bind(release)
    .bind(application)
    .fetch_optional(&state.db)
    .await?;
    Ok(Json(doc.ok_or_else(ApiError::not_found)?))
}
pub async fn delete_release(
    State(state): State<AppState>,
    Path((application, release)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    access::require(&state, &headers, true)
        .await?
        .application(&state, application)
        .await?;
    let changed = sqlx::query("DELETE FROM inventory_releases WHERE id=$1 AND application_id=$2")
        .bind(release)
        .bind(application)
        .execute(&state.db)
        .await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}
