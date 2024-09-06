/// JSON-RPC error type
#[derive(Debug, serde::Deserialize)]
pub struct JsonRpcError {
    /// Error code
    pub code: i32,
    /// Error message
    pub message: String,
}
