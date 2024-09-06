use std::str::FromStr;

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};

/// State for the authentication
pub struct AuthState {
    /// Header for the JWT
    pub header: Header,
    /// Validation for the JWT
    pub validation: Validation,
    /// Encoding key for the JWT
    pub encoding_key: EncodingKey,
    /// Decoding key for the JWT
    pub decoding_key: DecodingKey,
}

impl AuthState {
    /// Create a new [AuthState] from a secret
    pub fn from_secret(secret: &[u8]) -> Self {
        let header = Header::default();
        let validation = Validation::default();

        let encoding_key = EncodingKey::from_secret(secret);
        let decoding_key = DecodingKey::from_secret(secret);

        Self {
            header,
            validation,
            decoding_key,
            encoding_key,
        }
    }
}

impl FromStr for AuthState {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let secret_bytes = hex::decode(s).unwrap();
        Ok(Self::from_secret(&secret_bytes))
        // Alternative attempt that does not work:
        // Ok(Self::from_secret(s.as_bytes()))
    }
}
