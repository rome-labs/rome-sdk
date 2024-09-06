#![warn(missing_docs)]
#![doc = include_str!("../../README.md")]

/// Rome evm client interface.
pub use rome_evm_client;
/// Interface to Geth functionalities.
pub use rome_geth;
/// Interface to Solana functionalities
pub use rome_solana;
/// General utilities for the library.
pub use rome_utils;

mod config;
mod rome;
mod tx;

pub use config::*;
pub use rome::*;
pub use tx::*;
