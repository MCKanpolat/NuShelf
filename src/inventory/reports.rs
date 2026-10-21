use super::{access, graph};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    take: Option<i64>,
    after: Option<String>,
}
impl Page {
    fn validate(&self) -> ApiResult<i64> {
        let take = self.take.unwrap_or(100);
        if !(1..=200).contains(&take) || self.after.as_ref().is_some_and(|v| v.len() > 1024) {
            return Err(ApiError::bad(
                "Use take 1–200 and an after reference up to 1024 bytes",
            ));
        }
        Ok(take)
    }
}
fn affected(advisories: &Value, version: &str) -> Vec<Value> {
    advisories
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| crate::audit::affected(a["versions"].as_str().unwrap_or_default(), version))
        .cloned()
        .collect()
}
pub async fn release(
    State(state): State<AppState>,
    Path((application, release)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    access::require(&state, &headers, false)
        .await?
        .application(&state, application)
        .await?;
    let take = page.validate()?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let info:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'version',version,'repositoryUrl',repository_url,'commitSha',commit_sha,'sbomHash',sbom_hash,'rootRef',root_ref,'componentCount',component_count,'nugetCount',nuget_count,'edgeCount',edge_count,'createdAt',created_at) FROM inventory_releases WHERE id=$1 AND application_id=$2").bind(release).bind(application).fetch_optional(&mut *tx).await?;
    let info = info.ok_or_else(ApiError::not_found)?;
    let snapshot:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('hash',hash,'createdAt',created_at) FROM public.audit_snapshots WHERE current").fetch_optional(&mut *tx).await?;
    let edges:Vec<(String,String)>=sqlx::query_as("SELECT parent_ref,child_ref FROM inventory_edges WHERE release_id=$1 ORDER BY parent_ref,child_ref").bind(release).fetch_all(&mut *tx).await?;
    let names: Vec<(String, String)> =
        sqlx::query_as("SELECT bom_ref,name FROM inventory_components WHERE release_id=$1")
            .bind(release)
            .fetch_all(&mut *tx)
            .await?;
    let names: HashMap<_, _> = names.into_iter().collect();
    let mut rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('ref',c.bom_ref,'name',c.name,'purl',c.purl,'packageId',c.package_id,'version',c.version,'advisories',COALESCE(s.data->c.package_id,'[]'::jsonb)) FROM inventory_components c LEFT JOIN public.audit_snapshots s ON s.current WHERE c.release_id=$1 AND ($2::text IS NULL OR c.bom_ref>$2) ORDER BY c.bom_ref LIMIT $3").bind(release).bind(&page.after).bind(take+1).fetch_all(&mut *tx).await?;
    let more = rows.len() > take as usize;
    rows.truncate(take as usize);
    let root = info["rootRef"].as_str();
    let parents = graph::parents(root, &edges);
    for row in &mut rows {
        let reference = row["ref"].as_str().unwrap_or_default().to_owned();
        let path = graph::path(root, &reference, &parents);
        let relationship = if root == Some(reference.as_str()) {
            "application"
        } else if parents.get(&reference).map(String::as_str) == root && root.is_some() {
            "direct"
        } else if path.is_some() {
            "transitive"
        } else {
            "unrooted"
        };
        let full = path.unwrap_or_default();
        let truncated = full.len() > 64;
        let selected: Vec<_> = if truncated {
            full[..32]
                .iter()
                .chain(full[full.len() - 32..].iter())
                .collect()
        } else {
            full.iter().collect()
        };
        row["dependencyPath"] = json!(
            selected
                .into_iter()
                .map(|r| json!({"ref":r,"name":names.get(r)}))
                .collect::<Vec<_>>()
        );
        row["pathTruncated"] = json!(truncated);
        row["relationship"] = json!(relationship);
        row["advisories"] = json!(affected(
            &row["advisories"],
            row["version"].as_str().unwrap_or_default()
        ));
        row["assessment"] = json!(if relationship == "application" {
            "application_root"
        } else if snapshot.is_none() {
            "advisories_unavailable"
        } else if row["packageId"].is_null() {
            "unsupported_ecosystem"
        } else {
            "matched_against_snapshot"
        });
    }
    let next = if more {
        rows.last().map(|r| r["ref"].clone())
    } else {
        None
    };
    tx.commit().await?;
    Ok(Json(
        json!({"release":info,"advisorySnapshot":snapshot,"items":rows,"nextAfter":next,"inventorySource":"CI-reported CycloneDX; completeness and runtime reachability are not verified"}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diff {
    baseline: Uuid,
    target: Uuid,
}
pub async fn diff(
    State(state): State<AppState>,
    Path(application): Path<Uuid>,
    headers: HeaderMap,
    Query(input): Query<Diff>,
) -> ApiResult<Json<Value>> {
    access::require(&state, &headers, false)
        .await?
        .application(&state, application)
        .await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let mut sets = Vec::new();
    for release in [input.baseline, input.target] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM inventory_releases WHERE id=$1 AND application_id=$2)",
        )
        .bind(release)
        .bind(application)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(ApiError::not_found());
        }
        let values:Vec<(String,String)>=sqlx::query_as("SELECT DISTINCT package_id,version FROM inventory_components WHERE release_id=$1 AND package_id IS NOT NULL ORDER BY package_id,version").bind(release).fetch_all(&mut *tx).await?;
        let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (id, v) in values {
            map.entry(id).or_default().insert(v);
        }
        sets.push(map);
    }
    tx.commit().await?;
    let before = &sets[0];
    let after = &sets[1];
    let added: Vec<_> = after
        .iter()
        .filter(|(id, _)| !before.contains_key(*id))
        .map(|(id, v)| json!({"packageId":id,"versions":v}))
        .collect();
    let removed: Vec<_> = before
        .iter()
        .filter(|(id, _)| !after.contains_key(*id))
        .map(|(id, v)| json!({"packageId":id,"versions":v}))
        .collect();
    let changed: Vec<_> = before
        .iter()
        .filter_map(|(id, v)| {
            after
                .get(id)
                .filter(|next| *next != v)
                .map(|next| json!({"packageId":id,"before":v,"after":next}))
        })
        .collect();
    Ok(Json(
        json!({"baseline":input.baseline,"target":input.target,"ecosystem":"nuget","added":added,"removed":removed,"changed":changed}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Impact {
    take: Option<i64>,
    cursor: Option<String>,
    #[serde(default = "yes")]
    latest_only: bool,
}
fn yes() -> bool {
    true
}
#[derive(Deserialize, Serialize)]
struct Cursor {
    hash: String,
    release: Uuid,
    reference: String,
}
pub async fn impact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(input): Query<Impact>,
) -> ApiResult<Json<Value>> {
    let access = access::require(&state, &headers, false).await?;
    let take = input.take.unwrap_or(100);
    if !(1..=200).contains(&take) || input.cursor.as_ref().is_some_and(|s| s.len() > 4096) {
        return Err(ApiError::bad("Invalid impact pagination"));
    }
    let cursor: Option<Cursor> = input
        .cursor
        .as_ref()
        .map(|v| {
            URL_SAFE_NO_PAD
                .decode(v)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .ok_or_else(|| ApiError::bad("Invalid impact cursor"))
        })
        .transpose()?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let snapshot:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('hash',hash,'createdAt',created_at) FROM public.audit_snapshots WHERE current").fetch_optional(&mut *tx).await?;
    if cursor.as_ref().is_some_and(|c| {
        snapshot.as_ref().and_then(|s| s["hash"].as_str()) != Some(c.hash.as_str())
    }) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Advisory snapshot changed; reload impact results".into(),
        ));
    }
    let mut candidates: Vec<Value> = sqlx::query_scalar(include_str!("impact.sql"))
        .bind(access.manager)
        .bind(access.profile)
        .bind(input.latest_only)
        .bind(cursor.as_ref().map(|c| c.release))
        .bind(cursor.as_ref().map(|c| c.reference.as_str()))
        .bind(take + 1)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let more = candidates.len() > take as usize;
    candidates.truncate(take as usize);
    let next = if more {
        candidates
            .last()
            .and_then(|r| {
                Some(Cursor {
                    hash: snapshot.as_ref()?["hash"].as_str()?.into(),
                    release: Uuid::parse_str(r["releaseId"].as_str()?).ok()?,
                    reference: r["ref"].as_str()?.into(),
                })
            })
            .map(|c| serde_json::to_vec(&c).map(|bytes| URL_SAFE_NO_PAD.encode(bytes)))
            .transpose()
            .map_err(ApiError::internal)?
    } else {
        None
    };
    let scanned = candidates.len();
    let items: Vec<_> = candidates
        .into_iter()
        .filter_map(|mut row| {
            let matches = affected(
                &row["advisories"],
                row["version"].as_str().unwrap_or_default(),
            );
            if matches.is_empty() {
                None
            } else {
                row["advisories"] = json!(matches);
                Some(row)
            }
        })
        .collect();
    Ok(Json(
        json!({"advisorySnapshot":snapshot,"latestOnly":input.latest_only,"items":items,"scannedCandidates":scanned,"nextCursor":next,"assessment":"NuGet version-range matches in CI-reported inventories; absence of matches does not establish safety or runtime reachability"}),
    ))
}
