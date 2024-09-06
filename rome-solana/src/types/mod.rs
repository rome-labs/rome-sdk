mod commitment;

use std::sync::Arc;

pub use commitment::*;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::clock::Slot;
use solana_transaction_status::EncodedConfirmedBlock;

/// Atomic RPC client
pub type AtomicRpcClient = Arc<RpcClient>;

// **Slot Sender and Receiver**

/// Sender for slot.Block
pub type SlotSender = tokio::sync::mpsc::UnboundedSender<Slot>;
/// Receiver for slot.
pub type SlotReceiver = tokio::sync::mpsc::UnboundedReceiver<Slot>;

// **Block Sender and Receiver**

/// Sender for block.
pub type BlockSender = tokio::sync::mpsc::UnboundedSender<EncodedConfirmedBlock>;
/// Receiver for block.
pub type BlockReceiver = tokio::sync::mpsc::UnboundedReceiver<EncodedConfirmedBlock>;
