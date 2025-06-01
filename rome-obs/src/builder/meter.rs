use opentelemetry_otlp::{new_pipeline, TonicExporterBuilder};
use opentelemetry_sdk::{metrics::SdkMeterProvider, runtime::Tokio};

use super::resource::OtelResourceBuilder;

/// Builder for OpenTelemetry meter.
pub struct OtelMeterBuilder;

impl OtelMeterBuilder {
    /// Initializes the OpenTelemetry metrics system
    pub fn build(
        service_name: opentelemetry::Value,
        otel_exporter: TonicExporterBuilder,
    ) -> SdkMeterProvider {
        new_pipeline()
            .metrics(Tokio)
            .with_exporter(otel_exporter)
            .with_resource(OtelResourceBuilder::build(service_name))
            .build()
            .unwrap()
    }
}
