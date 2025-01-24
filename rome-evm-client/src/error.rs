use ethers::types::SignatureError;
use rlp::DecoderError;
use {
    ethers::types::transaction::request::RequestError, rome_evm::error::RomeProgramError,
    solana_client::client_error::ClientError, std::sync::PoisonError, thiserror::Error,
};

pub type ProgramResult<T> = std::result::Result<T, RomeEvmError>;

#[derive(Debug, Error)]
pub enum RomeEvmError {
    #[error("rpc client error {0:?}")]
    RpcClientError(ClientError),

    #[error("RomeEvmError: {0}")]
    RomeProgramError(RomeProgramError),

    #[error("RequestError error: {0}")]
    RequestError(#[from] RequestError),

    #[error("mutex lock error: {0}")]
    MutexLockError(String),

    #[error("base64 error: {0}")]
    Base64DecodeError(#[from] base64::DecodeError),

    #[error("log parser error: {0}")]
    LogParserError(String),

    #[error("Internal error")]
    InternalError,

    #[error("Emulation reverted: {0}, data: {1:?})")]
    EmulationRevert(String, String),

    #[error("Emulation error: {0:?}")]
    EmulationError(String),

    #[error("There are no unlocked holders left")]
    NoFreeHolders,

    #[error("bincode error {0:?}")]
    BincodeError(bincode::Error),

    #[error("custom error: {0}")]
    Custom(String),

    #[error("Transaction must include chainId")]
    NoChainId,

    #[error("Unsupported chain Id: {0}")]
    UnsupportedChainId(u64),

    #[error("Tokio send error")]
    TokioSendError,

    #[error("Tokio Join error: {0}")]
    JoinError(tokio::task::JoinError),

    #[error("Diesel error: {0}")]
    DieselError(diesel::result::Error),

    #[error("Connection pool error: {0}")]
    ConnectionPoolError(r2d2::Error),

    #[error("RLP Decoder error: {0}")]
    TxDecodeError(DecoderError),

    #[error("SignatureError: {0}")]
    SignatureError(SignatureError),

    #[error("serde_json Error: {0}")]
    SerdeJsonError(serde_json::Error),

    #[error("Ethers provider error: {0}")]
    EthersProviderError(ethers::providers::ProviderError),
}

impl From<ClientError> for RomeEvmError {
    fn from(e: ClientError) -> RomeEvmError {
        RomeEvmError::RpcClientError(e)
    }
}

impl From<RomeProgramError> for RomeEvmError {
    fn from(e: RomeProgramError) -> RomeEvmError {
        RomeEvmError::RomeProgramError(e)
    }
}

impl From<bincode::Error> for RomeEvmError {
    fn from(e: bincode::Error) -> RomeEvmError {
        RomeEvmError::BincodeError(e)
    }
}

impl<T> From<PoisonError<T>> for RomeEvmError {
    fn from(err: PoisonError<T>) -> RomeEvmError {
        RomeEvmError::MutexLockError(err.to_string())
    }
}

impl From<tokio::task::JoinError> for RomeEvmError {
    fn from(e: tokio::task::JoinError) -> RomeEvmError {
        RomeEvmError::JoinError(e)
    }
}

impl From<diesel::result::Error> for RomeEvmError {
    fn from(e: diesel::result::Error) -> RomeEvmError {
        RomeEvmError::DieselError(e)
    }
}

impl From<r2d2::Error> for RomeEvmError {
    fn from(e: r2d2::Error) -> RomeEvmError {
        RomeEvmError::ConnectionPoolError(e)
    }
}

impl From<DecoderError> for RomeEvmError {
    fn from(e: DecoderError) -> RomeEvmError {
        RomeEvmError::TxDecodeError(e)
    }
}

impl From<SignatureError> for RomeEvmError {
    fn from(e: SignatureError) -> RomeEvmError {
        RomeEvmError::SignatureError(e)
    }
}

impl From<serde_json::Error> for RomeEvmError {
    fn from(e: serde_json::Error) -> RomeEvmError {
        RomeEvmError::SerdeJsonError(e)
    }
}

impl From<ethers::providers::ProviderError> for RomeEvmError {
    fn from(e: ethers::providers::ProviderError) -> RomeEvmError {
        RomeEvmError::EthersProviderError(e)
    }
}
