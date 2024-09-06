use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Bytes, Signature, Transaction};

/// A tuple of a transaction and its signature.
#[derive(Clone, Debug)]
pub struct EthSignedTxTuple(TypedTransaction, Signature);

impl EthSignedTxTuple {
    /// Creates a new EthTxTuple from a TypedTransaction and a Signature.
    pub fn new(tx: TypedTransaction, sig: Signature) -> Self {
        Self(tx, sig)
    }

    /// Returns a reference to the transaction.
    pub fn tx(&self) -> &TypedTransaction {
        &self.0
    }

    /// Returns a reference to the signature.
    pub fn sig(&self) -> &Signature {
        &self.1
    }

    /// Takes the transaction and signature out of the tuple.
    pub fn into_parts(self) -> (TypedTransaction, Signature) {
        (self.0, self.1)
    }

    /// Into signed bytes
    pub fn signed_rlp_bytes(&self) -> Bytes {
        self.0.rlp_signed(&self.1)
    }
}

impl<'a> From<&'a EthSignedTxTuple> for Bytes {
    fn from(tuple: &'a EthSignedTxTuple) -> Self {
        tuple.signed_rlp_bytes()
    }
}

impl From<&Transaction> for EthSignedTxTuple {
    fn from(tx: &Transaction) -> Self {
        // calculate the signature of the transaction
        let sig = Signature {
            r: tx.r,
            s: tx.s,
            v: tx.v.as_u64(),
        };
        // convert the transaction into a typed transaction
        let tx = tx.into();

        // return EthTxTuple
        Self(tx, sig)
    }
}
