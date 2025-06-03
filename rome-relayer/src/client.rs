use std::ops::{Deref, DerefMut};

use super::relayer_api_client::RelayerApiClient;

/// A simplified abstraction around [RelayerApiClient]
#[derive(Debug)]
pub struct RelayerClient {
    inner: RelayerApiClient<tonic::transport::Channel>,
}

impl Deref for RelayerClient {
    type Target = RelayerApiClient<tonic::transport::Channel>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for RelayerClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl RelayerClient {
    /// Create a new instance of [RelayerClient]
    pub async fn init(url: impl ToString) -> Result<Self, tonic::transport::Error> {
        RelayerApiClient::connect(url.to_string())
            .await
            .map(|inner| Self { inner })
    }
}
