use std::borrow::Cow;
use std::ops::Deref;

use super::EthSignedTxTuple;

/// Multiple evm transactions over multiple rollups
/// executed atomically.
pub struct RemusTx<'a>(Cow<'a, [EthSignedTxTuple]>);

impl Deref for RemusTx<'_> {
    type Target = [EthSignedTxTuple];

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl<'a> RemusTx<'a> {
    /// Creates a new RheaTx from a [EthSignedTxTuple].
    pub fn new(tx: Vec<EthSignedTxTuple>) -> Self {
        Self(Cow::Owned(tx))
    }

    /// Creates a new RemusTx from a reference to a [EthSignedTxTuple].
    pub fn from_ref(tx: &'a [EthSignedTxTuple]) -> Self {
        Self(Cow::Borrowed(tx))
    }
}
