/// [tokio::task::JoinHandle] which returns [anyhow::Result<()>].
pub type AnyhowJoinHandle = tokio::task::JoinHandle<anyhow::Result<()>>;
