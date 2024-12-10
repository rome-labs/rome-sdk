use std::borrow::Cow;

use super::EthSignedTxTuple;
use solana_sdk::instruction::Instruction;

/// Multiple Ethereum transactions and Solana instructions over multiple rollups
/// executed atomically.
pub struct RomulusTx<'a> {
    eth_txs: Cow<'a, [EthSignedTxTuple]>,
    sol_ixs: Cow<'a, [Instruction]>,
}

impl<'a> RomulusTx<'a> {
    /// Creates a new RomulusTx from separate lists of Ethereum transactions and Solana instructions.
    pub fn new(eth_txs: Vec<EthSignedTxTuple>, sol_ixs: Vec<Instruction>) -> Self {
        Self {
            eth_txs: Cow::Owned(eth_txs),
            sol_ixs: Cow::Owned(sol_ixs),
        }
    }

    /// Creates a new RomulusTx from references to Ethereum transactions and Solana instructions.
    pub fn from_ref(eth_txs: &'a [EthSignedTxTuple], sol_ixs: &'a [Instruction]) -> Self {
        Self {
            eth_txs: Cow::Borrowed(eth_txs),
            sol_ixs: Cow::Borrowed(sol_ixs),
        }
    }

    /// Get a reference to the Ethereum transactions.
    pub fn eth_txs(&self) -> &[EthSignedTxTuple] {
        &self.eth_txs
    }

    /// Get a reference to the Solana instructions.
    pub fn sol_ixs(&self) -> &[Instruction] {
        &self.sol_ixs
    }
}
