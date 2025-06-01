use opentelemetry_otlp::{new_exporter, TonicExporterBuilder, WithExportConfig};

use crate::env::OtelEnv;

/// Builder for OpenTelemetry exporter.
pub struct OtelExporterBuilder;

impl OtelExporterBuilder {
    /// Create a new instance of [TonicExporterBuilder]
    /// with the given endpoint
    pub fn build(endpoint: url::Url) -> TonicExporterBuilder {
        new_exporter().tonic().with_endpoint(endpoint)
    }

    /// Create a new instance of [TonicExporterBuilder] from environment variables
    /// expects the `OTLP_RECEIVER_URL` environment variable to be set
    pub fn build_from_env() -> Result<TonicExporterBuilder, Box<dyn std::error::Error>> {
        OtelEnv::otel_receiver_url().map(Self::build)
    }
}
