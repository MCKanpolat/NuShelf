use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    registry,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub const PROTOCOL: &str = "2025-06-18";
fn rpc_error(id: Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}
async fn authorize(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if headers.get("origin").is_some_and(|origin| {
        origin.to_str().ok()
            != url::Url::parse(&state.config.public_url)
                .ok()
                .map(|u| u.origin().ascii_serialization())
                .as_deref()
    }) {
        return Err(ApiError::forbidden("Origin is not allowed"));
    }
    auth::authenticate(state, headers)
        .await?
        .require("mcp:read")
}
pub async fn endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> ApiResult<Response> {
    authorize(&state, &headers).await?;
    let reader = crate::upstream::sources::Reader::from_headers(&state, &headers).await?;
    if !headers
        .get("accept")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|a| a.contains("application/json") && a.contains("text/event-stream"))
    {
        return Err(ApiError(
            StatusCode::NOT_ACCEPTABLE,
            "Accept must include application/json and text/event-stream".into(),
        ));
    }
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    if message["jsonrpc"] != "2.0"
        || !message.is_object()
        || message
            .get("id")
            .is_some_and(|id| !(id.is_string() || id.is_number() || id.is_null()))
    {
        return Ok(rpc_error(id, -32600, "Invalid JSON-RPC request"));
    }
    // Transport may receive a JSON-RPC response. This server never initiates calls,
    // but acknowledges a well-formed response rather than sending another response.
    if message.get("method").is_none()
        && message.get("id").is_some()
        && (message.get("result").is_some() ^ message.get("error").is_some())
    {
        return Ok(StatusCode::ACCEPTED.into_response());
    }
    let Some(method) = message["method"].as_str() else {
        return Ok(rpc_error(id, -32600, "Invalid JSON-RPC request"));
    };
    if method != "initialize"
        && headers
            .get("mcp-protocol-version")
            .and_then(|h| h.to_str().ok())
            != Some(PROTOCOL)
    {
        return Err(ApiError::bad("MCP-Protocol-Version must be 2025-06-18"));
    }
    if message.get("id").is_none() {
        return Ok(StatusCode::ACCEPTED.into_response());
    }
    let params = &message["params"];
    let result = match method {
        "initialize" => {
            if params["protocolVersion"].as_str().is_none()
                || !params["capabilities"].is_object()
                || !params["clientInfo"].is_object()
            {
                return Ok(rpc_error(
                    id,
                    -32602,
                    "Initialization requires protocolVersion, capabilities and clientInfo",
                ));
            }
            json!({"protocolVersion":PROTOCOL,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"nushelf","version":env!("CARGO_PKG_VERSION")},"instructions":"Search and inspect packages in this registry. Tools are read-only; package publication uses the NuGet API."})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":[
            {"name":"search_packages","description":"Search listed NuGet packages by ID, tags or description.","inputSchema":{"type":"object","properties":{"query":{"type":"string","maxLength":256},"prerelease":{"type":"boolean","default":false},"take":{"type":"integer","minimum":1,"maximum":100}},"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}},
            {"name":"get_package","description":"Inspect package metadata, dependencies and all published versions.","inputSchema":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}},
            {"name":"list_versions","description":"List all versions, including unlisted versions, in NuGet precedence order.","inputSchema":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}
        ]}),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Ok(rpc_error(id, -32602, "Tool name is required"));
            };
            let empty = json!({});
            let args = params.get("arguments").unwrap_or(&empty);
            if !args.is_object() {
                return Ok(rpc_error(id, -32602, "Tool arguments must be an object"));
            }
            let value = match name {
                "search_packages" => {
                    if args.get("query").is_some_and(|v| !v.is_string())
                        || args.get("prerelease").is_some_and(|v| !v.is_boolean())
                        || args.get("take").is_some_and(|v| !v.is_i64())
                        || args
                            .as_object()
                            .unwrap()
                            .keys()
                            .any(|k| !["query", "prerelease", "take"].contains(&k.as_str()))
                    {
                        return Ok(rpc_error(id, -32602, "Invalid search arguments"));
                    }
                    crate::upstream::search_value(
                        &state,
                        &reader,
                        &registry::SearchQuery {
                            q: args["query"].as_str().unwrap_or_default().into(),
                            prerelease: args["prerelease"].as_bool().unwrap_or(false),
                            take: args["take"].as_i64(),
                            sem_ver_level: Some("2.0.0".into()),
                            ..Default::default()
                        },
                    )
                    .await
                }
                "get_package" | "list_versions" => {
                    let Some(package_id) = args["id"].as_str() else {
                        return Ok(rpc_error(id, -32602, "Package id is required"));
                    };
                    if args.as_object().unwrap().keys().any(|k| k != "id") {
                        return Ok(rpc_error(id, -32602, "Unknown tool argument"));
                    }
                    if name == "get_package" {
                        crate::upstream::details_value(&state, &reader, package_id).await
                    } else {
                        crate::upstream::versions_value(&state, &reader, package_id).await
                    }
                }
                _ => return Ok(rpc_error(id, -32602, "Unknown tool")),
            };
            match value {
                Ok(value) => {
                    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false})
                }
                Err(error) => json!({"content":[{"type":"text","text":error.1}],"isError":true}),
            }
        }
        _ => return Ok(rpc_error(id, -32601, "Method not found")),
    };
    Ok(Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response())
}
// Stateless JSON transport: no standalone SSE stream or resumable sessions.
pub async fn no_stream(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<StatusCode> {
    authorize(&state, &headers).await?;
    Ok(StatusCode::METHOD_NOT_ALLOWED)
}
