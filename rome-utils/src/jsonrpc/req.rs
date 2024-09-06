use std::borrow::Cow;

/// JSON-RPC request type
#[derive(Debug, serde::Serialize)]
pub struct JsonRpcRequest<'a> {
    /// JSON-RPC version
    pub jsonrpc: &'a str,
    /// Method to call
    pub method: &'a str,
    /// Parameters
    pub params: Cow<'a, Vec<serde_json::Value>>,
    /// Request id
    pub id: &'a str,
}

/// JSON-RPC request type with 'static lifetime
pub type JsonRpcRequestOwned = JsonRpcRequest<'static>;

impl<'a> JsonRpcRequest<'a> {
    /// Create a new JSON-RPC request
    pub fn new_owned(method: &'a str, params: Option<Vec<serde_json::Value>>) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params: Cow::Owned(params.unwrap_or_default()),
            id: "1",
        }
    }

    /// Create a new JSON-RPC  request
    pub fn new_borrowed(method: &'a str, params: &'a Vec<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params: Cow::Borrowed(params),
            id: "1",
        }
    }

    /// Create a new JSON-RPC request with a method
    pub fn new(method: &'a str) -> Self {
        Self::new_owned(method, None)
    }

    /// Create a new JSON-RPC request with a method and params
    pub fn new_with_params_owned(method: &'a str, params: Vec<serde_json::Value>) -> Self {
        Self::new_owned(method, Some(params))
    }

    /// Create a new JSON-RPC request with a method and a single param
    pub fn new_with_param_owned(method: &'a str, param: serde_json::Value) -> Self {
        Self::new_owned(method, Some(vec![param]))
    }

    /// Create a new JSON-RPC request with a method and params borrowed
    pub fn new_with_params_borrowed(method: &'a str, params: &'a Vec<serde_json::Value>) -> Self {
        Self::new_borrowed(method, params)
    }
}
