use crate::indexer::TxResult;
use ethers::addressbook::Address;
use ethers::prelude::transaction::eip2718::TypedTransaction;
use ethers::prelude::transaction::optimism::DepositTransaction;
use ethers::prelude::{Bytes, NameOrAddress, Transaction, TransactionRequest, H256, U256};
use ethers::types::U64;
use ethers::utils::keccak256;
use rlp::{Decodable, Rlp};
use solana_program::clock::Slot;

const L1_ATTRIBUTES_DEPOSITOR: [u8; 20] = [
    0xde, 0xad, 0xde, 0xad, 0xde, 0xad, 0xde, 0xad, 0xde, 0xad, 0xde, 0xad, 0xde, 0xad, 0xde, 0xad,
    0xde, 0xad, 0x00, 0x01,
];

const L1_ATTRIBUTES_SC: [u8; 20] = [
    0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x15,
];

const SET_L1_ATTRIBUTES_HASH: [u8; 4] = [0x01, 0x5d, 0x8e, 0xb9];

const L1_ATTRIBUTES_GAS_LIMIT: u64 = 150_000_000;

fn create_l1_attributes_tx_data(
    l1_block_number: Slot,
    l1_blockhash: &H256,
    l1_timestamp: &U256,
) -> Bytes {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 32];

    bytes.extend_from_slice(&SET_L1_ATTRIBUTES_HASH);

    U256::from(l1_block_number).to_big_endian(&mut buf);
    bytes.extend_from_slice(&buf);

    l1_timestamp.to_big_endian(&mut buf);
    bytes.extend_from_slice(&buf);

    U256::zero().to_big_endian(&mut buf); // base_fee
    bytes.extend_from_slice(&buf);

    bytes.extend_from_slice(l1_blockhash.as_bytes());

    U256::from(l1_block_number).to_big_endian(&mut buf); // sequence_number
    bytes.extend_from_slice(&buf);

    bytes.extend_from_slice(H256::zero().as_bytes()); // batcher_hash

    U256::zero().to_big_endian(&mut buf); // fee_overhead
    bytes.extend_from_slice(&buf);

    U256::zero().to_big_endian(&mut buf); // fee_scalar
    bytes.extend_from_slice(&buf);

    Bytes::from(bytes)
}

fn create_l1_attributes_source_hash(l1_block_number: Slot, l1_blockhash: &H256) -> H256 {
    let mut buf = [0u8; 32];
    U256::from(l1_block_number).to_big_endian(&mut buf);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(l1_blockhash.as_bytes());
    bytes.extend_from_slice(&buf);

    let part2 = keccak256(&bytes);
    U256::one().to_big_endian(&mut buf);

    bytes.clear();
    bytes.extend_from_slice(&buf);
    bytes.extend_from_slice(&part2);

    H256::from_slice(&keccak256(&bytes))
}

pub fn create_user_deposit_source_hash(l1_blockhash: &H256, l1_log_index: U256) -> H256 {
    let mut buf = [0u8; 32];
    l1_log_index.to_big_endian(&mut buf);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(l1_blockhash.as_bytes());
    bytes.extend_from_slice(&buf);

    let part2 = keccak256(&bytes);
    U256::zero().to_big_endian(&mut buf);

    bytes.clear();
    bytes.extend_from_slice(&buf);
    bytes.extend_from_slice(&part2);

    H256::from_slice(&keccak256(&bytes))
}

pub fn create_l1_attributes_tx(
    l1_block_number: Slot,
    l1_blockhash: &H256,
    l1_timestamp: &U256,
) -> (Transaction, TxResult) {
    let tx = TypedTransaction::DepositTransaction(DepositTransaction {
        tx: TransactionRequest {
            from: Some(Address::from(L1_ATTRIBUTES_DEPOSITOR)),
            to: Some(NameOrAddress::Address(Address::from(L1_ATTRIBUTES_SC))),
            gas: Some(U256::from(L1_ATTRIBUTES_GAS_LIMIT)),
            gas_price: None,
            value: None,
            data: Some(create_l1_attributes_tx_data(
                l1_block_number,
                l1_blockhash,
                l1_timestamp,
            )),
            nonce: None,
            chain_id: None,
        },
        source_hash: create_l1_attributes_source_hash(l1_block_number, l1_blockhash),
        mint: None,
        is_system_tx: true,
    });

    (
        Transaction::decode(&Rlp::new(tx.rlp().as_ref())).unwrap(),
        TxResult::default(),
    )
}

pub fn update_if_user_deposited_tx(tx: &mut Transaction, l1_blockhash: &H256, l1_log_index: U256) {
    if tx.transaction_type != Some(U64::from(0x7E)) || tx.is_system_tx {
        return;
    }

    tx.source_hash = create_user_deposit_source_hash(l1_blockhash, l1_log_index);
    tx.hash = tx.hash();
}
