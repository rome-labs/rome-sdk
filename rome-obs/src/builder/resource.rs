use opentelemetry::{KeyValue, Value};
use opentelemetry_sdk::Resource;

/// Builder for OpenTelemetry resource.
pub struct OtelResourceBuilder;

impl OtelResourceBuilder {
    /// Initiate [Resource] with the given service name
    pub fn build(service_name: Value) -> Resource {
        // Create the key-value pair for the resource
        let kvs = [KeyValue::new("service.name", service_name)];
        // Create a new resource with the key-value pair
        Resource::new(kvs)
    }
}
