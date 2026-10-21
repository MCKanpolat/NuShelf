//! Transactional per-registry publication quotas. Upstream cache is accounted separately.
mod handlers;
mod worker;
use crate::error::ApiResult;
pub use handlers::{status, update};
use sqlx::{Postgres, Transaction};
pub use worker::worker;

/// Hold the usage row until publication commits, checking quota before blob writes.
/// The same SQL function is used by accounting triggers, including non-HTTP writes.
pub async fn reserve(
    tx: &mut Transaction<'_, Postgres>,
    packages: i64,
    versions: i64,
    archives: i64,
    symbols: i64,
) -> ApiResult<()> {
    sqlx::query("SELECT check_publication_quota($1,$2,$3,$4)")
        .bind(packages)
        .bind(versions)
        .bind(archives)
        .bind(symbols)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
