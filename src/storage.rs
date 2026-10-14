use crate::config::Config;
use anyhow::{Result, bail};
use object_store::{
    ObjectStore,
    aws::{AmazonS3Builder, AmazonS3ConfigKey},
    azure::{AzureConfigKey, MicrosoftAzureBuilder},
    gcp::{GoogleCloudStorageBuilder, GoogleConfigKey},
    local::LocalFileSystem,
};
use std::sync::Arc;

pub fn build(config: &Config) -> Result<Arc<dyn ObjectStore>> {
    Ok(match config.storage_provider.as_str() {
        "local" => {
            std::fs::create_dir_all(&config.storage_path)?;
            Arc::new(LocalFileSystem::new_with_prefix(&config.storage_path)?)
        }
        // Compose supplies empty optional variables. Ignore them so provider defaults
        // and AWS workload credentials continue to work without invalid empty URLs.
        "s3" => {
            let mut builder = AmazonS3Builder::new();
            for (key, value) in std::env::vars().filter(|(_, v)| !v.is_empty()) {
                if let Ok(key) = key.to_ascii_lowercase().parse::<AmazonS3ConfigKey>() {
                    builder = builder.with_config(key, value);
                }
            }
            Arc::new(builder.build()?)
        }
        "azure" => {
            let mut builder = MicrosoftAzureBuilder::new();
            for (key, value) in std::env::vars().filter(|(_, v)| !v.is_empty()) {
                if let Ok(key) = key.to_ascii_lowercase().parse::<AzureConfigKey>() {
                    builder = builder.with_config(key, value);
                }
            }
            Arc::new(builder.build()?)
        }
        "gcs" => {
            let mut builder = GoogleCloudStorageBuilder::new();
            for (key, value) in std::env::vars().filter(|(_, v)| !v.is_empty()) {
                if let Ok(key) = key.to_ascii_lowercase().parse::<GoogleConfigKey>() {
                    builder = builder.with_config(key, value);
                }
            }
            Arc::new(builder.build()?)
        }
        other => bail!("Unknown STORAGE_PROVIDER '{other}'; use local, s3, azure or gcs"),
    })
}
