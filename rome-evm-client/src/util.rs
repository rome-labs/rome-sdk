use ethers::types::{NameOrAddress, TransactionRequest, U256};
use rome_evm::{tx::legacy::Legacy as LegacyTx, H160 as EvmH160};

pub struct RomeEvmUtil;

impl RomeEvmUtil {
    /// Convert [U256] to [rome_evm::U256]
    pub fn cast_u256(value: U256) -> rome_evm::U256 {
        let mut buf = [0; 32];

        value.to_big_endian(&mut buf);

        rome_evm::U256::from_big_endian(&buf)
    }

    /// Convert eth [TransactionRequest] to rome [Legacy]
    pub fn cast_transaction_request(value: &TransactionRequest, chain_id: u64) -> LegacyTx {
        LegacyTx {
            nonce: value.nonce.unwrap_or_default().as_u64(), // todo: load from chain?
            gas_price: value.gas_price.map(Self::cast_u256).unwrap_or_default(),
            gas_limit: value.gas.map(Self::cast_u256).unwrap_or_default(),
            to: value.to.clone().map(|v| match v {
                NameOrAddress::Address(addr) => EvmH160::from(addr.0),
                NameOrAddress::Name(_) => EvmH160::default(),
            }),
            value: value.value.map(Self::cast_u256).unwrap_or_default(),
            data: value.data.clone().unwrap_or_default().to_vec(),
            chain_id: value
                .chain_id
                .map(|a| a.as_u64().into())
                .unwrap_or(chain_id.into()),
            from: value.from.map(|v| EvmH160::from(v.0)).unwrap_or_default(),
            ..Default::default()
        }
    }
}
