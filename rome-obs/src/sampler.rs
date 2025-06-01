use opentelemetry::trace::{SamplingDecision, SamplingResult, TraceContextExt};
use opentelemetry::KeyValue;
use opentelemetry_sdk::trace::ShouldSample;

#[derive(Debug, Clone, Copy)]
pub struct OtelSampler;

impl ShouldSample for OtelSampler {
    fn should_sample(
        &self,
        parent_context: Option<&opentelemetry::Context>,
        _: opentelemetry::trace::TraceId,
        _: &str,
        _: &opentelemetry::trace::SpanKind,
        _: &[KeyValue],
        _: &[opentelemetry::trace::Link],
    ) -> SamplingResult {
        SamplingResult {
            decision: SamplingDecision::RecordAndSample,
            attributes: Vec::new(),
            trace_state: match parent_context {
                Some(ctx) => ctx.span().span_context().trace_state().clone(),
                None => Default::default(),
            },
        }
    }
}
