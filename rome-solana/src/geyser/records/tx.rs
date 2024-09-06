use std::borrow::Cow;

// use solana_geyser_plugin_interface::geyser_plugin_interface::{
//     ReplicaTransactionInfo, ReplicaTransactionInfoVersions,
// };
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status::UiTransactionStatusMeta;

use super::GeyserKafkaRecord;

/// Record for an instruction update.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
pub struct TxRecord<'a> {
    /// The first signature of the transaction, used for identifying the transaction.
    pub signature: Cow<'a, Signature>,

    /// Indicates if the transaction is a simple vote transaction.
    pub is_vote: bool,

    /// The sanitized transaction.
    pub transaction: Cow<'a, VersionedTransaction>,

    /// Metadata of the transaction status.
    pub transaction_status_meta: Cow<'a, UiTransactionStatusMeta>,
}

impl GeyserKafkaRecord for TxRecord<'_> {
    fn topic(&self) -> &'static str {
        "tx"
    }

    fn key(&self) -> String {
        format!("{:?}", self.signature)
    }
}

// impl<'a> From<ReplicaTransactionInfoVersions<'a>> for TxRecord<'a> {
//     fn from(value: ReplicaTransactionInfoVersions<'a>) -> Self {
//         let value = match value {
//             ReplicaTransactionInfoVersions::V0_0_1(info) => info,
//             ReplicaTransactionInfoVersions::V0_0_2(info) => &ReplicaTransactionInfo {
//                 signature: info.signature,
//                 is_vote: info.is_vote,
//                 transaction: info.transaction,
//                 transaction_status_meta: info.transaction_status_meta,
//                 transaction_status_meta: Cow::Owned(UiTransactionStatusMeta::from(
//                     value.transaction_status_meta.to_owned(),
//                 )),
//             },
//         };

//         Self {
//             signature: Cow::Borrowed(value.signature),
//             is_vote: value.is_vote,
//             transaction: Cow::Owned(value.transaction.to_versioned_transaction()),
//             transaction_status_meta: Cow::Owned(UiTransactionStatusMeta::from(
//                 value.transaction_status_meta.to_owned(),
//             )),
//         }
//     }
// }
