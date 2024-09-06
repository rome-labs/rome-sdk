use {
    crate::{
        error::{
            Result,
            RomeEvmError::{InternalError, TypedTransactionError},
        },
        indexer::log_parser::LogParser,
    },
    ethers::types::{
        transaction::eip2718::TypedTransaction, Address, Bloom, Bytes, Log, NameOrAddress,
        OtherFields, Signature as EthSignature, Transaction, TransactionReceipt,
        TxHash, H256, U256, U64,
    },
    rlp::Rlp,
    solana_program::keccak::hash,
    solana_transaction_status::{option_serializer::OptionSerializer, UiTransactionStatusMeta},
    std::{collections::BTreeMap, ops::Add},
};

fn new_transaction(
    transaction_hash: TxHash,
    tx_request: &TypedTransaction,
    eth_signature: &EthSignature,
    block_hash: Option<H256>,
    block_number: Option<U64>,
    transaction_index: Option<U64>,
) -> Transaction {
    Transaction {
        hash: transaction_hash,
        nonce: *tx_request.nonce().unwrap_or(&U256::default()),
        block_hash,
        block_number,
        transaction_index,
        transaction_type: match tx_request {
            TypedTransaction::Legacy(_) => None,
            TypedTransaction::Eip2930(_) => Some(U64::from(1)),
            TypedTransaction::Eip1559(_) => Some(U64::from(2)),
        },
        from: *tx_request.from().unwrap_or(&Address::default()),
        gas_price: match tx_request {
            TypedTransaction::Eip1559(_) => None,
            _ => tx_request.gas_price().clone(),
        },
        gas: tx_request.gas().map_or(U256::default(), |v| *v),
        to: tx_request.to().map(|v| match v {
            NameOrAddress::Address(addr) => addr.clone(),
            NameOrAddress::Name(_) => Address::default(),
        }),
        value: *tx_request.value().unwrap_or(&U256::default()),
        input: tx_request.data().unwrap_or(&Bytes::default()).clone(),
        v: U64::from(eth_signature.v),
        r: eth_signature.r,
        s: eth_signature.s,
        access_list: tx_request.access_list().map(|al| al.clone()),
        max_priority_fee_per_gas: match tx_request {
            TypedTransaction::Legacy(_) => None,
            TypedTransaction::Eip2930(_) => None,
            TypedTransaction::Eip1559(tx) => tx.max_priority_fee_per_gas,
        },
        max_fee_per_gas: match tx_request {
            TypedTransaction::Legacy(_) => None,
            TypedTransaction::Eip2930(_) => None,
            TypedTransaction::Eip1559(tx) => tx.max_fee_per_gas,
        },
        chain_id: tx_request.chain_id().map(|ci| U256::from(ci.as_u64())),
        other: OtherFields::default(),
    }
}

pub fn calc_contract_address(
    to: Option<&NameOrAddress>,
    from: Option<&Address>,
    nonce: Option<&U256>,
) -> Result<Option<Address>> {
    if to.is_some() {
        return Ok(None);
    }

    from.map(|from| {
        let mut rlp = rlp::RlpStream::new_list(2);
        rlp.append(from);
        rlp.append(nonce.unwrap_or(&U256::zero()));
        let hash = hash(&rlp.out());
        Some(Address::from(H256::from(hash.to_bytes())))
    })
    .ok_or_else(|| {
        tracing::error!("from address is expected to be something");
        return InternalError;
    })
}

pub fn decode_transaction_from_rlp(rlp: &Rlp) -> Result<(TypedTransaction, EthSignature)> {
    TypedTransaction::decode_signed(rlp).map_err(|e| {
        tracing::info!("Unable to decode transaction: {e}");
        TypedTransactionError(e)
    })
}

pub type EVMStatusCode = u64;

// Helper enum accumulating data from Solana iterative and write holder
// instructions to build resulting Eth-like transaction
#[derive(Clone)]
pub enum TxParser {
    SmallTxParser {
        tx_request: TypedTransaction,
        eth_signature: EthSignature,
        logs: Vec<Log>,
        status: Option<u64>,
    },
    BigTxParser {
        holder_data: Vec<u8>,
        pieces: BTreeMap<usize, usize>, // Ordered collection mapping piece offset to piece length
        logs: Vec<Log>,
        status: Option<u64>,
    },
}

impl TxParser {
    pub fn new_small_tx_parser(
        tx_request: TypedTransaction,
        eth_signature: EthSignature,
    ) -> Result<Self> {
        Ok(Self::SmallTxParser {
            tx_request,
            eth_signature,
            logs: vec![],
            status: None,
        })
    }

    pub fn new_big_tx_parser() -> Self {
        Self::BigTxParser {
            holder_data: Vec::new(),
            pieces: BTreeMap::new(),
            logs: vec![],
            status: None,
        }
    }

