pub mod builder;
pub mod env;
pub mod meter;
pub mod sampler;
pub mod tracer;

pub use opentelemetry::{KeyValue, Value};

pub struct Otel;

impl Otel {
    /// Initializes a global OpenTelemetry tracer and meter using environment-based configuration.
    ///
    /// This sets up both:
    /// - A **tracer**: for emitting structured spans and traces (via OTLP or stdout).
    /// - A **meter**: for recording metrics (counters, gauges, etc.) using the OTEL metrics API.
    ///
    /// # Configuration
    /// This function reads environment variables to conditionally enable and configure tracing and metrics:
    ///
    /// - `OTEL_TRACING_ENABLED=true`: Enables OpenTelemetry tracing via OTLP exporter.
    /// - `OTEL_STDOUT_LOGGING=true`: Enables tracing output to stdout.
    /// - `OTEL_LOG_LEVEL=trace|debug|info|warn|error`: Sets the log level for stdout logging.
    /// - `OTEL_METRICS_ENABLED=true`: Enables metrics collection/export.
    ///
    /// # Example
    /// ```rust
    /// use rome_obs::Otel;
    ///
    /// Otel::init_from_env("my-service").unwrap();
    ///
    /// ```
    pub fn init_from_env(
        service_name: impl Into<opentelemetry::Value> + Clone,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let service_name = service_name.into();

        tracer::OtelTracer::init_from_env(service_name.clone())?;
        meter::OtelMeter::init_from_env(service_name)?;

        Ok(())
    }
}
