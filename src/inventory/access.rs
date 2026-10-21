use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::http::HeaderMap;
use uuid::Uuid;

pub(super) struct Access {
    pub profile: Option<Uuid>,
    pub manager: bool,
}
impl Access {
    pub async fn application(&self, state: &AppState, id: Uuid) -> ApiResult<()> {
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inventory_applications WHERE id=$1 AND ($2 OR owner_id=$3))").bind(id).bind(self.manager).bind(self.profile).fetch_one(&state.db).await?;
        if exists {
            Ok(())
        } else {
            Err(ApiError::not_found())
        }
    }
}
pub(super) async fn require(
    state: &AppState,
    headers: &HeaderMap,
    write: bool,
) -> ApiResult<Access> {
    if auth::is_admin(state, headers) {
        return Ok(Access {
            profile: None,
            manager: true,
        });
    }
    let who = auth::authenticate(state, headers).await?;
    who.require(if write {
        "inventory:write"
    } else {
        "inventory:read"
    })?;
    if !who.package_prefix.is_empty() {
        return Err(ApiError::forbidden(
            "Application inventories require an unrestricted package prefix",
        ));
    }
    let mut manager = false;
    if let Some(feed) = state
        .feed
        .filter(|_| who.scopes.iter().any(|s| s == "feeds:manage"))
    {
        manager=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.feed_members WHERE feed_id=$1 AND profile_id=$2 AND role='administrator')").bind(feed).bind(who.profile_id).fetch_one(&state.db).await?;
    }
    Ok(Access {
        profile: Some(who.profile_id),
        manager,
    })
}
