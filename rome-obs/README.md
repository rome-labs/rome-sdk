# rome-obs

The observability module used by all services within Rome Protocol. 
The following is a summary the environment variables used to configure logging, tracing, and metrics through this crate. 

## LOG_LEVEL

* **Description:** Sets the global logging level for rome-obs.
* **Allowed Values:** `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`
* **Default:** `INFO` (if unset)
* **Notes:**
  * Controls which logs appear on stdout or in any attached log collector.

## OTLP_RECEIVER_URL

* **Description:** The endpoint URL of your OTLP (OpenTelemetry Protocol) collector/receiver. Required if `ENABLE_OTEL_TRACING="true"` or `ENABLE_OTEL_METRICS="true"`.
* **Notes:**
  * If OTLP_RECEIVER_URL is unset or empty while ENABLE_OTEL_TRACING or ENABLE_OTEL_METRICS is "true", all trace/metric calls become no-op.

## ENABLE_OTEL_TRACING

* **Description:** Toggles OpenTelemetry tracing.
* **Allowed Values:**
  * `"true"`
  * `"false"` 
* **Default:** `"false"` (if unset)
* **Notes:**
  * Must be exactly `"true"` (lowercase) to enable tracing.
  * The call to `Otel::init_from_env("service_name")?` will read this value at startup.

## ENABLE_OTEL_METRICS

* **Description:** Toggles OpenTelemetry metrics collection.
* **Allowed Values:**
  * `"true"` 
  * `"false"`
* **Default:** `"false"` (if unset)
* **Notes:**
  * Must be exactly `"true"` (lowercase) to enable metrics.

## ENABLE_STDOUT_LOGGING

* **Description:** Controls whether a stdout logging layer is attached.
* **Allowed Values:**
  * `"true"`
  * `"false"`
* **Default:** `"false"` (if unset)
* **Notes:**
  * If both `ENABLE_STDOUT_LOGGING="true"` and `ENABLE_OTEL_TRACING="true"`, logs appear on stdout **and** spans are forwarded to OTLP.

## OTEL_TRACING_QUEUE_SIZE

* **Description:** Sets the maximum number of spans buffered in memory before they are sent in batches.
* **Default:** `65_536` (if unset)
* **Limits:**
  * Must be a positive integer ≤ `100_000`.
  * If set above `100_000`, rome-obs will abort startup with an error.

## Initializing Tracing and Metrics

All services should call:

```rust
Otel::init_from_env("service_name")?;
```

Replace `"service_name"` with your actual service name (e.g., `"order-processor"`, `"payment-gateway"`, etc.).
