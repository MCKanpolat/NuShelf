use super::{
    model::{Discovered, Job},
    repository,
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::{
        cache,
        sources::{self, Reader, Source},
    },
};
pub(super) struct Outcome {
    pub skipped: bool,
    pub discovered: Vec<Discovered>,
}
pub(super) async fn process(state: &AppState, source: &Source, job: &Job) -> ApiResult<Outcome> {
    let skipped = || Outcome {
        skipped: true,
        discovered: Vec::new(),
    };
    let done = || Outcome {
        skipped: false,
        discovered: Vec::new(),
    };
    if cache::local_exists(state, &job.package_id).await?
        || !sources::candidates(state, &job.package_id)
            .await?
            .iter()
            .any(|s| s.id == source.id)
    {
        return Ok(skipped());
    }
    let pin = repository::pin(&state.db, &job.package_id).await?;
    if pin.is_some_and(|p| p != source.id) {
        return Ok(skipped());
    }
    if job.action == "delete" {
        repository::remove(state, source.id, job).await?;
        return Ok(done());
    }
    // Source-pinned fetches prevent a failed preferred source from causing a
    // mirror to import another source under the wrong administrative policy.
    let reader = Reader {
        admin: true,
        ..Default::default()
    };
    if job.version.is_empty() {
        let (meta, actual) =
            cache::metadata_from_source(state, &reader, &job.package_id, source.id).await?;
        if actual.id != source.id {
            return Err(ApiError::internal("Mirror source changed"));
        }
        let mut discovered = Vec::new();
        for v in meta.versions.as_array().into_iter().flatten() {
            let version = crate::version::NuGetVersion::parse(
                v.as_str()
                    .ok_or_else(|| ApiError::internal("Invalid stored upstream version"))?,
            )?
            .normalized;
            discovered.push(Discovered {
                id: job.package_id.clone(),
                version,
                action: "sync",
                at: job.event_time,
            });
        }
        return Ok(Outcome {
            skipped: false,
            discovered,
        });
    }
    // Refresh registration even on cache hits so listing/deprecation changes propagate.
    let (_, actual) =
        cache::catalog_from_source(state, &reader, &job.package_id, source.id).await?;
    if actual.id != source.id {
        return Err(ApiError::internal("Mirror source changed"));
    }
    let (_, actual, _) = cache::ensure(state, &reader, &job.package_id, &job.version).await?;
    if actual.id != source.id {
        return Err(ApiError::internal("Mirror source changed"));
    }
    repository::retain(&state.db, &job.package_id, &job.version).await?;
    Ok(done())
}
