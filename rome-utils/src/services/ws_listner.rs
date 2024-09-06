use std::fmt::Debug;

use anyhow::{bail, Context};
use futures_util::{sink::SinkExt, StreamExt};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::types::network::MaybeTlsTcpSocketStream;

/// WsListner is a generic struct that listens to a given websocket address
/// and sends the data to the channel.
/// Protocol: JSON-RPC 2.0 over Websocket
pub struct WsListner<'a, T: serde::de::DeserializeOwned + Debug> {
    /// Channel to send the polled data
    pub tx: UnboundedSender<T>,
    /// Address to poll
    pub ws_addr: Url,
    /// payload to send
    pub payload: JsonRpcRequest<'a>,
}

impl<T: serde::de::DeserializeOwned + Debug> WsListner<'_, T> {
    /// Subscribe to the websocket address and send the data to the channel
    /// Returns the websocket stream
    pub async fn connect(&self) -> anyhow::Result<MaybeTlsTcpSocketStream> {
        tracing::info!("Subscribing: {}", &self.ws_addr);

        let (mut socket, _) = tokio_tungstenite::connect_async(self.ws_addr.as_str())
            .await
            .context("Failed to connect")?;

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&self.payload)?,
            ))
            .await?;

        Ok(socket)
    }

    /// Listen to the websocket stream and send the data to the channel
    /// Checks for errors and unknown ids
    pub async fn listen_socket(&self, socket: MaybeTlsTcpSocketStream) -> anyhow::Result<()> {
        let (mut tx, mut rx) = socket.split();

        while let Some(Ok(msg)) = rx.next().await {
            match msg {
                Message::Text(msg) => {
                    tracing::debug!("Received message: {}", &msg);

                    let msg: JsonRpcResponse<T> =
                        serde_json::from_str(&msg).context("Failed to parse message")?;

                    let Some(id) = msg.id.as_ref() else {
                        tracing::warn!("No id in message: {:?}", msg);
                        continue;
                    };

                    if id != self.payload.id {
                        tracing::warn!("Unknown id: {:?}", id);
                        continue;
                    }

                    if msg.error.is_some() {
                        tracing::error!("Error: {:?}", msg.error);
                        continue;
                    }

                    let Some(result) = msg.result else {
                        tracing::warn!("No result in message: {:?}", msg);
                        continue;
                    };

                    self.tx.send(result).unwrap();
                }
                Message::Ping(res) | Message::Pong(res) => {
                    tracing::info!("Ping/Pong ${res:?}");

                    tx.send(Message::Pong(res))
                        .await
                        .context("Failed to send pong")?;
                }
                Message::Close(_) => bail!("Received close message. Closing..."),
                msg => tracing::warn!("Unknown message: {:?}", msg),
            }
        }

        bail!("Connection closed unexpectedly");
    }

    /// Uses [Self::connect] and [Self::listen_socket] to listen to the websocket address
    /// upto 5 times before exiting
    pub async fn listen(self) -> anyhow::Result<()> {
        let mut sub_errors = 0;
        let mut parse_errors = 0;

        while sub_errors < 5 && parse_errors < 5 {
            let Ok(socket) = self.connect().await else {
                tracing::error!("Failed to subscribe. Reconnecting...");
                sub_errors += 1;
                continue;
            };

            sub_errors = 0;

            let Err(err) = self.listen_socket(socket).await else {
                unreachable!();
            };

            parse_errors += 1;

            tracing::error!("Connection error: {:?}", err);
            tracing::error!("Reconnecting...");
        }

        bail!("Too many errors connecting to {}. Exiting...", self.ws_addr);
    }
}
