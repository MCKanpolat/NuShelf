use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const SCOPES: &[&str] = &[
    "package:push",
    "package:unlist",
    "package:read",
    "mcp:read",
    "keys:manage",
    "feeds:manage",
];

#[derive(FromRow, Clone)]
pub struct Identity {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub scopes: Vec<String>,
    pub package_prefix: String,
    pub expires_at: Option<DateTime<Utc>>,
}
impl Identity {
    pub fn require(&self, scope: &str) -> ApiResult<()> {
        if self.scopes.iter().any(|s| s == scope) {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "API key needs '{scope}' permission"
            )))
        }
    }
    pub fn package(&self, package_id: &str) -> ApiResult<()> {
        if package_id.starts_with(&self.package_prefix) {
            Ok(())
        } else {
            Err(ApiError::forbidden("Package is outside this key's prefix"))
        }
    }
}
pub fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-nuget-apikey")
        .and_then(|h| h.to_str().ok())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.strip_prefix("Bearer "))
        })
}
pub fn hash_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}
pub fn is_admin(state: &AppState, headers: &HeaderMap) -> bool {
    credential(headers).is_some_and(|key| {
        bool::from(
            Sha256::digest(key.as_bytes())
                .ct_eq(&Sha256::digest(state.config.admin_key.as_bytes())),
        )
    })
}
pub async fn authenticate(state: &AppState, headers: &HeaderMap) -> ApiResult<Identity> {
    let key = credential(headers).ok_or_else(ApiError::unauthorized)?;
    if key.len() > 200 {
        return Err(ApiError::unauthorized());
    }
    sqlx::query_as::<_, Identity>("SELECT id, profile_id, scopes, package_prefix, expires_at FROM api_keys WHERE key_hash=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())")
        .bind(hash_key(&key)).fetch_optional(&state.db).await?.ok_or_else(ApiError::unauthorized)
}
fn credential(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = token(headers) {
        return Some(value.to_owned());
    }
    let value = headers.get("authorization")?.to_str().ok()?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") || encoded.len() > 512 {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let value = String::from_utf8(decoded).ok()?;
    Some(value.split_once(':')?.1.to_owned())
}
async fn manage(
    state: &AppState,
    headers: &HeaderMap,
    profile: Uuid,
) -> ApiResult<Option<Identity>> {
    if is_admin(state, headers) {
        return Ok(None);
    }
    let who = authenticate(state, headers).await?;
    who.require("keys:manage")?;
    if who.profile_id != profile {
        return Err(ApiError::forbidden("This profile belongs to another owner"));
    }
    Ok(Some(who))
}
pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    if is_admin(&state, &headers) {
        return Ok(Json(
            json!({"admin":true,"storageProvider":state.config.storage_provider,"publicUrl":state.config.public_url}),
        ));
    }
    let who = authenticate(&state, &headers).await?;
    let name: String = sqlx::query_scalar("SELECT name FROM profiles WHERE id=$1")
        .bind(who.profile_id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(
        json!({"admin":false,"profileId":who.profile_id,"keyId":who.id,"name":name,"scopes":who.scopes,"packagePrefix":who.package_prefix,"expiresAt":who.expires_at}),
    ))
}
pub async fn profiles(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    if !is_admin(&state, &headers) {
        return Err(ApiError::unauthorized());
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',p.id,'name',p.name,'createdAt',p.created_at,'packageCount',(SELECT count(*) FROM packages WHERE owner_id=p.id)) FROM profiles p ORDER BY p.created_at DESC").fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
#[derive(Deserialize)]
pub struct ProfileInput {
    pub name: String,
}
fn valid_name(name: &str) -> ApiResult<&str> {
    let name = name.trim();
    if name.is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
        return Err(ApiError::bad("Profile name must contain 1–80 characters"));
    }
    Ok(name)
}
pub async fn create_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ProfileInput>,
) -> ApiResult<Json<Value>> {
    if !is_admin(&state, &headers) {
        return Err(ApiError::unauthorized());
    }
    let name = valid_name(&input.name)?;
    let id = Uuid::new_v4();
    let inserted =
        sqlx::query("INSERT INTO profiles(id,name) VALUES($1,$2) ON CONFLICT(name) DO NOTHING")
            .bind(id)
            .bind(name)
            .execute(&state.db)
            .await?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "Profile name already exists".into(),
        ));
    }
    Ok(Json(json!({"id":id,"name":name})))
}
pub async fn rename_profile(
    State(state): State<AppState>,
    Path(profile): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ProfileInput>,
) -> ApiResult<Json<Value>> {
    manage(&state, &headers, profile).await?;
    let name = valid_name(&input.name)?;
    match sqlx::query("UPDATE profiles SET name=$2 WHERE id=$1")
        .bind(profile)
        .bind(name)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => Err(ApiError::not_found()),
        Ok(_) => Ok(Json(json!({"id":profile,"name":name}))),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "Profile name already exists".into(),
        )),
        Err(e) => Err(e.into()),
    }
}
pub async fn keys(
    State(state): State<AppState>,
    Path(profile): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    manage(&state, &headers, profile).await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'label',label,'hint',key_hint,'scopes',scopes,'packagePrefix',package_prefix,'expiresAt',expires_at,'revokedAt',revoked_at,'createdAt',created_at) FROM api_keys WHERE profile_id=$1 ORDER BY created_at DESC").bind(profile).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInput {
    pub label: String,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub package_prefix: String,
    pub expires_at: Option<DateTime<Utc>>,
}
pub async fn create_key(
    State(state): State<AppState>,
    Path(profile): Path<Uuid>,
    headers: HeaderMap,
    Json(mut input): Json<KeyInput>,
) -> ApiResult<Json<Value>> {
    let parent = manage(&state, &headers, profile).await?;
    valid_name(&input.label)?;
    input.scopes.sort();
    input.scopes.dedup();
    if input.scopes.is_empty() || input.scopes.iter().any(|s| !SCOPES.contains(&s.as_str())) {
        return Err(ApiError::bad("Select valid key permissions"));
    }
    input.package_prefix = input.package_prefix.trim().to_ascii_lowercase();
    if !input.package_prefix.is_empty() {
        crate::version::package_id(&input.package_prefix)?;
    }
    if input.expires_at.is_some_and(|d| d <= Utc::now()) {
        return Err(ApiError::bad("Expiration must be in the future"));
    }
    if let Some(parent) = parent
        && (input.scopes.iter().any(|s| !parent.scopes.contains(s))
            || !input.package_prefix.starts_with(&parent.package_prefix)
            || parent
                .expires_at
                .is_some_and(|expiry| input.expires_at.is_none_or(|d| d > expiry)))
    {
        return Err(ApiError::forbidden(
            "A child key cannot exceed the current key's permissions, prefix or lifetime",
        ));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM profiles WHERE id=$1)")
        .bind(profile)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    let id = Uuid::new_v4();
    let raw = format!("nsh_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let hint = format!("{}…{}", &raw[..8], &raw[raw.len() - 4..]);
    sqlx::query("INSERT INTO api_keys(id,profile_id,label,key_hash,key_hint,scopes,package_prefix,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id).bind(profile).bind(input.label.trim()).bind(hash_key(&raw)).bind(hint).bind(input.scopes).bind(input.package_prefix).bind(input.expires_at).execute(&state.db).await?;
    Ok(Json(
        json!({"id":id,"key":raw,"message":"Save this key now. It will not be shown again."}),
    ))
}
pub async fn revoke_key(
    State(state): State<AppState>,
    Path((profile, key)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    manage(&state, &headers, profile).await?;
    let result = sqlx::query(
        "UPDATE api_keys SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1 AND profile_id=$2",
    )
    .bind(key)
    .bind(profile)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(Json(json!({"revoked":true})))
}
