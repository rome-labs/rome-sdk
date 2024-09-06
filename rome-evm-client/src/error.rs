use {
    ethers::types::transaction::{eip2718::TypedTransactionError, request::RequestError},
    rome_evm::error::RomeProgramError,
    solana_client::client_error::ClientError,
    std::sync::PoisonError,
    thiserror::Error,
};

pub type Result<T> = std::result::Result<T, RomeEvmError>;

#[derive(Debug, Error)]
pub enum RomeEvmError {
    #[error("rpc client error {0:?}")]
    RpcClientError(ClientError),

    #[error("RomeEvmError: {0}")]
    RomeProgramError(RomeProgramError),

    #[error("TypedTransactionError error: {0}")]
    TypedTransactionError(#[from] TypedTransactionError),

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

    #[error("Revert message: {0}, data: {1:?})")]
    Revert(String, Vec<u8>),

    #[error("There are no unlocked holders left")]
    NoFreeHolders,

    #[error("bincode error {0:?}")]
    BincodeError(bincode::Error),

    #[error("custom error: {0}")]
    Custom(String),

    #[error("emulation error: {0}")]
    EmulationError(String),

    #[error("there are no available holder accounts")]
    NoAvailableHolders,

    #[error("not enough attempts to send transaction")]
    NotEnoughAttempts,

    #[error("solana transaction can contain up to 256 accounts")]
    TooManyAccounts,

    #[error("Transaction must include chainId")]
    NoChainId,

    #[error("Unsupported chain Id: {0}")]
    UnsupportedChainId(u64),

    #[error("Cross-rollup execution is not available for given transactions")]
    CrossRollupExecutionNotAvailable,

    #[error("Tokio send error")]
    TokioSendError,
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
