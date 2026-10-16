use super::{application, model::Options, protocol, query, repository};
use crate::{
    AppState,
    error::ApiResult,
    upstream::{self, sources::Reader},
    version::{NuGetVersion, package_id},
};
use axum::{
    extract::{OriginalUri, Path, Query, State},
    http::{HeaderMap, Method},
    response::Response,
};
pub async fn download(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let id = package_id(&id)?;
    crate::feeds::check_package(&state, &id)?;
    let version = NuGetVersion::parse(&version)?.normalized;
    upstream::download(
        State(state),
        Path((id.clone(), version.clone(), format!("{id}.{version}.nupkg"))),
        method,
        headers,
    )
    .await
}

pub async fn service(State(state): State<AppState>) -> Response {
    protocol::service(&state)
}
pub async fn metadata() -> Response {
    protocol::metadata()
}
pub async fn endpoint(
    State(state): State<AppState>,
    Path(path): Path<String>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(opts): Query<Options>,
) -> ApiResult<Response> {
    query::validate(&opts)?;
    let reader = Reader::from_headers(&state, &headers).await?;
    let count = path.ends_with("/$count");
    let path = path.strip_suffix("/$count").unwrap_or(&path);
    let (query, single) = application::build(&state, &reader, path, &opts).await?;
    let page = repository::fetch(&state, query, &opts, path, single, count).await?;
    protocol::render(&state, &uri, &opts, path, page)
}
