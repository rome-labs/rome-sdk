mod ethereum_block_storage;
pub mod inmemory;
pub mod parsers;
pub mod pg_storage;
pub mod rollup_indexer;
mod solana_block_loader;
mod solana_block_storage;
mod standalone_indexer;

pub use crate::error::ProgramResult;
pub use ethereum_block_storage::{
    BlockParams, BlockProducer, BlockType, EthereumBlockStorage, PendingBlock, PendingBlocks,
    ProducedBlocks,
};
pub use parsers::block_parser::{BlockParseResult, BlockParser, TxResult};
pub use rollup_indexer::RollupIndexer;
pub use solana_block_loader::SolanaBlockLoader;
pub use solana_block_storage::SolanaBlockStorage;
pub use standalone_indexer::StandaloneIndexer;

#[cfg(test)]
pub mod test {
    use crate::indexer::parsers::block_parser::{GasReport, TxResult};
    use crate::indexer::parsers::log_parser::ExitReason;
    use ethers::core::k256::ecdsa::SigningKey;
    use ethers::prelude::transaction::eip2718::TypedTransaction;
    use ethers::prelude::OtherFields;
    use ethers::signers::{Signer, Wallet};
    use ethers::types::{
        Address, Bytes, NameOrAddress, Transaction, TransactionRequest, U256, U64,
    };
    use rand::{random, thread_rng};

    pub fn create_wallet() -> Wallet<SigningKey> {
        Wallet::new(&mut thread_rng())
    }

    pub fn create_result(gas_report: GasReport) -> TxResult {
        TxResult {
            exit_reason: ExitReason {
                code: 0,
                reason: "".to_string(),
                return_value: vec![],
            },
            logs: vec![],
            gas_report,
        }
    }

    pub async fn create_simple_tx(
        chain_id: Option<U64>,
        wallet: &Wallet<SigningKey>,
        gas_recipient: Option<Address>,
    ) -> (Transaction, TxResult) {
        let to = Address::random();
        let value = U256::from(100);
        let gas_price = U256::from(1);
        let gas = U256::from(22000);
        let nonce = U256::from(random::<u32>());

        let tx_request = TypedTransaction::Legacy(TransactionRequest {
            from: Some(wallet.address()),
            to: Some(NameOrAddress::Address(to)),
            gas: Some(gas),
            gas_price: Some(gas_price),
            value: Some(value),
            data: None,
            nonce: Some(nonce),
            chain_id,
        });

        let signature = wallet.sign_transaction(&tx_request).await.unwrap();

        (
            Transaction {
                hash: tx_request.hash(&signature),
                nonce: *tx_request.nonce().unwrap(),
                block_hash: None,
                block_number: None,
                transaction_index: None,
                from: wallet.address(),
                to: Some(to),
                value,
                gas_price: Some(gas_price),
                gas,
                input: Bytes::new(),
                v: U64::from(signature.v),
                r: signature.r,
                s: signature.s,
                transaction_type: None,
                access_list: None,
                max_priority_fee_per_gas: None,
                max_fee_per_gas: None,
                chain_id: Some(U256::from(chain_id.unwrap().as_u64())),
                other: OtherFields::default(),
            },
            create_result(GasReport {
                gas_value: gas,
                gas_recipient,
            }),
        )
    }

    pub async fn create_big_tx(
        chain_id: Option<U64>,
        wallet: &Wallet<SigningKey>,
        gas_recipient: Option<Address>,
    ) -> (Transaction, TxResult) {
        let value = U256::from(100);
        let gas_price = U256::from(1);
        let gas = U256::from(22000);
        let nonce = U256::from(random::<u32>());
        let data = Bytes::from(vec![0u8; 128]);

        let tx_request = TypedTransaction::Legacy(TransactionRequest {
            from: Some(wallet.address()),
            to: None,
            gas: Some(gas),
            gas_price: Some(gas_price),
            value: Some(value),
            data: Some(data.clone()),
            nonce: Some(nonce),
            chain_id,
        });

        let signature = wallet.sign_transaction(&tx_request).await.unwrap();

        (
            Transaction {
                hash: tx_request.hash(&signature),
                nonce: *tx_request.nonce().unwrap(),
                block_hash: None,
                block_number: None,
                transaction_index: None,
                from: wallet.address(),
                to: None,
                value,
                gas_price: Some(gas_price),
                gas,
                input: data,
                v: U64::from(signature.v),
                r: signature.r,
                s: signature.s,
                transaction_type: None,
                access_list: None,
                max_priority_fee_per_gas: None,
                max_fee_per_gas: None,
                chain_id: Some(U256::from(chain_id.unwrap().as_u64())),
                other: OtherFields::default(),
            },
            create_result(GasReport {
                gas_value: gas,
                gas_recipient,
            }),
        )
    }
}
