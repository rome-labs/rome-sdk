use std::ops::Deref;
use std::path::PathBuf;

use solana_sdk::signature::Keypair;
use tokio::fs::File;

use anyhow::Context;
use tokio::io::AsyncReadExt;

/// A structure that represents the payer in the Solana network
pub struct SolanaKeyPayer {
    pub payer: Keypair,
}

impl SolanaKeyPayer {
    /// Create a new instance of [SolanaPayer]
    pub fn new(payer: Keypair) -> Self {
        Self { payer }
    }

    /// Read the keypair from a file
    pub async fn read_from_file(path: &PathBuf) -> anyhow::Result<Self> {
        // Create a buffered reader
        let mut buf = Vec::new();

        // Read the file into a buffer
        File::open(path)
            .await
            .context("Failed to open payer keypair file")?
            .read_to_end(&mut buf)
            .await
            .context("Failed to read payer keypair file")?;

        // parse the keypair
        let json: Vec<u8> = serde_json::from_slice(&buf).context("Failed to parse keypair")?;

        let keypair = Keypair::from_bytes(&json).context("Failed to parse keypair")?;

        Ok(Self::new(keypair))
    }

    /// Consumes the instance and returns the keypair
    pub fn into_keypair(self) -> Keypair {
        self.payer
    }
}

impl From<SolanaKeyPayer> for Keypair {
    fn from(val: SolanaKeyPayer) -> Self {
        val.payer
    }
}

impl Deref for SolanaKeyPayer {
    type Target = Keypair;

    fn deref(&self) -> &Self::Target {
        &self.payer
    }
}
