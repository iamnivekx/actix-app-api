use opentelemetry::{global, trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::{MetricExporter, SpanExporter};
use opentelemetry_sdk::{
    metrics::{MeterProviderBuilder, PeriodicReader, SdkMeterProvider},
    propagation::TraceContextPropagator,
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
    Resource,
};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_bunyan_formatter::{BunyanFormattingLayer, JsonStorageLayer};
use tracing_opentelemetry::MetricsLayer;
use tracing_subscriber::{
    filter::EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt, Layer,
};
// Re-export tracing crates
pub use tracing;
pub use tracing_subscriber;

/// A boxed tracing [Layer].
pub type DynLayer<S> = dyn Layer<S> + Send + Sync;
pub type BoxLayer<DynLayer> = Box<DynLayer>;

// Create a Resource that captures information about the entity for which telemetry is recorded.
fn get_resource(name: &str, version: &str) -> Resource {
    Resource::builder()
        .with_schema_url(
            [
                KeyValue::new("service.name", name.to_string()),
                KeyValue::new("service.version", version.to_string()),
            ],
            "https://opentelemetry.io/schemas/1.33.0",
        )
        .build()
}

// Construct MeterProvider for MetricsLayer
fn init_meter_provider(name: &str, version: &str) -> SdkMeterProvider {
    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_temporality(opentelemetry_sdk::metrics::Temporality::default())
        .build()
        .expect("Failed to build MetricExporter");

    let reader =
        PeriodicReader::builder(exporter).with_interval(std::time::Duration::from_secs(30)).build();

    let mut meter_provider =
        MeterProviderBuilder::default().with_resource(get_resource(name, version));
    meter_provider = meter_provider.with_reader(reader);

    let meter_provider = meter_provider.build();
    global::set_meter_provider(meter_provider.clone());

    meter_provider
}

// Construct TracerProvider for OpenTelemetryLayer
fn init_tracer_provider(name: &str, version: &str) -> SdkTracerProvider {
    // Spans are exported in batch - recommended setup for a production application.
    global::set_text_map_propagator(TraceContextPropagator::new());
    let exporter =
        SpanExporter::builder().with_tonic().build().expect("Failed to build SpanExporter");

    let tracer_provider = SdkTracerProvider::builder()
        // Customize sampling strategy
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(1.0))))
        // If export trace to AWS X-Ray, you can use XrayIdGenerator
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(get_resource(name, version))
        .with_batch_exporter(exporter)
        .build();
    global::set_tracer_provider(tracer_provider.clone());
    tracer_provider
}

/// Initializes a new [Subscriber].
pub fn init_logging(srv_name: String, level: String) -> WorkerGuard {
    let srv_name = srv_name.clone();
    let version = env!("CARGO_PKG_VERSION");
    let tracer_provider = init_tracer_provider(srv_name.as_str(), version);
    let meter_provider = init_meter_provider(srv_name.as_str(), version);
    let tracer = tracer_provider.tracer(srv_name.to_string());

    // Filter based on level - trace, debug, info, warn, error
    // Tunable via `RUST_LOG` env variable
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    // Create a `tracing` layer to emit spans as structured logs to Metrics
    let metrics_layer = MetricsLayer::new(meter_provider.clone());
    // Create a `tracing` layer to emit spans as structured logs to otel
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let file_appended = tracing_appender::rolling::daily("./logs", "api");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appended);

    // Create a `tracing` layer to emit spans as structured logs to file system
    let file_layer = BunyanFormattingLayer::new(srv_name, non_blocking);

    // Create a `tracing` layer to emit spans as structured logs to stdout
    let std_layer = fmt::layer().with_writer(std::io::stderr);

    // Combined them all together in a `tracing` subscriber
    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(metrics_layer)
        .with(otel_layer)
        .with(file_layer)
        .with(JsonStorageLayer)
        .with(std_layer);

    subscriber.init();
    guard
}

#[cfg(test)]
mod test {

    use super::init_logging;
    use tracing::{debug, error, info, warn};
    #[tokio::main]
    #[test]
    async fn test_init_logging() {
        let guard = init_logging("App".to_string(), "debug".to_string());
        debug!(target: "logging", "debug something...");
        info!(target: "logging", "info something...");
        warn!(target: "logging", "warn something...");
        error!(target: "logging", "error something...");
        drop(guard)
    }
}
