use super::JsonRpcError;

/// JSON-RPC response type
#[derive(Debug, serde::Deserialize)]
pub struct JsonRpcResponse<T> {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Request id
    pub id: Option<String>,
    /// Result
    pub result: Option<T>,
    /// Error
    pub error: Option<JsonRpcError>,
}
