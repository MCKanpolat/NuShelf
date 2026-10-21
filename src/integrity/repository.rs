use crate::{AppState, error::ApiResult};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(super) async fn retain(tx: &mut Transaction<'_, Postgres>) -> ApiResult<()> {
    sqlx::query("DELETE FROM integrity_runs WHERE id IN (SELECT id FROM integrity_runs WHERE status='completed' ORDER BY created_at DESC,id DESC OFFSET 30)").execute(&mut **tx).await?;
    Ok(())
}

/// Snapshot references after publication has committed; serialize run creation per schema.
pub(super) async fn enqueue(state: &AppState, scheduled: bool) -> ApiResult<Option<Uuid>> {
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(current_schema()||':integrity',0))")
        .execute(&mut *tx)
        .await?;
    let active: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM integrity_runs WHERE status IN ('queued','running')")
            .fetch_optional(&mut *tx)
            .await?;
    if active.is_some() {
        tx.commit().await?;
        return Ok(if scheduled { None } else { active });
    }
    if scheduled {
        let due: bool = sqlx::query_scalar(
            "SELECT enabled AND next_scan<=now() FROM integrity_policy WHERE singleton",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !due {
            return Ok(None);
        }
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO integrity_runs(id) VALUES($1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO integrity_items(run_id,storage_key,kind,package_id,version,expected_size,expected_digest) SELECT $1,storage_key,'package',package_id,version,size,sha512 FROM package_versions UNION ALL SELECT $1,storage_key,'cache',package_id,version,size,sha512 FROM upstream_cache UNION ALL SELECT $1,storage_key,'symbol',package_id,version,size,sha256 FROM symbol_files")
        .bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE integrity_policy SET next_scan=now()+(interval_hours*interval '1 hour') WHERE singleton").execute(&mut *tx).await?;
    retain(&mut tx).await?;
    tx.commit().await?;
    Ok(Some(id))
}
