use std::path::Path;

use anyhow::Context;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub trait ReadableConfig: serde::de::DeserializeOwned {
    /// Read the config file from the given path
    fn read(path: &Path) -> impl std::future::Future<Output = anyhow::Result<Self>> + Send
    where
        Self: Sized;
}

impl<T> ReadableConfig for T
where
    T: serde::de::DeserializeOwned,
{
    /// Read the config file from the given path
    async fn read(path: &Path) -> anyhow::Result<T> {
        // Create a buffer
        let mut buf = Vec::new();

        // Read the file as a serde value
        File::open(path)
            .await
            .context("Failed to open config file")?
            .read_to_end(&mut buf)
            .await
            .context("Failed to read config file")?;

        // Get the extension of the file
        let ext = path.extension().map(|ext| ext.to_str().unwrap_or_default());

        // Parse according to the extension
        match ext {
            Some("yml") | Some("yaml") => {
                let value =
                    serde_yaml::from_slice(&buf).context("Failed to parse config file as YAML")?;

                serde_yaml::from_value(value).context("Error parsing config file")
            }
            _ => {
                let value =
                    serde_json::from_slice(&buf).context("Failed to parse config file as JSON")?;

                serde_json::from_value(value).context("Error parsing config file")
            }
        }
    }
}
