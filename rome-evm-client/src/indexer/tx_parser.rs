use {
    crate::{
        error::{
            ProgramResult,
            RomeEvmError::{InternalError, TypedTransactionError},
        },
        indexer::log_parser::LogParser,
    },
    ethers::types::{
        transaction::eip2718::TypedTransaction, Address, Bloom, Bytes, Log, NameOrAddress,
        OtherFields, Signature as EthSignature, Transaction, TransactionReceipt, TxHash, H256,
        U256, U64,
    },
    rlp::Rlp,
    solana_program::keccak::hash,
    solana_transaction_status::{option_serializer::OptionSerializer, UiTransactionStatusMeta},
    std::{collections::BTreeMap, ops::Add},
};

#[derive(Default, Debug, Clone)]
pub struct GasReport {
    pub gas_value: U256,
    pub gas_recipient: Option<Address>,
}

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
            _ => tx_request.gas_price(),
        },
        gas: tx_request.gas().map_or(U256::default(), |v| *v),
        to: tx_request.to().map(|v| match v {
            NameOrAddress::Address(addr) => *addr,
            NameOrAddress::Name(_) => Address::default(),
        }),
        value: *tx_request.value().unwrap_or(&U256::default()),
        input: tx_request.data().unwrap_or(&Bytes::default()).clone(),
        v: U64::from(eth_signature.v),
        r: eth_signature.r,
        s: eth_signature.s,
        access_list: tx_request.access_list().cloned(),
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
) -> ProgramResult<Option<Address>> {
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
        InternalError
    })
}

pub fn decode_transaction_from_rlp(rlp: &Rlp) -> ProgramResult<(TypedTransaction, EthSignature)> {
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
        gas_report: GasReport,
    },
    BigTxParser {
        holder_data: Vec<u8>,
        pieces: BTreeMap<usize, usize>, // Ordered collection mapping piece offset to piece length
        logs: Vec<Log>,
        status: Option<u64>,
        gas_report: GasReport,
    },
}

impl TxParser {
    pub fn new_small_tx_parser(
        tx_request: TypedTransaction,
        eth_signature: EthSignature,
    ) -> ProgramResult<Self> {
        Ok(Self::SmallTxParser {
            tx_request,
            eth_signature,
            logs: vec![],
            status: None,
            gas_report: GasReport::default(),
        })
    }

    pub fn new_big_tx_parser() -> Self {
        Self::BigTxParser {
            holder_data: Vec::new(),
            pieces: BTreeMap::new(),
            logs: vec![],
            status: None,
            gas_report: GasReport::default(),
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

    fn set_gas_report(&mut self, report: GasReport) {
        let gas_report = match self {
            TxParser::SmallTxParser { gas_report, .. } => gas_report,
            TxParser::BigTxParser { gas_report, .. } => gas_report,
        };
        *gas_report = report;
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
    ) -> ProgramResult<Option<u64>> {
        let (events, exit_reason, gas_value, gas_recipient) = match &meta.log_messages {
            OptionSerializer::Some(logs) => {
                let mut parser = LogParser::new();
                parser.parse(logs)?;

                (parser.events, parser.exit_reason, parser.gas_value, parser.gas_recipient)
            }
            _ => (vec![], None, None, None),
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

        if let Some(gas_value) = gas_value {
            self.set_gas_report(GasReport{ gas_value, gas_recipient });
        }

        Ok(status)
    }

    pub fn build(
        &self,
        log_index: U256,
        transaction_index: U64,
        block_hash: H256,
        block_number: U64,
        block_gas_used: U256,
    ) -> ProgramResult<(Transaction, TransactionReceipt, GasReport)> {
        if !self.is_complete() {
            tracing::warn!("Transaction parser is not complete");
            return Err(InternalError);
        }

        let (tx_request, eth_signature, logs, status, gas_report) = match self {
            TxParser::SmallTxParser {
                tx_request,
                eth_signature,
                logs,
                status,
                gas_report,
            } => (
                tx_request.clone(),
                eth_signature.clone(),
                logs.clone(),
                status.clone(),
                gas_report.clone(),
            ),
            TxParser::BigTxParser {
                holder_data,
                logs,
                status,
                gas_report,
                ..
            } => {
                let (tx, e_sig) = decode_transaction_from_rlp(&Rlp::new(holder_data.as_slice()))?;
                (tx, e_sig, logs.clone(), status.clone(), gas_report.clone())
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
            gas_used: Some(gas_report.gas_value),
            cumulative_gas_used: block_gas_used
                .checked_add(gas_report.gas_value)
                .expect("This must never happen - block gas overflow!"),
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
            effective_gas_price: if let Some(price) = transaction.gas_price {
                Some(price)
            } else if let Some(price) = transaction.max_priority_fee_per_gas {
                Some(price)
            } else {
                panic!("No gas_price nor max_priority_fee_per_gas defined")
            },
            other: OtherFields::default(),
        };

        Ok((transaction, receipt, gas_report))
    }
}
