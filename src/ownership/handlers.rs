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
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

fn conflict(message: &str) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}
async fn caller(state: &AppState, headers: &HeaderMap) -> ApiResult<Option<auth::Identity>> {
    if auth::is_admin(state, headers) {
        return Ok(None);
    }
    let who = auth::authenticate(state, headers).await?;
    who.require("package:transfer")?;
    Ok(Some(who))
}
async fn lock_package(tx: &mut Transaction<'_, Postgres>, id: &str) -> ApiResult<Uuid> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    sqlx::query_scalar("SELECT owner_id FROM packages WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(ApiError::not_found)
}
async fn recipient(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    target: Uuid,
) -> ApiResult<()> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.profiles WHERE id=$1)")
            .bind(target)
            .fetch_one(&mut **tx)
            .await?;
    if !exists {
        return Err(ApiError::bad("Recipient profile does not exist"));
    }
    if let Some(feed) = state.feed {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM public.feed_members WHERE feed_id=$1 AND profile_id=$2 FOR SHARE",
        )
        .bind(feed)
        .bind(target)
        .fetch_optional(&mut **tx)
        .await?;
        if !role.is_some_and(|r| r == "publisher" || r == "administrator") {
            return Err(ApiError::bad(
                "Recipient needs publisher membership in this feed",
            ));
        }
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    target_profile_id: Uuid,
    expected_owner_id: Uuid,
}
pub async fn request(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let id = package_id(&id)?;
    let who = caller(&state, &headers).await?;
    if let Some(who) = &who {
        who.package(&id)?;
    }
    let mut tx = state.db.begin().await?;
    let owner = lock_package(&mut tx, &id).await?;
    if who.is_some_and(|w| w.profile_id != owner) {
        return Err(ApiError::forbidden(
            "Only the current owner may request a transfer",
        ));
    }
    if input.expected_owner_id != owner {
        return Err(conflict(
            "Package ownership changed; reload before transferring",
        ));
    }
    if input.target_profile_id == owner {
        return Err(ApiError::bad("Recipient already owns this package"));
    }
    recipient(&state, &mut tx, input.target_profile_id).await?;
    sqlx::query("UPDATE package_ownership_transfers SET status='expired',completed_at=now() WHERE package_id=$1 AND status='pending' AND expires_at<=now()")
        .bind(&id).execute(&mut *tx).await?;
    let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM package_ownership_transfers WHERE package_id=$1 AND status='pending')").bind(&id).fetch_one(&mut *tx).await?;
    if pending {
        return Err(conflict("A transfer is already awaiting acceptance"));
    }
    let transfer = Uuid::new_v4();
    sqlx::query("INSERT INTO package_ownership_transfers(id,package_id,source_profile,target_profile) VALUES($1,$2,$3,$4)")
        .bind(transfer).bind(&id).bind(owner).bind(input.target_profile_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":transfer,"packageId":id,"status":"pending"})),
    ))
}
pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let who = caller(&state, &headers).await?;
    let profile = who.as_ref().map(|w| w.profile_id);
    let prefix = who.as_ref().map_or("", |w| w.package_prefix.as_str());
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',t.id,'packageId',t.package_id,'sourceProfileId',source_profile,'sourceName',s.name,'targetProfileId',target_profile,'targetName',r.name,'status',CASE WHEN t.status='pending' AND expires_at<=now() THEN 'expired' ELSE t.status END,'createdAt',t.created_at,'expiresAt',expires_at,'completedAt',completed_at) FROM package_ownership_transfers t JOIN public.profiles s ON s.id=source_profile JOIN public.profiles r ON r.id=target_profile WHERE ($1::uuid IS NULL OR source_profile=$1 OR target_profile=$1) AND starts_with(package_id,$2) AND starts_with(package_id,$3) ORDER BY t.created_at DESC,t.id LIMIT 200")
        .bind(profile).bind(prefix).bind(&state.read_prefix).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
async fn complete(
    state: &AppState,
    headers: &HeaderMap,
    transfer: Uuid,
    accept: bool,
) -> ApiResult<StatusCode> {
    let who = caller(state, headers).await?;
    let id: String =
        sqlx::query_scalar("SELECT package_id FROM package_ownership_transfers WHERE id=$1")
            .bind(transfer)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(ApiError::not_found)?;
    if let Some(who) = &who {
        who.package(&id)?;
    }
    let mut tx = state.db.begin().await?;
    let owner = lock_package(&mut tx, &id).await?;
    let (source, target, pending): (Uuid,Uuid,bool) = sqlx::query_as("SELECT source_profile,target_profile,status='pending' AND expires_at>now() FROM package_ownership_transfers WHERE id=$1 FOR UPDATE")
        .bind(transfer).fetch_one(&mut *tx).await?;
    if let Some(who) = who {
        let allowed = if accept {
            who.profile_id == target
        } else {
            who.profile_id == source || who.profile_id == target
        };
        if !allowed {
            return Err(ApiError::forbidden(
                "This transfer belongs to another profile",
            ));
        }
    }
    if !pending {
        return Err(conflict("Transfer has expired or is already completed"));
    }
    if owner != source {
        return Err(conflict("Package ownership no longer matches this request"));
    }
    if accept {
        recipient(state, &mut tx, target).await?;
        sqlx::query("UPDATE packages SET owner_id=$2 WHERE id=$1")
            .bind(&id)
            .bind(target)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE package_ownership_transfers SET status=$2,completed_at=now() WHERE id=$1")
        .bind(transfer)
        .bind(if accept { "accepted" } else { "cancelled" })
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn accept(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    complete(&state, &headers, id, true).await
}
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    complete(&state, &headers, id, false).await
}
