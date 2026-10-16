//! Offline export and import of the package store.
//!
//! Package archives are immutable and written under fresh keys, so copying
//! them needs no coordination with a running server. The tool works against
//! any storage provider and keeps a SHA-256 manifest so a restore can prove
//! the copy is intact. PostgreSQL is backed up separately (see
//! `scripts/backup.sh`); take the database dump first and the storage copy
//! second, because every committed row points at a blob that already exists.
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use object_store::{ObjectStore, WriteMultipart, path::Path as StorePath};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MANIFEST: &str = "manifest.json";
const OBJECTS: &str = "objects";
const FORMAT: u32 = 1;
const CHUNK: usize = 8 * 1024 * 1024;
/// Written by the readiness probe; never part of a backup.
const PROBE_PREFIX: &str = ".nushelf-readiness-probe";

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    #[serde(rename = "createdAt")]
    created_at: String,
    objects: Vec<Entry>,
}
#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    key: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub copied: usize,
    pub skipped: usize,
    pub bytes: u64,
}

fn object_file(dir: &Path, key: &str) -> PathBuf {
    dir.join(OBJECTS).join(key)
}

async fn read_manifest(dir: &Path) -> Result<Option<Manifest>> {
    match tokio::fs::read(dir.join(MANIFEST)).await {
        Ok(bytes) => {
            let manifest: Manifest = serde_json::from_slice(&bytes).context("invalid manifest")?;
            ensure!(
                manifest.format == FORMAT,
                "unsupported backup format {}",
                manifest.format
            );
            Ok(Some(manifest))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

async fn write_manifest(dir: &Path, objects: Vec<Entry>) -> Result<()> {
    let manifest = Manifest {
        format: FORMAT,
        created_at: chrono::Utc::now().to_rfc3339(),
        objects,
    };
    let temporary = dir.join(format!("{MANIFEST}.part"));
    tokio::fs::write(&temporary, serde_json::to_vec_pretty(&manifest)?).await?;
    tokio::fs::rename(&temporary, dir.join(MANIFEST)).await?;
    Ok(())
}

async fn digest(path: &Path) -> Result<(u64, String)> {
    let mut file = tokio::fs::File::open(path).await?;
    let (mut hasher, mut size, mut buffer) = (Sha256::new(), 0u64, vec![0u8; 1024 * 1024]);
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok((size, hex::encode(hasher.finalize())));
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
}

/// Copies every stored object into `dir`. Objects already present with the
/// same key and size are kept, which makes repeated runs incremental.
pub async fn export(store: Arc<dyn ObjectStore>, dir: &Path) -> Result<Summary> {
    tokio::fs::create_dir_all(dir.join(OBJECTS)).await?;
    let previous: HashMap<String, Entry> = read_manifest(dir)
        .await?
        .map(|m| m.objects.into_iter().map(|e| (e.key.clone(), e)).collect())
        .unwrap_or_default();
    let mut listing = store.list(None);
    let mut metas = Vec::new();
    while let Some(meta) = listing.next().await {
        let meta = meta.context("listing storage")?;
        if !meta.location.as_ref().starts_with(PROBE_PREFIX) {
            metas.push(meta);
        }
    }
    metas.sort_by(|a, b| a.location.cmp(&b.location));
    let (mut summary, mut entries) = (Summary::default(), Vec::with_capacity(metas.len()));
    for meta in metas {
        let key = meta.location.to_string();
        let target = object_file(dir, &key);
        let size = meta.size;
        if let Some(entry) = previous.get(&key)
            && entry.size == size
            && tokio::fs::metadata(&target)
                .await
                .is_ok_and(|m| m.len() == size)
        {
            entries.push(entry.clone());
            summary.skipped += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let temporary = target.with_extension("part");
        let mut file = tokio::fs::File::create(&temporary).await?;
        let mut stream = store
            .get(&meta.location)
            .await
            .with_context(|| format!("reading {key}"))?
            .into_stream();
        let (mut hasher, mut written) = (Sha256::new(), 0u64);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("reading {key}"))?;
            hasher.update(&chunk);
            written += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        ensure!(
            written == size,
            "{key} changed while it was copied ({written} of {size} bytes); run the export again"
        );
        tokio::fs::rename(&temporary, &target).await?;
        entries.push(Entry {
            key,
            size,
            sha256: hex::encode(hasher.finalize()),
        });
        summary.copied += 1;
        summary.bytes += size;
    }
    write_manifest(dir, entries).await?;
    Ok(summary)
}

/// Restores a backup into `store`. Every file is verified against the manifest
/// before the first upload, so a damaged backup changes nothing. Objects that already exist with the same size are
/// left untouched.
pub async fn import(store: Arc<dyn ObjectStore>, dir: &Path) -> Result<Summary> {
    verify(dir).await?;
    let manifest = read_manifest(dir).await?.context("manifest disappeared")?;
    let mut summary = Summary::default();
    for entry in &manifest.objects {
        let location = StorePath::parse(&entry.key)?;
        let source = object_file(dir, location.as_ref());
        let size = entry.size;
        if store
            .head(&location)
            .await
            .is_ok_and(|existing| existing.size == size)
        {
            summary.skipped += 1;
            continue;
        }
        let mut file = tokio::fs::File::open(&source).await?;
        let mut upload =
            WriteMultipart::new_with_chunk_size(store.put_multipart(&location).await?, CHUNK);
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            upload.write(&buffer[..read]);
        }
        upload.finish().await?;
        summary.copied += 1;
        summary.bytes += size;
    }
    Ok(summary)
}

/// Checks a backup directory against its manifest without touching storage.
pub async fn verify(dir: &Path) -> Result<usize> {
    let manifest = read_manifest(dir)
        .await?
        .with_context(|| format!("no {MANIFEST} in {}", dir.display()))?;
    for entry in &manifest.objects {
        let location = StorePath::parse(&entry.key)
            .with_context(|| format!("unsafe object key '{}'", entry.key))?;
        let (size, sha256) = digest(&object_file(dir, location.as_ref()))
            .await
            .with_context(|| format!("reading backup file for {}", entry.key))?;
        if size != entry.size || sha256 != entry.sha256 {
            bail!(
                "{} does not match the manifest; the backup is damaged",
                entry.key
            );
        }
    }
    Ok(manifest.objects.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::{PutPayload, memory::InMemory};

    async fn seeded() -> Arc<dyn ObjectStore> {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        for (key, body) in [
            ("packages/a/acme.core.1.0.0.nupkg", vec![1u8; 10]),
            ("feeds/f1/packages/b.nupkg", vec![2u8; 3 * 1024 * 1024]),
            (".nushelf-readiness-probe/x", vec![9u8; 2]),
        ] {
            store
                .put(&StorePath::from(key), PutPayload::from(body))
                .await
                .unwrap();
        }
        store
    }
    async fn keys(store: &Arc<dyn ObjectStore>) -> Vec<String> {
        let mut found: Vec<String> = store
            .list(None)
            .map(|m| m.unwrap().location.to_string())
            .collect()
            .await;
        found.sort();
        found
    }

    #[tokio::test]
    async fn round_trip_is_incremental_and_skips_the_probe_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let source = seeded().await;
        let first = export(source.clone(), dir.path()).await.unwrap();
        assert_eq!((first.copied, first.skipped), (2, 0));
        assert_eq!(verify(dir.path()).await.unwrap(), 2);
        let again = export(source.clone(), dir.path()).await.unwrap();
        assert_eq!((again.copied, again.skipped), (0, 2));

        let target: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let restored = import(target.clone(), dir.path()).await.unwrap();
        assert_eq!((restored.copied, restored.skipped), (2, 0));
        assert_eq!(
            keys(&target).await,
            [
                "feeds/f1/packages/b.nupkg",
                "packages/a/acme.core.1.0.0.nupkg"
            ]
        );
        let body = target
            .get(&StorePath::from("feeds/f1/packages/b.nupkg"))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(body.len(), 3 * 1024 * 1024);
        assert_eq!(import(target, dir.path()).await.unwrap().skipped, 2);
    }

    #[tokio::test]
    async fn damaged_backup_is_rejected_before_anything_is_uploaded() {
        let dir = tempfile::tempdir().unwrap();
        export(seeded().await, dir.path()).await.unwrap();
        tokio::fs::write(
            object_file(dir.path(), "packages/a/acme.core.1.0.0.nupkg"),
            vec![7u8; 10],
        )
        .await
        .unwrap();
        assert!(verify(dir.path()).await.is_err());
        let target: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let error = import(target.clone(), dir.path()).await.unwrap_err();
        assert!(error.to_string().contains("damaged"), "{error}");
        assert!(keys(&target).await.is_empty());
    }

    #[tokio::test]
    async fn manifest_keys_cannot_escape_the_backup_directory() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            format: FORMAT,
            created_at: String::new(),
            objects: vec![Entry {
                key: "../outside".into(),
                size: 0,
                sha256: String::new(),
            }],
        };
        tokio::fs::write(
            dir.path().join(MANIFEST),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();
        let target: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        assert!(import(target, dir.path()).await.is_err());
        assert!(verify(dir.path()).await.is_err());
    }
}
