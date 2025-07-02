use std::borrow::Cow;
use std::ops::Deref;

use super::EthSignedTxTuple;

/// A single evm transaction.
#[derive(Clone, Debug)]
pub struct RheaTx<'a>(Cow<'a, EthSignedTxTuple>);

impl Deref for RheaTx<'_> {
    type Target = EthSignedTxTuple;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> RheaTx<'a> {
    /// Creates a new RheaTx from a EthSignedTxTuple.
    pub fn new(tx: EthSignedTxTuple) -> Self {
        Self(Cow::Owned(tx))
    }

    /// Creates a new RheaTx from a reference to a EthSignedTxTuple.
    pub fn from_ref(tx: &'a EthSignedTxTuple) -> Self {
        Self(Cow::Borrowed(tx))
    }
}
