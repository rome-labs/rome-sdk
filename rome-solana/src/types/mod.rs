mod commitment;

use std::sync::Arc;

pub use commitment::*;

use solana_sdk::clock::Slot;
use solana_transaction_status::UiConfirmedBlock;

/// Async Atomic RPC client
pub type AsyncAtomicRpcClient = Arc<solana_client::nonblocking::rpc_client::RpcClient>;

/// Sync Atomic RPC client
pub type SyncAtomicRpcClient = Arc<solana_client::rpc_client::RpcClient>;

// **Slot Sender and Receiver**

/// Sender for slot.Block
pub type SlotSender = tokio::sync::mpsc::UnboundedSender<Slot>;
/// Receiver for slot.
pub type SlotReceiver = tokio::sync::mpsc::UnboundedReceiver<Slot>;

// **Block Sender and Receiver**

/// Sender for block.
pub type BlockSender = tokio::sync::mpsc::UnboundedSender<UiConfirmedBlock>;
/// Receiver for block.
pub type BlockReceiver = tokio::sync::mpsc::UnboundedReceiver<UiConfirmedBlock>;
