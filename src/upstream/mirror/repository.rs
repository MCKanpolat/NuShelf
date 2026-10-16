use super::model::{Discovered, Job, Mirror};
use crate::{
    AppState,
    error::ApiResult,
    upstream::sources::{SELECT_SOURCE, Source},
};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
pub(super) async fn lock(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
) -> ApiResult<Option<Mirror>> {
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("mirror:{source}"))
            .fetch_one(&mut **tx)
            .await?;
    if !locked {
        return Ok(None);
    }
    Ok(
        sqlx::query_as("SELECT * FROM upstream_mirrors WHERE source_id=$1 FOR UPDATE")
            .bind(source)
            .fetch_optional(&mut **tx)
            .await?,
    )
}
pub(super) async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    generation: Uuid,
    items: &[Discovered],
) -> ApiResult<()> {
    // A catalog can repeat an item. Only newer events (or a fresh search scan)
    // reset completion; retrying discovery after a crash is idempotent.
    for item in items {
        sqlx::query("INSERT INTO upstream_mirror_jobs(source_id,package_id,version,action,event_time,generation) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(source_id,package_id,version) DO UPDATE SET action=EXCLUDED.action,event_time=EXCLUDED.event_time,generation=EXCLUDED.generation,done=false,skipped=false,attempts=0,last_error=NULL,next_attempt=now() WHERE EXCLUDED.event_time>upstream_mirror_jobs.event_time OR EXCLUDED.generation<>upstream_mirror_jobs.generation")
        .bind(source).bind(&item.id).bind(&item.version).bind(item.action).bind(item.at).bind(generation).execute(&mut **tx).await?;
    }
    Ok(())
}
pub(super) async fn pending(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
) -> ApiResult<Option<Job>> {
    Ok(sqlx::query_as("SELECT * FROM upstream_mirror_jobs WHERE source_id=$1 AND NOT done AND attempts<6 AND next_attempt<=now() ORDER BY event_time,package_id,version LIMIT 1 FOR UPDATE").bind(source).fetch_optional(&mut **tx).await?)
}
pub(super) async fn finish(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    job: &Job,
    skipped: bool,
) -> ApiResult<()> {
    sqlx::query("UPDATE upstream_mirror_jobs SET done=true,skipped=$4,last_error=NULL WHERE source_id=$1 AND package_id=$2 AND version=$3").bind(source).bind(&job.package_id).bind(&job.version).bind(skipped).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn status(db: &sqlx::PgPool, source: Uuid) -> ApiResult<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('sourceId',m.source_id,'enabled',m.enabled,'intervalSeconds',m.interval_seconds,'status',m.status,'requested',m.requested,'cursor',m.cursor,'discoveryComplete',m.discovery_complete,'startedAt',m.started_at,'completedAt',m.completed_at,'nextRun',m.next_run,'lastError',m.last_error,'jobs',jsonb_build_object('total',(SELECT count(*) FROM upstream_mirror_jobs WHERE source_id=m.source_id),'completed',(SELECT count(*) FROM upstream_mirror_jobs WHERE source_id=m.source_id AND done AND NOT skipped),'skipped',(SELECT count(*) FROM upstream_mirror_jobs WHERE source_id=m.source_id AND skipped),'failed',(SELECT count(*) FROM upstream_mirror_jobs WHERE source_id=m.source_id AND NOT done AND attempts>=6))) FROM upstream_mirrors m WHERE source_id=$1").bind(source).fetch_optional(db).await?.unwrap_or_else(||json!({"sourceId":source,"enabled":false,"intervalSeconds":3600,"status":"idle","requested":false,"cursor":{},"discoveryComplete":false,"jobs":{"total":0,"completed":0,"skipped":0,"failed":0}})))
}

