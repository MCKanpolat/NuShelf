use crate::{
    error::{ApiError, ApiResult},
    package,
};
use bytes::Bytes;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};
#[derive(Serialize, PartialEq)]
struct File {
    size: u64,
    sha256: String,
}
struct Inventory {
    files: BTreeMap<String, File>,
    frameworks: BTreeSet<String>,
    runtimes: BTreeSet<String>,
    metadata: Value,
}
fn inventory(bytes: &Bytes) -> ApiResult<Inventory> {
    let parsed = package::parse(bytes)?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(ApiError::internal)?;
    let mut files = BTreeMap::new();
    let mut frameworks = BTreeSet::new();
    let mut runtimes = BTreeSet::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(ApiError::internal)?;
        let name = file.name().to_string();
        if file.enclosed_name().is_none()
            || name.contains(['\\', ':'])
            || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
            || files.contains_key(&name)
        {
            return Err(ApiError::bad("Package contains unsafe or duplicate paths"));
        }
        if file.is_dir() {
            continue;
        }
        let parts: Vec<_> = name.split('/').collect();
        if parts.len() >= 3 && ["lib", "ref"].contains(&parts[0]) {
            frameworks.insert(format!("{}/{}", parts[0], parts[1]));
        }
        if parts.len() >= 3 && parts[0] == "runtimes" {
            runtimes.insert(parts[1].to_string());
        }
        let mut hash = Sha256::new();
        let mut size = 0;
        let mut buf = [0u8; 32768];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|_| ApiError::bad("Invalid ZIP content"))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            size += n as u64;
            if total > 512 * 1024 * 1024 {
                return Err(ApiError::bad("Expanded archive exceeds 512 MiB"));
            }
            hash.update(&buf[..n]);
        }
        files.insert(
            name,
            File {
                size,
                sha256: hex::encode(hash.finalize()),
            },
        );
    }
    Ok(Inventory {
        files,
        frameworks,
        runtimes,
        metadata: json!({"dependencies":parsed.dependencies,"license":parsed.license_expression,"title":parsed.title,"description":parsed.description,"authors":parsed.authors,"tags":parsed.tags,"projectUrl":parsed.project_url}),
    })
}
fn set_diff(before: &BTreeSet<String>, after: &BTreeSet<String>) -> Value {
    json!({"added":after.difference(before).collect::<Vec<_>>(),"removed":before.difference(after).collect::<Vec<_>>()})
}
pub(super) fn compare(before: Bytes, after: Bytes) -> ApiResult<Value> {
    let left = inventory(&before)?;
    let right = inventory(&after)?;
    let added: Vec<_> = right
        .files
        .iter()
        .filter(|(p, _)| !left.files.contains_key(*p))
        .map(|(p, f)| json!({"path":p,"file":f}))
        .collect();
    let removed: Vec<_> = left
        .files
        .iter()
        .filter(|(p, _)| !right.files.contains_key(*p))
        .map(|(p, f)| json!({"path":p,"file":f}))
        .collect();
    let changed: Vec<_> = left
        .files
        .iter()
        .filter_map(|(p, l)| {
            right
                .files
                .get(p)
                .filter(|r| *r != l)
                .map(|r| json!({"path":p,"before":l,"after":r}))
        })
        .collect();
    let metadata: Vec<_> = left
        .metadata
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, v)| right.metadata[*k] != **v)
        .map(|(k, v)| json!({"field":k,"before":v,"after":right.metadata[k]}))
        .collect();
    Ok(
        json!({"files":{"added":added,"removed":removed,"changed":changed},"frameworks":set_diff(&left.frameworks,&right.frameworks),"runtimes":set_diff(&left.runtimes,&right.runtimes),"metadata":metadata,"compressedBytes":{"before":before.len(),"after":after.len()}}),
    )
}
