#![doc = include_str!("../README.md")]

/// An abstracted set of functions for interacting with the rome-sdk.
pub mod abstracted;
/// Interface to interact with the engine `API` of `geth`.
pub mod engine;
/// Various indexing strategies that can be used to bundle data from geth node.
pub mod indexers;
/// Types related to `geth`.
pub mod types;
