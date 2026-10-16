use super::{
    filter::field,
    model::{Options, boolean, number},
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::sources::Reader,
};
use serde_json::Value;
use sqlx::{Arguments, Execute, Postgres, QueryBuilder};
// Only visible archives are included. Upstream routing, grants, local precedence,
// and named-feed key prefixes are checked before OData filtering and paging.
pub(super) fn base_query(
    state: &AppState,
    reader: &Reader,
    semver2: bool,
    extras: Value,
) -> ApiResult<QueryBuilder<'static, Postgres>> {
    let mut arguments = sqlx::postgres::PgArguments::default();
    arguments.add(reader.admin).map_err(ApiError::internal)?;
    arguments.add(reader.profile).map_err(ApiError::internal)?;
    arguments
        .add(reader.prefix.clone())
        .map_err(ApiError::internal)?;
    arguments.add(reader.profile).map_err(ApiError::internal)?;
    arguments.add(extras).map_err(ApiError::internal)?;
    arguments
        .add(state.read_prefix.clone())
        .map_err(ApiError::internal)?;
    arguments.add(semver2).map_err(ApiError::internal)?;
    Ok(QueryBuilder::with_arguments(
        include_str!("sql/versions.sql"),
        arguments,
    ))
}

pub(super) struct Page {
    pub rows: Vec<Value>,
    pub total: i64,
    pub single: bool,
    pub count: bool,
    pub take: i64,
    pub skip: i64,
    pub requested: i64,
}
pub(super) async fn fetch(
    state: &AppState,
    mut b: QueryBuilder<'_, Postgres>,
    opts: &Options,
    path: &str,
    single: bool,
    count: bool,
) -> ApiResult<Page> {
    let skip = number(opts, "$skip", 0, 100_000)?;
    let requested = number(opts, "$top", 100, 100_000)?;
    let take = requested.min(100);
    // Reuse one repeatable snapshot for count and data.
    let mut tx = state.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let mut sql = b.sql().to_string();
    if matches!(path, "GetUpdates" | "GetUpdates()") && !boolean(opts, "includeAllVersions", false)?
    {
        sql = format!(
            "SELECT DISTINCT ON (id) data,id,version_sort FROM ({sql}) updates ORDER BY id,version_sort DESC"
        );
    }
    let mut page = QueryBuilder::<Postgres>::new(format!("SELECT data FROM ({sql}) results"));
    let count_sql = format!("SELECT count(*) FROM ({sql}) counted");
    let args = b
        .build()
        .take_arguments()
        .map_err(ApiError::internal)?
        .unwrap();
    let total: i64 = sqlx::query_scalar_with(&count_sql, args.clone())
        .fetch_one(&mut *tx)
        .await
        .map_err(query_error)?;
    if count {
        return Ok(Page {
            rows: Vec::new(),
            total,
            single,
            count,
            take,
            skip,
            requested,
        });
    }
    page.push(" ORDER BY ");
    if let Some(order) = opts.get("$orderby") {
        if order.len() > 256 {
            return Err(ApiError::bad("Order is too long"));
        }
        for term in order.split(',') {
            let parts = term.split_whitespace().collect::<Vec<_>>();
            if parts.is_empty()
                || parts.len() > 2
                || parts.get(1).is_some_and(|s| !matches!(*s, "asc" | "desc"))
            {
                return Err(ApiError::bad("Invalid OData order"));
            }
            page.push(field(parts[0])?);
            if parts.get(1) == Some(&"desc") {
                page.push(" DESC");
            }
            page.push(",");
        }
    }
    page.push("id,version_sort");
    // Keep the same bound arguments for the count and the page.
    let sql = format!(
        "{} LIMIT {} OFFSET {}",
        page.sql(),
        if single { 1 } else { take + 1 },
        skip
    );

    let rows: Vec<Value> = sqlx::query_scalar_with(&sql, args)
        .fetch_all(&mut *tx)
        .await
        .map_err(query_error)?;
    tx.commit().await?;
    Ok(Page {
        rows,
        total,
        single,
        count,
        take,
        skip,
        requested,
    })
}

fn query_error(e: sqlx::Error) -> ApiError {
    if let sqlx::Error::Database(ref error) = e
        && error
            .code()
            .is_some_and(|c| c.starts_with("22") || c == "42883" || c == "42804")
    {
        return ApiError::bad("Invalid OData operand or field type");
    }
    ApiError::from(e)
}
