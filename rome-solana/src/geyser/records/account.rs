use std::borrow::Cow;

use super::GeyserKafkaRecord;

/// Record for an account update.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
pub struct AccountRecord<'a> {
    /// The Pubkey for the account
    pub pubkey: Cow<'a, [u8]>,

    /// The lamports for the account
    pub lamports: u64,

    /// The Pubkey of the owner program account
    pub owner: Cow<'a, [u8]>,

    /// This account's data contains a loaded program (and is now read-only)
    pub executable: bool,

    /// The epoch at which this account will next owe rent
    pub rent_epoch: u64,

    /// The data held in this account.
    pub data: Cow<'a, [u8]>,

    /// A global monotonically increasing atomic number, which can be used
    /// to tell the order of the account update. For example, when an
    /// account is updated in the same slot multiple times, the update
    /// with higher write_version should supersede the one with lower
    /// write_version.
    pub write_version: u64,
}

impl GeyserKafkaRecord for AccountRecord<'_> {
    fn topic(&self) -> &'static str {
        "account"
    }

    fn key(&self) -> String {
        format!("{:?}", self.pubkey)
    }
}
