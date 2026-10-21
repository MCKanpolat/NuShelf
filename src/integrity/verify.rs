use crate::AppState;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use object_store::path::Path;
use sha2::{Digest, Sha256, Sha512};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(FromRow)]
pub(super) struct Item {
    pub run_id: Uuid,
    pub storage_key: String,
    pub kind: String,
    pub expected_size: i64,
    pub expected_digest: String,
}
pub(super) struct Outcome {
    pub status: &'static str,
    pub size: Option<i64>,
    pub digest: Option<String>,
}
impl Outcome {
    pub fn error(status: &'static str) -> Self {
        Self {
            status,
            size: None,
            digest: None,
        }
    }
}
pub(super) async fn verify(state: &AppState, item: &Item) -> Outcome {
    let object = match state
        .storage
        .get(&Path::from(item.storage_key.as_str()))
        .await
    {
        Ok(object) => object,
        Err(object_store::Error::NotFound { .. }) => return Outcome::error("missing"),
        Err(error) => {
            tracing::warn!(feed=?state.feed,run=%item.run_id,error=%error,"Integrity storage read failed");
            return Outcome::error("unavailable");
        }
    };
    let mut stream = object.into_stream();
    let mut size = 0_i64;
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                tracing::warn!(feed=?state.feed,run=%item.run_id,error=%error,"Integrity storage read failed");
                return Outcome::error("unavailable");
            }
        };
        let Some(total) = size.checked_add(chunk.len() as i64) else {
            return Outcome::error("mismatch");
        };
        size = total;
        for part in chunk.chunks(64 * 1024) {
            if item.kind == "symbol" {
                sha256.update(part);
            } else {
                sha512.update(part);
            }
        }
        tokio::task::yield_now().await;
    }
    let digest = if item.kind == "symbol" {
        hex::encode(sha256.finalize())
    } else {
        STANDARD.encode(sha512.finalize())
    };
    Outcome {
        status: if size == item.expected_size && digest == item.expected_digest {
            "ok"
        } else {
            "mismatch"
        },
        size: Some(size),
        digest: Some(digest),
    }
}
