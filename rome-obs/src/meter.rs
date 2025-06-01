use crate::builder::exporter::OtelExporterBuilder;
use crate::builder::meter::OtelMeterBuilder;
use crate::env::OtelEnv;

use std::sync::{Arc, OnceLock};

use opentelemetry::{global, metrics::Meter, KeyValue, Value};

pub struct OtelMeter {
    meter: Meter,
}

impl OtelMeter {
    /// Initializes the OpenTelemetry metrics system from environment variables.
    ///
    /// This should be called once during startup. It reads environment config,
    /// sets up the exporter and meter provider, and stores a globally accessible meter.
    ///
    /// # Example
    /// ```no_run
    /// use rome_obs::meter::OtelMeter;
    /// use rome_obs::Value;
    ///
    /// OtelMeter::init_from_env(Value::String("my-service".into()));
    /// ```
    pub fn init_from_env(service_name: Value) -> Result<(), Box<dyn std::error::Error>> {
        let enable_otel_metrics = OtelEnv::is_otel_metrics_enabled();

        if enable_otel_metrics {
            let exporter = OtelExporterBuilder::build_from_env()?;
            let meter_provider = OtelMeterBuilder::build(service_name.clone(), exporter);
            global::set_meter_provider(meter_provider);
        }

        let metrics = OtelMeter {
            meter: global::meter(service_name.to_string()),
        };

        GLOBAL_METRICS
            .set(Arc::new(metrics))
            .map_err(|_| "Failed to set global metrics")?;

        Ok(())
    }

    /// Returns a reference to the globally-initialized `OtelMeter`, if available.
    ///
    /// # Example
    /// ```no_run
    /// use rome_obs::meter::OtelMeter;
    ///
    /// if let Some(metrics) = OtelMeter::get() {
    ///     metrics.count("app.requests".into(), None);
    /// }
    /// ```
    pub fn get() -> Option<&'static Arc<Self>> {
        GLOBAL_METRICS.get()
    }

    /// Increments a counter metric by 1 with optional attributes.
    ///
    /// # Example
    /// ```no_run
    /// use rome_obs::meter::OtelMeter;
    /// use rome_obs::{KeyValue, Value};
    ///
    /// let attributes = [
    ///     KeyValue::new("contract", Value::String("0xabc...".into())),
    ///     KeyValue::new("method", Value::String("setBalance".into())),
    /// ];
    ///
    /// if let Some(metrics) = OtelMeter::get() {
    ///     metrics.count("contract.call".into(), Some(&attributes));
    /// }
    /// ```
    pub fn count(&self, counter_name: String, attributes: Option<&[KeyValue]>) {
        let counter = self.meter.u64_counter(counter_name).init();
        counter.add(1, attributes.unwrap_or_default());
    }

    /// Records a gauge value (like a point-in-time measurement) with associated attributes.
    ///
    /// # Example
    /// ```no_run
    /// use rome_obs::meter::OtelMeter;
    /// use rome_obs::{KeyValue, Value};
    ///
    /// let attrs = vec![
    ///     KeyValue::new("source", "relayer"),
    ///     KeyValue::new("block_hash", Value::String("0xabc123".into())),
    /// ];
    ///
    /// if let Some(metrics) = OtelMeter::get() {
    ///     metrics.record("relayer.slot.fetch_latency", 42, Some(attrs));
    /// }
    /// ```
    pub async fn record(
        &self,
        name: &'static str,
        measurement: u64,
        attributes: Option<Vec<KeyValue>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let instrument = self.meter.u64_observable_gauge(name).try_init()?;
        let attrs = attributes.unwrap_or_default();

        self.meter
            .register_callback(&[instrument.as_any()], move |observer| {
                observer.observe_u64(&instrument, measurement, &attrs)
            })?;

        Ok(())
    }
}

/// Singleton reference to the global `OtelMeter` instance.
static GLOBAL_METRICS: OnceLock<Arc<OtelMeter>> = OnceLock::new();
