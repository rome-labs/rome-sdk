use tracing::Level;

/// Environment variable for logging level.
pub const LOG_LEVEL_ENV: &str = "LOG_LEVEL_ENV";
/// OTEL receiver URL environment variable
pub const OTEL_RECEIVER_URL_ENV: &str = "OTLP_RECEIVER_URL";
/// Environment variable to enable OpenTelemetry tracing.
pub const ENABLE_OTEL_TRACING_ENV: &str = "ENABLE_OTEL_TRACING";
/// Environment variable to enable OpenTelemetry metrics.
pub const ENABLE_OTEL_METRICS_ENV: &str = "ENABLE_OTEL_METRICS";
/// Environment variable to enable stdout logging.
pub const ENABLE_STDOUT_LOGGING_ENV: &str = "ENABLE_STDOUT_LOGGING";
/// Environment variable to accept queue size for otel tracing.
pub const OTEL_TRACING_QUEUE_SIZE: &str = "OTEL_TRACING_QUEUE_SIZE";

/// Set of utility functions to parse environment variables for OpenTelemetry configuration.
pub struct OtelEnv;

impl OtelEnv {
    /// Check if OpenTelemetry tracing is enabled.
    pub fn is_otel_tracing_enabled() -> bool {
        std::env::var(ENABLE_OTEL_TRACING_ENV)
            .unwrap_or("false".to_string())
            .parse()
            .unwrap_or_else(|_| panic!("must be a boolean"))
    }

    /// Check if OpenTelemetry metrics is enabled.
    pub fn is_otel_metrics_enabled() -> bool {
        std::env::var(ENABLE_OTEL_METRICS_ENV)
            .unwrap_or("false".to_string())
            .parse()
            .unwrap_or_else(|_| panic!("must be a boolean"))
    }

    /// Check if stdout logging is enabled.
    pub fn is_stdout_logging_enabled() -> bool {
        std::env::var(ENABLE_STDOUT_LOGGING_ENV)
            .unwrap_or("true".to_string())
            .parse()
            .unwrap_or_else(|_| panic!("must be a boolean"))
    }

    /// Parse [LOG_LEVEL_ENV] environment variable to a [tracing::Level].
    pub fn log_level() -> Result<Level, Box<dyn std::error::Error>> {
        Ok(std::env::var(LOG_LEVEL_ENV)
            .ok()
            .map(|s| s.to_uppercase())
            .and_then(|s| s.parse::<Level>().ok())
            .unwrap_or(Level::INFO))
    }

    /// Parse [OTEL_RECEIVER_URL_ENV] environment variable to a [url::Url].
    pub fn otel_receiver_url() -> Result<url::Url, Box<dyn std::error::Error>> {
        // Get the value of the environment variable
        let Ok(value) = std::env::var(OTEL_RECEIVER_URL_ENV) else {
            return Err(format!("{} must be set", OTEL_RECEIVER_URL_ENV).into());
        };

        // Parse the value as a URL
        let Ok(value) = url::Url::parse(&value) else {
            return Err(format!("{} must be a valid URL", OTEL_RECEIVER_URL_ENV).into());
        };

        Ok(value)
    }

    /// Parse [OTEL_TRACING_QUEUE_SIZE] environment variable to a [usize].
    pub fn tracing_queue_size() -> Result<usize, Box<dyn std::error::Error>> {
        Ok(std::env::var(OTEL_TRACING_QUEUE_SIZE)
            .map_err(|_| format!("{} must be set", OTEL_TRACING_QUEUE_SIZE))?
            .parse::<usize>()
            .map_err(|_| {
                format!(
                    "{} must be a valid unsigned integer",
                    OTEL_TRACING_QUEUE_SIZE
                )
            })?)
    }
}