pub(super) async fn remove(state: &AppState, source: Uuid, job: &Job) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "upstream:archive:{}/{}",
            job.package_id, job.version
        ))
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended($1,0))")
        .bind(format!("source:{source}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO storage_cleanup(storage_key) SELECT c.storage_key FROM upstream_cache c JOIN upstream_metadata m USING(package_id) WHERE m.source_id=$1 AND c.package_id=$2 AND c.version=$3 ON CONFLICT DO NOTHING").bind(source).bind(&job.package_id).bind(&job.version).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM upstream_cache c USING upstream_metadata m WHERE m.package_id=c.package_id AND m.source_id=$1 AND c.package_id=$2 AND c.version=$3").bind(source).bind(&job.package_id).bind(&job.version).execute(&mut *tx).await?;
    sqlx::query("UPDATE upstream_metadata SET versions=COALESCE((SELECT jsonb_agg(v) FROM jsonb_array_elements(versions) v WHERE v<>to_jsonb($3::text)),'[]'),catalog_checked_at=NULL,expires_at=now() WHERE source_id=$1 AND package_id=$2").bind(source).bind(&job.package_id).bind(&job.version).execute(&mut *tx).await?;
    sqlx::query("SELECT refresh_upstream_search($1)")
        .bind(&job.package_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn pin(db: &sqlx::PgPool, id: &str) -> ApiResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT source_id FROM upstream_metadata WHERE package_id=$1")
            .bind(id)
            .fetch_optional(db)
            .await?,
    )
}
pub(super) async fn retain(db: &sqlx::PgPool, id: &str, version: &str) -> ApiResult<()> {
    sqlx::query("UPDATE upstream_cache SET mirrored=true WHERE package_id=$1 AND version=$2")
        .bind(id)
        .bind(version)
        .execute(db)
        .await?;
    Ok(())
}
pub(super) async fn source(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Source> {
    Ok(sqlx::query_as(&format!("{SELECT_SOURCE} WHERE id=$1"))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?)
}
pub(super) async fn due(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT next_run<=now() FROM upstream_mirrors WHERE source_id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?,
    )
}
pub(super) async fn reset(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    url: &str,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM upstream_mirror_jobs WHERE source_id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE upstream_mirrors SET source_url=$2,cursor='{}',discovery_complete=false WHERE source_id=$1").bind(id).bind(url).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn start(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    generation: Uuid,
    cursor: &Value,
) -> ApiResult<()> {
    sqlx::query("UPDATE upstream_mirrors SET status='running',requested=true,generation=$2,cursor=$3,discovery_complete=false,started_at=now(),completed_at=NULL,last_error=NULL WHERE source_id=$1").bind(id).bind(generation).bind(cursor).execute(&mut **tx).await?;
    sqlx::query("UPDATE upstream_mirror_jobs SET attempts=0,next_attempt=now(),last_error=NULL WHERE source_id=$1 AND NOT done").bind(id).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn fail_job(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    job: &Job,
    error: &str,
) -> ApiResult<()> {
    let attempts = job.attempts + 1;
    let seconds = 2_i32.pow(attempts.min(10) as u32);
    sqlx::query("UPDATE upstream_mirror_jobs SET attempts=$4,last_error=$5,next_attempt=now()+make_interval(secs=>$6::double precision) WHERE source_id=$1 AND package_id=$2 AND version=$3").bind(id).bind(&job.package_id).bind(&job.version).bind(attempts).bind(error).bind(seconds as f64).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    cursor: &Value,
    complete: bool,
) -> ApiResult<()> {
    sqlx::query("UPDATE upstream_mirrors SET cursor=$2,discovery_complete=$3 WHERE source_id=$1")
        .bind(id)
        .bind(cursor)
        .bind(complete)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub(super) async fn fail_scan(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    error: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE upstream_mirrors SET requested=false,status='failed',last_error=$2,next_run=now()+make_interval(secs=>interval_seconds) WHERE source_id=$1").bind(id).bind(error).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn settle(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<bool> {
    let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upstream_mirror_jobs WHERE source_id=$1 AND NOT done AND attempts<6)").bind(id).fetch_one(&mut **tx).await?;
    if waiting {
        return Ok(false);
    }
    sqlx::query("UPDATE upstream_mirrors SET requested=false,status=CASE WHEN EXISTS(SELECT 1 FROM upstream_mirror_jobs WHERE source_id=$1 AND NOT done) THEN 'failed' ELSE 'completed' END,completed_at=now(),next_run=now()+make_interval(secs=>interval_seconds),last_error=CASE WHEN EXISTS(SELECT 1 FROM upstream_mirror_jobs WHERE source_id=$1 AND NOT done) THEN 'Some packages failed after six attempts; inspect jobs before retrying' ELSE NULL END WHERE source_id=$1").bind(id).execute(&mut **tx).await?;
    Ok(true)
}
