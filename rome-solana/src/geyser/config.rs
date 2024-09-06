/// Configuration for the kafka client
#[derive(clap::Args, Debug)]
pub struct GeyserKafkaConfig {
    /// Kafka hosts
    #[clap(long)]
    pub kafka_hosts: Vec<String>,

    /// Poll interval
    #[clap(long)]
    pub kafka_poll_interval_ms: u64,
}
