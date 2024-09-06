// use anyhow::bail;

use rome_utils::auth::AuthState;

/// Claims for the JWT token
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct EngineClaim {
    /// Issued at timestamp
    iat: i64,
}

impl EngineClaim {
    /// Construct a new claim
    pub fn new() -> Self {
        Self {
            iat: chrono::Utc::now().timestamp(),
        }
    }

    /// Convert [EngineClaim] to token
    pub fn to_token(&self, auth_state: &AuthState) -> anyhow::Result<String> {
        jsonwebtoken::encode(&auth_state.header, self, &auth_state.encoding_key)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

impl Default for EngineClaim {
    fn default() -> Self {
        Self::new()
    }
}