    pub fn write_trx_data_to_holder(&mut self, offset: usize, data: &[u8]) {
        if let Self::BigTxParser {
            ref mut holder_data,
            ref mut pieces,
            ..
        } = self
        {
            if holder_data.len() < offset + data.len() {
                holder_data.resize(offset + data.len(), 0);
            };

            holder_data.as_mut_slice()[offset..][..data.len()].copy_from_slice(data);
            pieces.insert(offset, data.len());
        } else {
            tracing::warn!("Unable to set holder data because transaction is not supposed to have holder account")
        }
    }

    pub fn is_complete(&self) -> bool {
        match self {
            TxParser::SmallTxParser { .. } => true,
            TxParser::BigTxParser { pieces, .. } => {
                // check that all found pieces are forming uninterrupted buffer
                let mut position: usize = 0;
                for (offset, length) in pieces.iter() {
                    if *offset != position {
                        return false;
                    }

                    position += length;
                }

                position != 0
            }
        }
    }

    fn set_logs(&mut self, events: Vec<Log>) {
        let logs = match self {
            TxParser::SmallTxParser { logs, .. } => logs,
            TxParser::BigTxParser { logs, .. } => logs,
        };
        *logs = events;
    }

    fn set_status(&mut self, evm_status: Option<u64>) {
        let status = match self {
            TxParser::SmallTxParser { status, .. } => status,
            TxParser::BigTxParser { status, .. } => status,
        };

        *status = evm_status;
    }

    // Returns transaction status if provided meta corresponds to Solana transaction
    // containing finalization step of EVM transaction. Otherwise None
    pub fn parse_transaction_logs(
        &mut self,
        meta: &UiTransactionStatusMeta,
        tx_hash: TxHash,
    ) -> Result<Option<u64>> {
        let (events, exit_reason) = match &meta.log_messages {
            OptionSerializer::Some(logs) => {
                let mut parser = LogParser::new();
                parser.parse(logs)?;

                (parser.events, parser.exit_reason)
            }
            _ => (vec![], None),
        };

        let status = if let Some(reason) = exit_reason {
            let status = (reason.code == 0) as u64;
            self.set_status(Some(status));

            if status != 1 {
                tracing::info!("tx {}, {}", tx_hash, reason.log());
            }

            Some(status)
        } else {
            None
        };

        if !events.is_empty() {
            self.set_logs(events)
        }

        Ok(status)
    }

    pub fn build(
        &self,
        log_index: U256,
        transaction_index: U64,
        block_hash: H256,
        block_number: U64,
    ) -> Result<(Transaction, TransactionReceipt)> {
        if !self.is_complete() {
            tracing::warn!("Transaction parser is not complete");
            return Err(InternalError);
        }

        let (tx_request, eth_signature, logs, status) = match self {
            TxParser::SmallTxParser {
                tx_request,
                eth_signature,
                logs,
                status,
            } => (
                tx_request.clone(),
                eth_signature.clone(),
                logs.clone(),
                status.clone(),
            ),
            TxParser::BigTxParser {
                holder_data,
                logs,
                status,
                ..
            } => {
                let (tx, e_sig) = decode_transaction_from_rlp(&Rlp::new(&holder_data.as_slice()))?;
                (tx, e_sig, logs.clone(), status.clone())
            }
        };

        let transaction_hash = tx_request.hash(&eth_signature);
        let transaction = new_transaction(
            transaction_hash,
            &tx_request,
            &eth_signature,
            Some(block_hash),
            Some(block_number),
            Some(transaction_index),
        );

        let mut transaction_log_index = U256::zero();
        let receipt = TransactionReceipt {
            transaction_hash,
            transaction_index,
            transaction_type: transaction.transaction_type,
            block_hash: Some(block_hash),
            block_number: Some(block_number),
            from: transaction.from,
            to: transaction.to,
            gas_used: Some(U256::from(0)), // TODO gas calculation
            cumulative_gas_used: U256::from(0),
            contract_address: calc_contract_address(
                tx_request.to(),
                tx_request.from(),
                tx_request.nonce(),
            )?,
            logs: logs
                .iter()
                .map(|e| {
                    let result = Log {
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        transaction_hash: Some(transaction_hash),
                        transaction_index: Some(transaction_index),
                        log_index: Some(
                            log_index
                                .checked_add(transaction_log_index)
                                .expect("Unable to increment log index"),
                        ),
                        transaction_log_index: Some(transaction_log_index),
                        removed: Some(false),
                        ..e.clone()
                    };
                    transaction_log_index = transaction_log_index.add(U256::one());
                    result
                })
                .collect::<Vec<Log>>(),
            status: status.map(|a| a.into()),
            logs_bloom: Bloom::default(),
            root: None,
            effective_gas_price: None,
            other: OtherFields::default(),
        };

        Ok((transaction, receipt))
    }
}
