mod remus;
mod rhea;
mod romulus;
mod tx_tuple;

pub use remus::*;
pub use rhea::*;
use rome_evm_client::error::RomeEvmError;
pub use tx_tuple::*;

/// Rome Tx
pub type RomeTx<'a> = Box<dyn rome_solana::batch::AdvanceTx<'a, Error = RomeEvmError>>;
