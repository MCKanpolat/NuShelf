use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::{
        cache,
        sources::{self, Reader, SELECT_SOURCE, Source},
    },
    version::{NuGetVersion, package_id},
};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    id: String,
    version: String,
    #[serde(default)]
    probe: bool,
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub async fn explain(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(input): Query<Input>,
) -> ApiResult<Json<Value>> {
    let id = package_id(&input.id)?;
    let version = NuGetVersion::parse(&input.version)?.normalized;
    crate::feeds::check_package(&state, &id)?;
    let reader = Reader::from_headers(&state, &headers).await?;
    if !reader.authenticated {
        return Err(ApiError::unauthorized());
    }
    if !id.starts_with(&reader.prefix) {
        return Err(ApiError::forbidden("Package is outside this key's prefix"));
    }
    let mut steps = Vec::new();
    let code;
    if cache::local_exists(&state, &id).await? {
        let listed: Option<bool> = sqlx::query_scalar(
            "SELECT listed FROM package_versions WHERE package_id=$1 AND version=$2",
        )
        .bind(&id)
        .bind(&version)
        .fetch_optional(&state.db)
        .await?;
        code = match listed {
            Some(true) => "local_available",
            Some(false) => "local_unlisted",
            None => "local_version_missing",
        }
        .to_string();
        steps.push(json!({"stage":"local","code":code,"message":"Local namespaces never fall back to upstreams. Unlisted versions remain downloadable by exact version."}));
    } else {
        let rows = sqlx::query_as::<_, Source>(&format!("{SELECT_SOURCE} ORDER BY priority,id"))
            .fetch_all(&state.db)
            .await?;
        let candidates =
            sources::select_candidates(rows.iter().filter(|s| s.enabled).cloned().collect(), &id);
        let pin: Option<Uuid> =
            sqlx::query_scalar("SELECT source_id FROM upstream_metadata WHERE package_id=$1")
                .bind(&id)
                .fetch_optional(&state.db)
                .await?;
        let metadata: Option<cache::Metadata> =
            sqlx::query_as("SELECT * FROM upstream_metadata WHERE package_id=$1")
                .bind(&id)
                .fetch_optional(&state.db)
                .await?;
        let mut eligible = Vec::new();
        for source in &rows {
            let selected = candidates.iter().any(|s| s.id == source.id);
            let allowed = reader.check(source, &id).is_ok();
            // Do not disclose private source identities, rules or pins without a grant.
            let visible = !source.private || allowed || reader.admin;
            let reason = if !source.enabled {
                "disabled"
            } else if source
                .excludes
                .iter()
                .any(|p| sources::matching(p, &id).is_some())
            {
                "excluded"
            } else if !source
                .patterns
                .iter()
                .any(|p| sources::matching(p, &id).is_some())
            {
                "pattern_mismatch"
            } else if !selected {
                "less_specific_rule"
            } else if pin.is_some_and(|p| p != source.id) {
                "different_origin_pin"
            } else if !allowed {
                "access_denied"
            } else {
                "candidate"
            };
            if visible {
                steps.push(json!({"stage":"routing","sourceId":source.id,"source":source.name,"priority":source.priority,"patterns":source.patterns,"excludes":source.excludes,"pinned":pin==Some(source.id),"code":reason}));
            } else if selected && pin.is_none_or(|p| p == source.id) {
                steps.push(json!({"stage":"routing","code":"access_denied","message":"A selected private source requires package:read and a profile grant."}));
            }
            if selected && pin.is_none_or(|p| p == source.id) {
                eligible.push(source);
            }
        }
        if pin.is_some() && eligible.is_empty() {
            code = "origin_policy_conflict".to_string();
        } else if eligible.is_empty() {
            code = "no_matching_source".to_string();
        } else {
            let mut outcome = "upstream_not_probed".to_string();
            for source in eligible {
                if reader.check(source, &id).is_err() {
                    outcome = "access_denied".into();
                    break;
                }
                let cached: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upstream_cache WHERE package_id=$1 AND version=$2)").bind(&id).bind(&version).fetch_one(&state.db).await?;
                if cached {
                    outcome = "cached_available".into();
                    break;
                }
                let missed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upstream_misses WHERE package_id=$1 AND source_id=$2 AND expires_at>now())").bind(&id).bind(source.id).fetch_one(&state.db).await?;
                if missed && pin.is_none() {
                    steps.push(json!({"stage":"metadata","source":source.name,"code":"negative_cache","message":"Resolver skips this source until its cached miss expires."}));
                    outcome = "package_missing".into();
                    continue;
                }
                if !input.probe {
                    if let Some(meta) = &metadata {
                        let fresh = meta.expires_at > chrono::Utc::now();
                        let available = meta
                            .versions
                            .as_array()
                            .is_some_and(|v| v.contains(&json!(version)));
                        steps.push(json!({"stage":"metadata","source":source.name,"code":if fresh {"metadata_fresh"} else {"metadata_expired"},"expiresAt":meta.expires_at,"containsVersion":available}));
                        if fresh {
                            outcome = if available {
                                "metadata_version_available"
                            } else {
                                "version_missing"
                            }
                            .into();
                        }
                    }
                    break;
                }
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(35),
                    super::probe::versions(&state, source, &id, &version),
                )
                .await;
                match result {
                    Ok(Ok(value)) => {
                        outcome = value["code"].as_str().unwrap_or("upstream_error").into();
                        let next = value["continue"] == true && pin.is_none();
                        steps.push(json!({"stage":"probe","source":source.name,"result":value}));
                        if !next {
                            break;
                        }
                    }
                    other => {
                        let message = match other {
                            Ok(Err(e)) => e.1,
                            _ => "Upstream probe timed out".into(),
                        };
                        steps.push(json!({"stage":"probe","source":source.name,"code":"upstream_error","message":message}));
                        outcome = "upstream_error".into();
                        break;
                    }
                }
            }
            code = outcome;
        }
    }
    let config = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<configuration>\n  <packageSources><clear/><add key=\"NuShelf\" value=\"{}/v3/index.json\" /></packageSources>\n  <packageSourceMapping><packageSource key=\"NuShelf\"><package pattern=\"*\" /></packageSource></packageSourceMapping>\n</configuration>",
        xml(&state.config.public_url)
    );
    Ok(Json(
        json!({"packageId":id,"version":version,"code":code,"steps":steps,"nugetConfig":config,"scope":"current caller and feed","readOnly":true,"limitations":"Point-in-time policy and metadata diagnosis; does not execute NuGet restore, verify archive availability, or inspect the client's global packages folder. A live probe bypasses metadata freshness and stale fallback."}),
    ))
}
