use std::str::FromStr;

use anyhow::bail;

use super::{JsonRpcRequest, JsonRpcResponse};

/// JSON-RPC client
#[derive(Debug)]
pub struct JsonRpcClient {
    /// Reqwest client
    client: reqwest::Client,
    /// JSON-RPC server URL
    pub url: url::Url,
}

impl FromStr for JsonRpcClient {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(reqwest::Url::from_str(s)?))
    }
}

impl From<url::Url> for JsonRpcClient {
    fn from(url: url::Url) -> Self {
        Self::new(url)
    }
}

impl JsonRpcClient {
    /// Create a new JsonRpcClient with a given url and bearer token
    pub fn new_with_auth(url: url::Url, token: String) -> Self {
        let client = reqwest::Client::builder()
            .default_headers(
                std::iter::once((
                    reqwest::header::AUTHORIZATION,
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
                ))
                .collect(),
            )
            .build()
            .unwrap();

        Self { client, url }
    }

    /// Create a new JsonRpcClient with a given url
    pub fn new(url: reqwest::Url) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
        }
    }

    /// Send a JSON-RPC request and return the result
    pub async fn send_req<R: serde::de::DeserializeOwned>(
        &self,
        req: JsonRpcRequest<'_>,
        headers: Option<reqwest::header::HeaderMap>,
    ) -> anyhow::Result<R> {
        let res = self
            .client
            .post(self.url.as_ref())
            .json(&req)
            .headers(headers.unwrap_or_default())
            .send()
            .await?
            .error_for_status()?;

        let res: JsonRpcResponse<R> = res.json().await?;

        if let Some(err) = res.error {
            bail!("Error in response: {:?}", err);
        }

        let Some(result) = res.result else {
            bail!("No result in response");
        };

        Ok(result)
    }
}
