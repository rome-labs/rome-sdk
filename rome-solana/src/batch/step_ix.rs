use anyhow::Context;
use solana_sdk::signature::{Keypair, Signature};

use crate::tower::SolanaTower;

use super::AtomicIxBatch;

/// A set of [IxBatch] executed in steps
pub struct StepIxBatch<'a> {
    /// list of [IxBatch]es executed parallely
    /// out of order
    pub prep: Option<Vec<AtomicIxBatch<'a>>>,
    /// list of [IxBatch]es executed sequentially
    pub exec: Option<Vec<AtomicIxBatch<'a>>>,
    /// Final [IxBatch] executed atomically
    pub submit: AtomicIxBatch<'a>,
}

impl<'a> StepIxBatch<'a> {
    /// Create a new [StepIxBatch] with a single [IxBatch]
    pub fn new(submit: AtomicIxBatch<'a>) -> Self {
        Self {
            prep: None,
            exec: None,
            submit,
        }
    }

    /// Create a new [StepIxBatch] with a list of [IxBatch]es
    pub fn new_with_prep(submit: AtomicIxBatch<'a>, prep: Vec<AtomicIxBatch<'a>>) -> Self {
        Self {
            prep: Some(prep),
            exec: None,
            submit,
        }
    }

    /// Create a new [StepIxBatch] with a list of [IxBatch]es
    pub fn new_with_exec(submit: AtomicIxBatch<'a>, exec: Vec<AtomicIxBatch<'a>>) -> Self {
        Self {
            prep: None,
            exec: Some(exec),
            submit,
        }
    }

    /// Create a new [StepIxBatch] with a list of [IxBatch]es
    pub fn new_with_prep_and_exec(
        submit: AtomicIxBatch<'a>,
        prep: Vec<AtomicIxBatch<'a>>,
        exec: Vec<AtomicIxBatch<'a>>,
    ) -> Self {
        Self {
            prep: Some(prep),
            exec: Some(exec),
            submit,
        }
    }

    /// Send the instructions in batches
    pub async fn send_and_confirm(
        &self,
        solana_tower: &SolanaTower,
        payer: &Keypair,
    ) -> anyhow::Result<Signature> {
        // send prep parallel
        if let Some(prep) = &self.prep {
            solana_tower
                .send_and_confirm_parallel(prep, payer)
                .await
                .context("Error sending prep transactions")?;
        }

        // send exec sequential
        if let Some(exec) = &self.exec {
            solana_tower
                .send_and_confirm_sequential(exec, payer)
                .await
                .context("Error sending exec transactions")?;
        }

        // send submit tx and return signature
        solana_tower.send_and_confirm(&self.submit, payer).await
    }
}
