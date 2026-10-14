use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    version::package_id,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Clone, FromRow)]
pub struct Source {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub priority: i32,
    pub enabled: bool,
    pub patterns: Vec<String>,
    pub excludes: Vec<String>,
    pub private: bool,
    pub auth_type: String,
    pub username: Option<String>,
    pub credential_env: Option<String>,
    pub allow_http: bool,
    pub allow_private_network: bool,
    pub timeout_seconds: i32,
    pub metadata_ttl_seconds: i32,
    pub stale_on_error: bool,
    pub service_index: Option<Value>,
    pub index_checked_at: Option<DateTime<Utc>>,
    pub allowed_profiles: Vec<Uuid>,
}
pub const SELECT_SOURCE: &str = "SELECT s.*,ARRAY(SELECT profile_id FROM upstream_grants g WHERE g.source_id=s.id) AS allowed_profiles FROM upstream_sources s";
#[derive(Clone, Default)]
pub struct Reader {
    pub admin: bool,
    pub authenticated: bool,
    pub profile: Option<Uuid>,
    pub prefix: String,
}
impl Reader {
    pub async fn from_headers(state: &AppState, headers: &HeaderMap) -> ApiResult<Self> {
        if auth::is_admin(state, headers) {
            return Ok(Self {
                admin: true,
                authenticated: true,
                ..Default::default()
            });
        }
        if !headers.contains_key("authorization") && !headers.contains_key("x-nuget-apikey") {
            return Ok(Self::default());
        }
        let who = auth::authenticate(state, headers).await?;
        Ok(Self {
            admin: false,
            authenticated: true,
            profile: who
                .scopes
                .iter()
                .any(|s| s == "package:read")
                .then_some(who.profile_id),
            prefix: who.package_prefix,
        })
    }
    pub fn check(&self, source: &Source, id: &str) -> ApiResult<()> {
        if !source.enabled {
            return Err(ApiError::not_found());
        }
        if !source.private || self.admin {
            return Ok(());
        }
        let allowed = if let Some(profile) = self.profile {
            id.starts_with(&self.prefix) && source.allowed_profiles.contains(&profile)
        } else {
            false
        };
        if allowed {
            Ok(())
        } else if self.authenticated {
            Err(ApiError::forbidden(
                "Private upstream requires package:read permission and a profile grant",
            ))
        } else {
            Err(ApiError::unauthorized())
        }
    }
}
fn yes() -> bool {
    true
}
fn priority() -> i32 {
    100
}
fn timeout() -> i32 {
    15
}
fn ttl() -> i32 {
    300
}
fn patterns() -> Vec<String> {
    vec!["*".into()]
}
fn none() -> String {
    "none".into()
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceInput {
    pub name: String,
    pub url: String,
    #[serde(default = "priority")]
    pub priority: i32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "patterns")]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default)]
    pub private: Option<bool>,
    #[serde(default)]
    pub allowed_profiles: Vec<Uuid>,
    #[serde(default = "none")]
    pub auth_type: String,
    pub username: Option<String>,
    pub credential_env: Option<String>,
    #[serde(default)]
    pub allow_http: bool,
    #[serde(default)]
    pub allow_private_network: bool,
    #[serde(default = "timeout")]
    pub timeout_seconds: i32,
    #[serde(default = "ttl")]
    pub metadata_ttl_seconds: i32,
    #[serde(default = "yes")]
    pub stale_on_error: bool,
}
impl SourceInput {
    fn is_private(&self) -> bool {
        self.private.unwrap_or(self.auth_type != "none")
    }
    fn validate(mut self) -> ApiResult<Self> {
        self.name = self.name.trim().into();
        if self.name.is_empty() || self.name.len() > 80 || self.name.chars().any(char::is_control) {
            return Err(ApiError::bad("Source name must contain 1–80 characters"));
        }
        let url = url::Url::parse(self.url.trim())
            .map_err(|_| ApiError::bad("Invalid service index URL"))?;
        if url.host_str().is_none()
            || !matches!(url.scheme(), "https" | "http")
            || (url.scheme() == "http" && !self.allow_http)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ApiError::bad(
                "Use an HTTPS service index URL without credentials/query; HTTP needs explicit permission",
            ));
        }
        self.url = url.to_string();
        if !(0..=10000).contains(&self.priority)
            || !(1..=30).contains(&self.timeout_seconds)
            || !(1..=86400).contains(&self.metadata_ttl_seconds)
        {
            return Err(ApiError::bad("Invalid priority, timeout or metadata TTL"));
        }
        if self.patterns.is_empty()
            || self.patterns.len() > 100
            || self.excludes.len() > 100
            || self.allowed_profiles.len() > 100
        {
            return Err(ApiError::bad(
                "Provide 1–100 include patterns; at most 100 excludes/profiles",
            ));
        }
        for group in [&mut self.patterns, &mut self.excludes] {
            for pattern in group.iter_mut() {
                *pattern = pattern.trim().to_ascii_lowercase();
                if pattern == "*" {
                    continue;
                }
                let base = pattern.strip_suffix('*').unwrap_or(pattern);
                package_id(base)?;
            }
            group.sort();
            group.dedup();
        }
        self.allowed_profiles.sort();
        self.allowed_profiles.dedup();
        match self.auth_type.as_str() {
            "none" => {
                self.username = None;
                self.credential_env = None;
            }
            "bearer" | "basic" => {
                let name = self.credential_env.as_deref().unwrap_or_default();
                if !name.starts_with("UPSTREAM_")
                    || name.len() > 100
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                {
                    return Err(ApiError::bad(
                        "Credentials must reference an UPSTREAM_* environment variable",
                    ));
                }
                if self.auth_type == "basic"
                    && self.username.as_deref().is_none_or(|s| {
                        s.is_empty()
                            || s.len() > 200
                            || s.contains(':')
                            || s.chars().any(char::is_control)
                    })
                {
                    return Err(ApiError::bad("Basic authentication needs a valid username"));
                }
            }
            _ => {
                return Err(ApiError::bad(
                    "Authentication must be none, bearer or basic",
                ));
            }
        }
        Ok(self)
    }
    fn source(&self, id: Uuid) -> Source {
        Source {
            id,
            name: self.name.clone(),
            url: self.url.clone(),
            priority: self.priority,
            enabled: self.enabled,
            patterns: self.patterns.clone(),
            excludes: self.excludes.clone(),
            private: self.is_private(),
            auth_type: self.auth_type.clone(),
            username: self.username.clone(),
            credential_env: self.credential_env.clone(),
            allow_http: self.allow_http,
            allow_private_network: self.allow_private_network,
            timeout_seconds: self.timeout_seconds,
            metadata_ttl_seconds: self.metadata_ttl_seconds,
            stale_on_error: self.stale_on_error,
            service_index: None,
            index_checked_at: None,
            allowed_profiles: self.allowed_profiles.clone(),
        }
    }
}
pub fn matching(pattern: &str, id: &str) -> Option<usize> {
    if let Some(prefix) = pattern.strip_suffix('*') {
        id.starts_with(prefix).then_some(prefix.len())
    } else {
        (pattern == id).then_some(1000 + pattern.len())
    }
}
pub async fn candidates(state: &AppState, id: &str) -> ApiResult<Vec<Source>> {
    let rows = sqlx::query_as::<_, Source>(&format!(
        "{SELECT_SOURCE} WHERE enabled ORDER BY priority,id"
    ))
    .fetch_all(&state.db)
    .await?;
    Ok(select_candidates(rows, id))
}
pub fn select_candidates(rows: Vec<Source>, id: &str) -> Vec<Source> {
    let mut matches: Vec<_> = rows
        .into_iter()
        .filter_map(|s| {
            if s.excludes.iter().any(|p| matching(p, id).is_some()) {
                return None;
            }
            s.patterns
                .iter()
                .filter_map(|p| matching(p, id))
                .max()
                .map(|specificity| (specificity, s))
        })
        .collect();
    let best = matches.iter().map(|(n, _)| *n).max();
    matches.retain(|(n, _)| Some(*n) == best);
    matches.into_iter().map(|(_, s)| s).collect()
}
pub fn admin(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if auth::is_admin(state, headers) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}
pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT (to_jsonb(s)-'service_index'-'index_checked_at') || jsonb_build_object('allowedProfiles',COALESCE((SELECT jsonb_agg(profile_id) FROM upstream_grants WHERE source_id=s.id),'[]'::jsonb),'cacheVersions',(SELECT count(*) FROM upstream_cache c JOIN upstream_metadata m USING(package_id) WHERE m.source_id=s.id),'cacheBytes',(SELECT COALESCE(sum(c.size),0) FROM upstream_cache c JOIN upstream_metadata m USING(package_id) WHERE m.source_id=s.id)) FROM upstream_sources s ORDER BY priority,id").fetch_all(&state.db).await?;
    // Credential environment names are references, not secret values. Raw service indexes never leave this API.
    Ok(Json(json!(rows)))
}
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SourceInput>,
) -> ApiResult<Json<Value>> {
    save(&state, &headers, Uuid::new_v4(), input, false).await
}
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<SourceInput>,
) -> ApiResult<Json<Value>> {
    save(&state, &headers, id, input, true).await
}
async fn save(
    state: &AppState,
    headers: &HeaderMap,
    id: Uuid,
    input: SourceInput,
    update: bool,
) -> ApiResult<Json<Value>> {
    admin(state, headers)?;
    let input = input.validate()?;
    let source_url =
        url::Url::parse(&input.url).map_err(|_| ApiError::bad("Invalid source URL"))?;
    let public_url = url::Url::parse(&state.config.public_url).map_err(ApiError::internal)?;
    if source_url.origin() == public_url.origin()
        && source_url.path().trim_end_matches('/') == "/v3/index.json"
    {
        return Err(ApiError::bad(
            "An upstream cannot point back to this registry",
        ));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("source:{id}"))
        .execute(&mut *tx)
        .await?;
    if update {
        let old: Option<String> =
            sqlx::query_scalar("SELECT url FROM upstream_sources WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let old = old.ok_or_else(ApiError::not_found)?;
        let cached: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upstream_metadata WHERE source_id=$1)")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if old != input.url && cached {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Clear this source's cache before changing its URL".into(),
            ));
        }
    }
    let profiles: i64 = sqlx::query_scalar("SELECT count(*) FROM profiles WHERE id=ANY($1)")
        .bind(&input.allowed_profiles)
        .fetch_one(&mut *tx)
        .await?;
    if profiles != input.allowed_profiles.len() as i64 {
        return Err(ApiError::bad("An allowed profile does not exist"));
    }
    let result=sqlx::query("INSERT INTO upstream_sources(id,name,url,priority,enabled,patterns,excludes,private,auth_type,username,credential_env,allow_http,allow_private_network,timeout_seconds,metadata_ttl_seconds,stale_on_error) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,url=EXCLUDED.url,priority=EXCLUDED.priority,enabled=EXCLUDED.enabled,patterns=EXCLUDED.patterns,excludes=EXCLUDED.excludes,private=EXCLUDED.private,auth_type=EXCLUDED.auth_type,username=EXCLUDED.username,credential_env=EXCLUDED.credential_env,allow_http=EXCLUDED.allow_http,allow_private_network=EXCLUDED.allow_private_network,timeout_seconds=EXCLUDED.timeout_seconds,metadata_ttl_seconds=EXCLUDED.metadata_ttl_seconds,stale_on_error=EXCLUDED.stale_on_error,service_index=NULL,index_checked_at=NULL")
        .bind(id).bind(&input.name).bind(&input.url).bind(input.priority).bind(input.enabled).bind(&input.patterns).bind(&input.excludes).bind(input.is_private()).bind(&input.auth_type).bind(&input.username).bind(&input.credential_env).bind(input.allow_http).bind(input.allow_private_network).bind(input.timeout_seconds).bind(input.metadata_ttl_seconds).bind(input.stale_on_error).execute(&mut *tx).await;
    if let Err(sqlx::Error::Database(e)) = &result
        && e.is_unique_violation()
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Source name already exists".into(),
        ));
    }
    result?;
    sqlx::query("DELETE FROM upstream_grants WHERE source_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for profile in &input.allowed_profiles {
        sqlx::query("INSERT INTO upstream_grants(source_id,profile_id) VALUES($1,$2)")
            .bind(id)
            .bind(profile)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM upstream_misses WHERE source_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"name":input.name})))
}
pub async fn test_input(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SourceInput>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let source = input.validate()?.source(Uuid::nil());
    let index = super::http::fetch_index(&source).await?;
    Ok(Json(
        json!({"ok":true,"resources":index["resources"].as_array().into_iter().flatten().map(|v|v["@type"].clone()).collect::<Vec<_>>()}),
    ))
}
pub async fn test_saved(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let source = sqlx::query_as::<_, Source>(&format!("{SELECT_SOURCE} WHERE id=$1"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let index = super::http::fetch_index(&source).await?;
    Ok(Json(
        json!({"ok":true,"resources":index["resources"].as_array().into_iter().flatten().map(|v|v["@type"].clone()).collect::<Vec<_>>()}),
    ))
}
pub async fn cache_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',c.catalog->>'id','version',c.version,'sourceId',s.id,'source',s.name,'private',s.private,'size',c.size,'downloads',c.downloads,'cachedAt',c.cached_at) FROM upstream_cache c JOIN upstream_metadata m USING(package_id) JOIN upstream_sources s ON s.id=m.source_id ORDER BY c.cached_at DESC LIMIT 500").fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn purge(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("source:{id}"))
        .execute(&mut *tx)
        .await?;
    let keys:Vec<String>=sqlx::query_scalar("SELECT storage_key FROM upstream_cache c JOIN upstream_metadata m USING(package_id) WHERE m.source_id=$1").bind(id).fetch_all(&mut *tx).await?;
    sqlx::query("DELETE FROM upstream_metadata WHERE source_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM upstream_misses WHERE source_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut orphaned = 0;
    for key in &keys {
        if state
            .storage
            .delete(&object_store::path::Path::from(key.as_str()))
            .await
            .is_err()
        {
            orphaned += 1;
        }
    }
    Ok(Json(
        json!({"clearedVersions":keys.len(),"storageCleanupFailures":orphaned}),
    ))
}
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("source:{id}"))
        .execute(&mut *tx)
        .await?;
    let used: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upstream_metadata WHERE source_id=$1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if used {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Clear this source's cache before deleting it".into(),
        ));
    }
    let result = sqlx::query("DELETE FROM upstream_sources WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    tx.commit().await?;
    Ok(Json(json!({"deleted":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pattern_specificity() {
        assert!(matching("acme.*", "acme.core").unwrap() > matching("*", "acme.core").unwrap());
        assert!(
            matching("acme.core", "acme.core").unwrap() > matching("acme.*", "acme.core").unwrap()
        );
        assert_eq!(matching("acme.*", "other.core"), None);
    }
}
