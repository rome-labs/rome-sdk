use ethers::types::Transaction;
use futures_util::{SinkExt, StreamExt};
use reqwest::{header::HeaderValue, Url};
use rome_utils::jsonrpc::{JsonRpcClient, JsonRpcRequest};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

pub mod types;
pub mod utils;

pub use types::{BlobParam, TxConfig};
use types::{DaSubmissionBlock, SubscribeBlobResponse};
use utils::{compress_blocks_to_blobs, decode_blob_to_tx, string_to_base64_namespace};

pub struct RomeDaClient {
    chain_id: u64,
    ws_url: String,
    auth_token: String,
    rpc_client: JsonRpcClient,
}

impl RomeDaClient {
    /// Create a new RomeDaClient instance
    pub fn new(
        celestia_rpc_url: Url,
        celestia_ws_url: String,
        celestia_auth_token: String,
        chain_id: u64,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            chain_id,
            ws_url: celestia_ws_url,
            auth_token: celestia_auth_token.clone(),
            rpc_client: JsonRpcClient::new_with_auth(celestia_rpc_url, celestia_auth_token),
        })
    }

    pub fn get_namespace(&self) -> anyhow::Result<String> {
        let rome_code = format!("rome_{:x}", self.chain_id);
        let namespace = string_to_base64_namespace(&rome_code)?;
        Ok(namespace)
    }

    pub async fn submit_blocks(&self, blocks: &[DaSubmissionBlock]) -> anyhow::Result<()> {
        let namespace = self.get_namespace()?;
        let blobs: Vec<BlobParam> = compress_blocks_to_blobs(&namespace, blocks)?;

        if !blobs.is_empty() {
            match self.submit_blobs(&blobs).await {
                Ok(height) => tracing::info!("Blobs submitted with height: {}", height),
                Err(e) => tracing::error!("Error submitting blob: {}", e),
            };
        }

        Ok(())
    }

    pub async fn submit_blobs(&self, blobs: &Vec<BlobParam>) -> anyhow::Result<u64> {
        let tx_config = TxConfig {
            gas_price: None,
            is_gas_price_set: None,
            gas: None,
            key_name: None,
            signer_address: None,
            fee_granter_address: None,
        };

        let res = self
            .rpc_client
            .send_req::<serde_json::Value>(
                JsonRpcRequest::new_with_params_owned(
                    "blob.Submit",
                    vec![serde_json::json!(blobs), serde_json::json!(tx_config)],
                ),
                None,
            )
            .await?;
        let height = serde_json::from_value(res)?;
        Ok(height)
    }

    pub async fn subscribe_blobs(
        &self,
    ) -> anyhow::Result<mpsc::Receiver<Result<Vec<BlobParam>, anyhow::Error>>> {
        let (tx, rx) = mpsc::channel(100);

        let mut req = self.ws_url.clone().into_client_request()?;

        let namespace = self.get_namespace()?;

        let headers = req.headers_mut();
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.auth_token)).unwrap(),
        );

        let (ws_stream, _) = connect_async(req).await.expect("Failed to connect");
        tracing::info!("Connected to Celestia");

        // Split the WebSocket stream
        let (mut write_stream, mut read_stream) = ws_stream.split();

        // JSON-RPC request to subscribe to blobs
        let subscribe_request = json!({
            "jsonrpc": "2.0",
            "method": "blob.Subscribe",
            "id": 1,
            "params": [
                namespace
            ]
        });

        // Send the subscription request
        write_stream
            .send(Message::Text(subscribe_request.to_string()))
            .await
            .expect("Failed to send");

        let tx_clone = tx.clone();
        tokio::spawn(async move {
            while let Some(message) = read_stream.next().await {
                match message {
                    Ok(Message::Text(text)) => {
                        tracing::debug!("Received message: {:?}\n", text);
                        if text.contains("Blobs") {
                            let root = serde_json::from_str::<serde_json::Value>(&text).unwrap();
                            match serde_json::from_value::<SubscribeBlobResponse>(
                                root.get("params").unwrap().get(1).unwrap().clone(),
                            ) {
                                Ok(resp) => {
                                    tracing::debug!(
                                        "Received block header at height: {:?}",
                                        resp.height
                                    );

                                    let _ = tx_clone.send(Ok(resp.blobs)).await;
                                }
                                Err(e) => {
                                    tracing::error!("Error in parse response: {:?}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error in receiving message: {:?}", e);
                    }
                    _ => {}
                }
            }
        });

        Ok(rx)
    }

    pub async fn fetch_blobs_by_height(&self, height: u64) -> anyhow::Result<Vec<Transaction>> {
        let namespace = self.get_namespace()?;
        let params: Vec<serde_json::Value> = vec![height.into(), vec![namespace].into()];

        match self
            .rpc_client
            .send_req::<Vec<BlobParam>>(
                JsonRpcRequest::new_with_params_owned("blob.GetAll", params.clone()),
                None,
            )
            .await
        {
            Ok(res) => {
                let transactions: Vec<Transaction> = res
                    .iter()
                    .map(|blob| decode_blob_to_tx(blob).unwrap())
                    .collect();
                Ok(transactions)
            }
            Err(e) => {
                tracing::error!("Error in fetching blobs: {:?}", e);
                Err(e)
            }
        }
    }
}

#[test]
pub fn test_serilize_blocks() -> anyhow::Result<()> {
    use ethers::types::{Transaction, U256, U64};

    let mut tx1: Transaction = Transaction::default();
    tx1.nonce = U256::from(10000);

    let mut tx2: Transaction = Transaction::default();
    tx2.nonce = U256::from(10001);

    let mut tx3: Transaction = Transaction::default();
    tx3.nonce = U256::from(10002);

    let blocks = vec![
        DaSubmissionBlock {
            block_number: U64::from(9009),
            timestamp: U256::from(10000),
            transactions: vec![],
        },
        DaSubmissionBlock {
            block_number: U64::from(1050),
            timestamp: U256::from(10001),
            transactions: vec![tx1],
        },
        DaSubmissionBlock {
            block_number: U64::from(1051),
            timestamp: U256::from(10002),
            transactions: vec![],
        },
        DaSubmissionBlock {
            block_number: U64::from(1052),
            timestamp: U256::from(10003),
            transactions: vec![],
        },
        DaSubmissionBlock {
            block_number: U64::from(1053),
            timestamp: U256::from(10004),
            transactions: vec![tx2, tx3],
        },
    ];

    let blobs: Vec<BlobParam> = compress_blocks_to_blobs(&"", &blocks)?;

    for blob in blobs {
        let tx = decode_blob_to_tx(&blob).unwrap();
        println!("  transaction: {:?}", tx);
    }

    Ok(())
}

#[ignore] // TODO Fix test
#[test]
pub fn test_serilize_blob() -> anyhow::Result<()> {
    let blob = BlobParam {
        namespace: "".to_string(),
        data: "".to_string(),
        share_version: 0,
        commitment: "".to_string(),
        index: 0,
    };

    let tx = decode_blob_to_tx(&blob).unwrap();
    println!("  transaction: {:?}", tx);

    Ok(())
}
