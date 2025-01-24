use serde::{Deserialize, Serialize};
use solana_sdk::commitment_config::{CommitmentConfig, CommitmentLevel};

/// The commitment level of data in solana.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::Parser)]
pub enum Commitment {
    /// The data is confirmed by the leader node.
    Confirmed,
    /// The data is confirmed by the majority of the nodes.
    Finalized,
    /// The data is confirmed by all the nodes.
    Processed,
}

impl TryFrom<CommitmentLevel> for Commitment {
    type Error = anyhow::Error;

    fn try_from(value: CommitmentLevel) -> Result<Self, Self::Error> {
        match value {
            CommitmentLevel::Confirmed => Ok(Self::Confirmed),
            CommitmentLevel::Finalized => Ok(Self::Finalized),
            CommitmentLevel::Processed => Ok(Self::Processed),
        }
    }
}

impl TryFrom<CommitmentConfig> for Commitment {
    type Error = anyhow::Error;

    fn try_from(value: CommitmentConfig) -> Result<Self, Self::Error> {
        Commitment::try_from(value.commitment)
    }
}
