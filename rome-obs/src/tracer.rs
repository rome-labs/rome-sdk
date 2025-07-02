use crate::builder::exporter::OtelExporterBuilder;
use crate::builder::resource::OtelResourceBuilder;
use crate::env::OtelEnv;

use std::time::Duration;

use backoff::ExponentialBackoff;
use opentelemetry_sdk::trace::BatchConfigBuilder;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::fmt::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Registry;

use crate::sampler::OtelSampler;

/// Default queue size to buffer spans for delayed processing.
const DEFAULT_QUEUE_SIZE: usize = 65_536;
/// Maximum queue size to buffer spans for delayed processing.
const MAX_QUEUE_SIZE: usize = 100_000;
/// Maximum number of spans to process in a single batch.
const MAX_EXPORT_BATCH_SIZE: usize = 256;
/// The delay interval in milliseconds between two consecutive processing of batches.
const SCHEDULED_DELAY: u64 = 3_000;
/// Initial delay for exponential backoff in milliseconds
const INITIAL_BACKOFF_DELAY: u64 = 100;
/// Maximum delay for exponential backoff in milliseconds
const MAX_BACKOFF_DELAY: u64 = 30_000;

pub struct OtelTracer;

impl OtelTracer {
    /// Initializes a global OpenTelemetry tracing subscriber using environment configuration.
    ///
    /// This function conditionally enables:
    /// - stdout logging (with configurable log level)
    /// - OpenTelemetry tracing (using OTLP exporter)
    ///
    /// Environment variables:
    /// - `OTEL_STDOUT_LOGGING=true` enables stdout logs
    /// - `OTEL_TRACING_ENABLED=true` enables OpenTelemetry tracing
    /// - `OTEL_LOG_LEVEL=trace|debug|info|warn|error` controls log level (used with stdout)
    ///
    /// # Example
    /// ```no_run
    /// use rome_obs::tracer::OtelTracer;
    /// use rome_obs::Value;
    ///
    /// OtelTracer::init_from_env(Value::String("my-service".into()));
    /// ```
    pub fn init_from_env(
        service_name: opentelemetry::Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let enable_stdout_logging = OtelEnv::is_stdout_logging_enabled();
        let enable_otel_tracing = OtelEnv::is_otel_tracing_enabled();

        let std_out_layer = enable_stdout_logging
            .then(OtelEnv::log_level)
            .transpose()?
            .map(|lvl| Layer::default().with_writer(std::io::stdout.with_max_level(lvl)));

        let otel_layer = if enable_otel_tracing {
            let max_queue_size = OtelEnv::tracing_queue_size().unwrap_or(DEFAULT_QUEUE_SIZE);
            if max_queue_size > MAX_QUEUE_SIZE {
                return Err(format!(
                    "Provided queue size {} exceeds maximum allowed queue size {}",
                    max_queue_size, MAX_QUEUE_SIZE,
                )
                .into());
            }

            // Configure exponential backoff
            let backoff = ExponentialBackoff {
                initial_interval: Duration::from_millis(INITIAL_BACKOFF_DELAY),
                max_interval: Duration::from_millis(MAX_BACKOFF_DELAY),
                multiplier: 2.0,
                ..Default::default()
            };

            let tracer = backoff::retry(backoff, || {
                let otel_exporter = OtelExporterBuilder::build_from_env().map_err(|e| {
                    tracing::warn!("Failed to build OTLP exporter, retrying: {}", e);
                    backoff::Error::transient(format!("Failed to build OTLP exporter: {}", e))
                })?;

                let batch_config = BatchConfigBuilder::default()
                    .with_max_queue_size(max_queue_size)
                    .with_max_export_batch_size(MAX_EXPORT_BATCH_SIZE)
                    .with_scheduled_delay(Duration::from_millis(SCHEDULED_DELAY))
                    .build();

                let config = opentelemetry_sdk::trace::Config::default()
                    .with_resource(OtelResourceBuilder::build(service_name.clone()))
                    .with_sampler(OtelSampler);

                let pipeline = opentelemetry_otlp::new_pipeline()
                    .tracing()
                    .with_exporter(otel_exporter);

                pipeline
                    .with_trace_config(config)
                    .with_batch_config(batch_config)
                    .install_batch(opentelemetry_sdk::runtime::Tokio)
                    .map_err(|e| {
                        tracing::warn!("Failed to install otel tracer, retrying: {}", e);
                        backoff::Error::transient(format!("Failed to install OTLP tracer: {}", e))
                    })
            })
            .map_err(|e| format!("Failed to install otel tracer after retries: {}", e))?;

            Some(tracing_opentelemetry::layer().with_tracer(tracer))
        } else {
            None
        };

        let filter = EnvFilter::new(format!(
            "info,{}=trace,h2=off,tower_http=off,hyper=info",
            service_name
        ));

        let subscriber = Registry::default()
            .with(filter)
            .with(std_out_layer)
            .with(otel_layer);

        tracing::subscriber::set_global_default(subscriber)
            .map_err(|e| format!("Failed to set global default subscriber: {}", e).into())
    }
}
