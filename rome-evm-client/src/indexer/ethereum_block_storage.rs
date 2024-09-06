use {
    crate::{error::Result, indexer::transaction_storage::TransactionStorage},
    ethers::{
        abi::ethereum_types::BigEndianHash,
        types::{
            Address, Block, Bloom, Bytes, OtherFields, Transaction, TxHash, H256, H64, U256, U64,
        },
    },
    jsonrpsee_core::Serialize,
    std::{
        collections::BTreeMap,
        sync::{Arc, RwLock},
    },
};

const SHA3_UNCLES: [u8; 32] = [
    0x1d, 0xcc, 0x4d, 0xe8, 0xde, 0xc7, 0x5d, 0x7a, 0xab, 0x85, 0xb5, 0x67, 0xb6, 0xcc, 0xd4, 0x1a,
    0xd3, 0x12, 0x45, 0x1b, 0x94, 0x8a, 0x74, 0x13, 0xf0, 0xa1, 0x42, 0xfd, 0x40, 0xd4, 0x93, 0x47,
];

const EMPTY_ROOT: [u8; 32] = [
    0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6, 0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
    0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0, 0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
];

#[derive(Clone, Serialize)]
#[serde(untagged)]
pub enum BlockType {
    BlockWithTransactions(Block<Transaction>),
    BlockWithHashes(Block<TxHash>),
}

pub struct BlockData {
    hash: H256,
    parent_hash: H256,
    pub number: U64,
    pub timestamp: U256,
    gas_used: U256,
    transactions: Vec<TxHash>,
}

impl BlockData {
    pub fn new(
        hash: H256,
        parent_hash: H256,
        number: U64,
        timestamp: U256,
        gas_used: U256,
        transactions: Vec<TxHash>,
    ) -> Self {
        Self {
            hash,
            parent_hash,
            number,
            timestamp,
            gas_used,
            transactions: transactions.clone(),
        }
    }

    fn get_block_base<T>(&self) -> Block<T> {
        let tx_root = if self.transactions.is_empty() {
            H256::from(EMPTY_ROOT)
        } else {
            H256::from_uint(&U256::one())
        };

        Block::<T> {
            hash: Some(self.hash),
            parent_hash: self.parent_hash,
            number: Some(self.number),
            gas_limit: U256::from(48000000000000u64),
            uncles_hash: H256::from(SHA3_UNCLES),
            author: Some(Address::default()),
            state_root: H256::zero(),
            transactions_root: tx_root,
            receipts_root: tx_root,
            gas_used: self.gas_used,
            extra_data: Bytes::default(),
            logs_bloom: Some(Bloom::default()),
            timestamp: self.timestamp,
            difficulty: U256::zero(),
            total_difficulty: Some(U256::zero()),
            seal_fields: vec![],
            uncles: vec![],
            transactions: vec![],
            size: Some(U256::one()),
            mix_hash: Some(H256::default()),
            nonce: Some(H64::zero()),
            base_fee_per_gas: Some(U256::zero()),
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            withdrawals: None,
            withdrawals_root: None,
            other: OtherFields::default(),
        }
    }

    pub fn get_block(
        &self,
        full_transactions: bool,
        transaction_storage: &TransactionStorage,
    ) -> BlockType {
        if full_transactions {
            BlockType::BlockWithTransactions(Block::<Transaction> {
                transactions: self
                    .transactions
                    .iter()
                    .filter_map(|tx_hash| {
                        transaction_storage
                            .get_transaction(tx_hash)
                            .map_or(None, |tx| tx.get_transaction().map(|tx| tx.clone()))
                    })
                    .collect(),
                ..self.get_block_base()
            })
        } else {
            BlockType::BlockWithHashes(Block::<TxHash> {
                transactions: self
                    .transactions
                    .iter()
                    .filter_map(|tx_hash| {
                        transaction_storage
                            .get_transaction(tx_hash)
                            .map_or(None, |tx| tx.get_transaction().map(|_| tx_hash.clone()))
                    })
                    .collect(),
                ..self.get_block_base()
            })
        }
    }

    pub fn get_transactions(&self, transaction_storage: &TransactionStorage) -> Vec<Transaction> {
        self.transactions
            .iter()
            .filter_map(|tx_hash| {
                transaction_storage
                    .get_transaction(tx_hash)
                    .map_or(None, |tx| tx.get_transaction().map(|tx| tx.clone()))
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct EthereumBlockStorage {
    blocks_by_number: Arc<RwLock<BTreeMap<U64, Arc<BlockData>>>>,
    blocks_by_hash: Arc<RwLock<BTreeMap<H256, Arc<BlockData>>>>,
}

impl EthereumBlockStorage {
    pub fn new() -> Self {
        Self {
            blocks_by_number: Arc::new(RwLock::new(BTreeMap::new())),
            blocks_by_hash: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub fn get_block_by_number(&self, block_number: U64) -> Result<Option<Arc<BlockData>>> {
        let lock = self.blocks_by_number.read()?;
        Ok(lock.get(&block_number).map(|e| e.clone()))
    }

    pub fn set_block(&self, block_data: Arc<BlockData>) -> Result<()> {
        let mut lock1 = self.blocks_by_number.write()?;
        let mut lock2 = self.blocks_by_hash.write()?;
        lock1.insert(block_data.number, block_data.clone());
        lock2.insert(block_data.hash, block_data.clone());
        Ok(())
    }

    pub fn latest_block(&self) -> Result<Option<U64>> {
        let lock = self.blocks_by_number.read()?;
        if let Some((block_number, _)) = lock.last_key_value() {
            return Ok(Some(*block_number));
        }

        return Ok(None);
    }
}
