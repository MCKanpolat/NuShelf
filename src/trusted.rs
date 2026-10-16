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
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::FromRow;
use std::time::Duration;
use uuid::Uuid;
pub const ISSUER: &str = "https://token.actions.githubusercontent.com";
#[derive(FromRow)]
struct Policy {
    id: Uuid,
    profile_id: Uuid,
    name: String,
    repository: String,
    repository_id: String,
    owner_id: String,
    workflow: String,
    git_ref: String,
    environment: Option<String>,
    package_prefix: String,
    feed_id: Option<Uuid>,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    profile_id: Uuid,
    name: String,
    repository: String,
    repository_id: String,
    owner_id: String,
    workflow: String,
    git_ref: String,
    environment: Option<String>,
    package_prefix: String,
    feed_id: Option<Uuid>,
}
pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    crate::upstream::sources::admin(&state, &headers)?;
    let rows: Vec<Policy> =
        sqlx::query_as("SELECT * FROM public.trusted_publishers ORDER BY created_at DESC")
            .fetch_all(&state.db)
            .await?;
    Ok(Json(json!(rows.into_iter().map(|p|json!({"id":p.id,"profileId":p.profile_id,"name":p.name,"repository":p.repository,"repositoryId":p.repository_id,"ownerId":p.owner_id,"workflow":p.workflow,"gitRef":p.git_ref,"environment":p.environment,"packagePrefix":p.package_prefix,"feedId":p.feed_id,"enabled":p.enabled})).collect::<Vec<_>>())))
}
fn valid(input: &Input) -> ApiResult<()> {
    let parts: Vec<_> = input.repository.split('/').collect();
    if input.name.trim().is_empty()
        || input.name.len() > 80
        || parts.len() != 2
        || parts.iter().any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.')
        })
    {
        return Err(ApiError::bad("A policy needs a name and owner/repository"));
    }
    for id in [&input.repository_id, &input.owner_id] {
        if id.is_empty() || id.len() > 30 || !id.bytes().all(|c| c.is_ascii_digit()) {
            return Err(ApiError::bad(
                "Repository and owner IDs must be immutable numeric GitHub IDs",
            ));
        }
    }
    if !input.workflow.starts_with(".github/workflows/")
        || input.workflow.contains("..")
        || !(input.workflow.ends_with(".yml") || input.workflow.ends_with(".yaml"))
        || input.workflow.len() > 200
        || input.workflow.contains(['@', '\\', '\n', '\r'])
    {
        return Err(ApiError::bad(
            "Workflow must name a .github/workflows YAML file",
        ));
    }
    if !(input.git_ref.starts_with("refs/heads/") || input.git_ref.starts_with("refs/tags/"))
        || input.git_ref.len() > 200
        || input.git_ref.contains(['*', '?', '\n', '\r'])
    {
        return Err(ApiError::bad("Use an exact branch or tag reference"));
    }
    if input.package_prefix.trim().is_empty() {
        return Err(ApiError::bad(
            "Trusted publishing requires a package prefix",
        ));
    }
    package_id(input.package_prefix.trim())?;
    if input
        .environment
        .as_ref()
        .is_some_and(|e| e.is_empty() || e.len() > 100 || e.chars().any(char::is_control))
    {
        return Err(ApiError::bad("Invalid environment name"));
    }
    Ok(())
}
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<Json<Value>> {
    crate::upstream::sources::admin(&state, &headers)?;
    valid(&input)?;
    if let Some(feed) = input.feed_id {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM public.feed_members WHERE feed_id=$1 AND profile_id=$2",
        )
        .bind(feed)
        .bind(input.profile_id)
        .fetch_optional(&state.db)
        .await?;
        if role.is_none_or(|r| r == "reader") {
            return Err(ApiError::bad(
                "The publisher profile must have publisher or administrator membership in the selected feed",
            ));
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(314159271)")
        .execute(&mut *tx)
        .await?;
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM public.trusted_publishers WHERE enabled")
        .fetch_one(&mut *tx)
        .await?
        >= 100
    {
        return Err(ApiError::bad(
            "At most 100 active trusted publishers supported",
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.trusted_publishers(id,profile_id,name,repository,repository_id,owner_id,workflow,git_ref,environment,package_prefix,feed_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)").bind(id).bind(input.profile_id).bind(input.name).bind(input.repository).bind(input.repository_id).bind(input.owner_id).bind(input.workflow).bind(input.git_ref).bind(input.environment).bind(input.package_prefix.trim().to_ascii_lowercase()).bind(input.feed_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id})))
}
pub async fn disable(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    crate::upstream::sources::admin(&state, &headers)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE public.trusted_publishers SET enabled=false WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE public.api_keys SET revoked_at=COALESCE(revoked_at,now()) WHERE trusted_publisher_id=$1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize, Clone)]
pub struct Claims {
    pub iss: String,
    pub aud: Value,
    pub sub: String,
    pub exp: i64,
    pub iat: i64,
    pub nbf: i64,
    pub jti: String,
    pub repository: String,
    pub repository_id: String,
    pub repository_owner_id: String,
    pub workflow_ref: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub environment: Option<String>,
    pub event_name: String,
}
async fn keys(state: &AppState, kid: &str) -> ApiResult<JwkSet> {
    let row: (Option<Value>, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT document,fetched_at FROM public.trusted_oidc_keys WHERE singleton")
            .fetch_one(&state.db)
            .await?;
    if let (Some(document), Some(time)) = &row {
        let set: JwkSet = serde_json::from_value(document.clone()).map_err(ApiError::internal)?;
        if *time > Utc::now() - chrono::Duration::minutes(5) && set.find(kid).is_some() {
            return Ok(set);
        }
        if *time > Utc::now() - chrono::Duration::seconds(30) {
            return Err(ApiError::unauthorized());
        }
    }
    if sqlx::query("UPDATE public.trusted_oidc_keys SET last_attempt=now() WHERE singleton AND (last_attempt IS NULL OR last_attempt<now()-interval '30 seconds')").execute(&state.db).await?.rows_affected()==0{return Err(ApiError::unauthorized());}
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(ApiError::internal)?;
    let mut response = client
        .get(format!("{ISSUER}/.well-known/jwks"))
        .send()
        .await
        .map_err(|_| ApiError::bad("GitHub OIDC keys unavailable"))?
        .error_for_status()
        .map_err(|_| ApiError::bad("GitHub OIDC keys unavailable"))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApiError::bad("Incomplete OIDC key response"))?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(ApiError::bad("OIDC key response exceeds limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let set: JwkSet =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("Invalid GitHub OIDC keys"))?;
    let document: Value = serde_json::from_slice(&bytes).map_err(ApiError::internal)?;
    sqlx::query("UPDATE public.trusted_oidc_keys SET document=$1,fetched_at=now() WHERE singleton")
        .bind(document)
        .execute(&state.db)
        .await?;
    Ok(set)
}
fn verify(token: &str, audience: &str, set: &JwkSet) -> ApiResult<Claims> {
    let header = jsonwebtoken::decode_header(token).map_err(|_| ApiError::unauthorized())?;
    if header.alg != Algorithm::RS256 {
        return Err(ApiError::unauthorized());
    }
    let kid = header.kid.ok_or_else(ApiError::unauthorized)?;
    let key = set.find(&kid).ok_or_else(ApiError::unauthorized)?;
    let key = DecodingKey::from_jwk(key).map_err(|_| ApiError::unauthorized())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[audience]);
    validation.validate_nbf = true;
    validation.leeway = 15;
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "nbf", "iat", "jti"]);
    let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation)
        .map_err(|_| ApiError::unauthorized())?
        .claims;
    let now = Utc::now().timestamp();
    if claims.iat > now + 15
        || claims.iat < now - 600
        || claims.exp > claims.iat + 900
        || claims.exp <= now
        || claims.nbf > now + 15
        || claims.jti.is_empty()
        || claims.jti.len() > 200
    {
        return Err(ApiError::unauthorized());
    }
    Ok(claims)
}
fn matches(policy: &Policy, c: &Claims) -> bool {
    if !["push", "workflow_dispatch", "release"].contains(&c.event_name.as_str())
        || !c.repository.eq_ignore_ascii_case(&policy.repository)
        || c.repository_id != policy.repository_id
        || c.repository_owner_id != policy.owner_id
        || c.git_ref != policy.git_ref
        || c.environment != policy.environment
        || c.workflow_ref != format!("{}/{}@{}", c.repository, policy.workflow, policy.git_ref)
    {
        return false;
    }
    let suffix = if let Some(environment) = &policy.environment {
        format!("environment:{}", environment.replace(':', "%3A"))
    } else {
        format!("ref:{}", policy.git_ref)
    };
    let Some((owner, repo)) = c.repository.split_once('/') else {
        return false;
    };
    c.sub == format!("repo:{}:{suffix}", c.repository)
        || c.sub
            == format!(
                "repo:{owner}@{}/{repo}@{}:{suffix}",
                c.repository_owner_id, c.repository_id
            )
}
#[derive(Deserialize)]
pub struct Exchange {
    token: String,
}
pub async fn exchange(
    State(state): State<AppState>,
    Json(input): Json<Exchange>,
) -> ApiResult<Json<Value>> {
    if state.feed.is_some() {
        return Err(ApiError::not_found());
    }
    if input.token.len() > 16 * 1024 {
        return Err(ApiError::unauthorized());
    }
    let header = jsonwebtoken::decode_header(&input.token).map_err(|_| ApiError::unauthorized())?;
    if header.alg != Algorithm::RS256 {
        return Err(ApiError::unauthorized());
    }
    let kid = header
        .kid
        .filter(|k| !k.is_empty() && k.len() < 200)
        .ok_or_else(ApiError::unauthorized)?;
    let active: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.trusted_publishers WHERE enabled)")
            .fetch_one(&state.db)
            .await?;
    if !active {
        return Err(ApiError::unauthorized());
    }
    let claims = verify(
        &input.token,
        &state.config.public_url,
        &keys(&state, &kid).await?,
    )?;
    let mut tx = state.db.begin().await?;
    let rows: Vec<Policy> = sqlx::query_as(
        "SELECT * FROM public.trusted_publishers WHERE enabled ORDER BY created_at FOR SHARE",
    )
    .fetch_all(&mut *tx)
    .await?;
    let matching: Vec<_> = rows.iter().filter(|p| matches(p, &claims)).collect();
    if matching.len() != 1 {
        return Err(ApiError::forbidden(
            "OIDC token must match exactly one trusted publishing policy",
        ));
    }
    let policy = matching[0];
    if let Some(feed) = policy.feed_id {
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.feed_members m JOIN public.feeds f ON f.id=m.feed_id WHERE f.id=$1 AND m.profile_id=$2 AND m.role IN ('publisher','administrator') AND f.enabled)").bind(feed).bind(policy.profile_id).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(ApiError::forbidden(
                "Publisher feed membership was removed or disabled",
            ));
        }
    }
    let token_id = auth::hash_key(&format!("{}:{}", claims.iss, claims.jti));
    let token_expiry =
        DateTime::from_timestamp(claims.exp, 0).ok_or_else(ApiError::unauthorized)?;
    if sqlx::query("INSERT INTO public.trusted_token_uses(token_id,expires_at) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(token_id).bind(token_expiry).execute(&mut *tx).await?.rows_affected()==0{return Err(ApiError::forbidden("OIDC token already exchanged"));}
    let raw = format!("nsh_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let id = Uuid::new_v4();
    let expiry = Utc::now() + chrono::Duration::minutes(10);
    let hint = format!("{}…{}", &raw[..8], &raw[raw.len() - 4..]);
    sqlx::query("INSERT INTO public.api_keys(id,profile_id,label,key_hash,key_hint,scopes,package_prefix,expires_at,bound_feed,feed_id,trusted_publisher_id) VALUES($1,$2,$3,$4,$5,ARRAY['package:push','package:read'],$6,$7,true,$8,$9)").bind(id).bind(policy.profile_id).bind(format!("Trusted: {}",policy.name)).bind(auth::hash_key(&raw)).bind(hint).bind(&policy.package_prefix).bind(expiry).bind(policy.feed_id).bind(policy.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM public.trusted_token_uses WHERE expires_at<now()-interval '1 day'")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let source = if let Some(feed) = policy.feed_id {
        let name: String = sqlx::query_scalar("SELECT name FROM public.feeds WHERE id=$1")
            .bind(feed)
            .fetch_one(&state.db)
            .await?;
        format!("{}/feeds/{name}/v3/index.json", state.config.public_url)
    } else {
        format!("{}/v3/index.json", state.config.public_url)
    };
    Ok(Json(
        json!({"id":id,"profileId":policy.profile_id,"key":raw,"expiresAt":expiry,"source":source}),
    ))
}
