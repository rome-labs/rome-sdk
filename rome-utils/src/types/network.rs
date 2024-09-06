use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// WebSocket stream type that can be either TLS or not over a TCP stream
pub type MaybeTlsTcpSocketStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
