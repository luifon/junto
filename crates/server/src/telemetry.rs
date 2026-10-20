//! Opt-in request monitoring with a fixed set of attributes (ADR 0007).

use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use opentelemetry::metrics::{Counter, Histogram, MeterProvider};
use opentelemetry::trace::{Span, SpanKind, Status, Tracer, TracerProvider};
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Stream};
use opentelemetry_sdk::trace::{
    BatchConfigBuilder, BatchSpanProcessor, SdkTracer, SdkTracerProvider,
};

const QUEUE_SIZE: usize = 1024;
const BATCH_SIZE: usize = 256;
const EXPORT_TIMEOUT: Duration = Duration::from_secs(2);
const EXPORT_INTERVAL: Duration = Duration::from_secs(5);
const CARDINALITY: usize = 4096;

#[derive(Clone)]
pub struct Endpoint(reqwest::Url);

impl Endpoint {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let url = reqwest::Url::parse(value).map_err(|_| "expected an HTTP or HTTPS base URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "expected an HTTP or HTTPS base URL without credentials, query or fragment",
            );
        }
        Ok(Self(url))
    }

    fn export_url(&self, kind: &str) -> String {
        format!("{}/v1/{kind}", self.0.as_str().trim_end_matches('/'))
    }
}

pub struct Telemetry {
    traces: SdkTracerProvider,
    metrics: SdkMeterProvider,
    instruments: Instruments,
}

#[derive(Clone)]
struct Instruments {
    tracer: SdkTracer,
    requests: Counter<u64>,
    duration: Histogram<f64>,
}

impl Telemetry {
    pub async fn start(endpoint: Option<Endpoint>) -> std::io::Result<Option<Self>> {
        let Some(endpoint) = endpoint else {
            return Ok(None);
        };
        tokio::task::spawn_blocking(move || Self::build(endpoint))
            .await
            .map_err(|_| std::io::Error::other("monitoring initialization task failed"))?
            .map(Some)
    }

    fn build(endpoint: Endpoint) -> std::io::Result<Self> {
        let failed =
            |_| std::io::Error::other("JUNTO_OTEL_ENDPOINT: exporter initialization failed");
        let client = reqwest_otel::blocking::Client::builder()
            .timeout(EXPORT_TIMEOUT)
            .redirect(reqwest_otel::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| std::io::Error::other("monitoring HTTP client initialization failed"))?;
        let traces = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .with_endpoint(endpoint.export_url("traces"))
            .with_timeout(EXPORT_TIMEOUT)
            .with_http_client(client.clone())
            .build()
            .map_err(failed)?;
        let metrics = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .with_endpoint(endpoint.export_url("metrics"))
            .with_temporality(opentelemetry_sdk::metrics::Temporality::Cumulative)
            .with_timeout(EXPORT_TIMEOUT)
            .with_http_client(client)
            .build()
            .map_err(failed)?;
        let resource = Resource::builder_empty()
            .with_attribute(KeyValue::new("service.name", "junto-server"))
            .build();
        let batch = BatchSpanProcessor::builder(traces)
            .with_batch_config(
                BatchConfigBuilder::default()
                    .with_max_queue_size(QUEUE_SIZE)
                    .with_max_export_batch_size(BATCH_SIZE)
                    .with_scheduled_delay(EXPORT_INTERVAL)
                    .build(),
            )
            .build();
        let traces = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
            .with_max_attributes_per_span(3)
            .with_max_events_per_span(0)
            .with_max_links_per_span(0)
            .with_span_processor(batch)
            .build();
        let metrics = SdkMeterProvider::builder()
            .with_resource(resource)
            .with_reader(
                PeriodicReader::builder(metrics)
                    .with_interval(EXPORT_INTERVAL)
                    .build(),
            )
            .with_view(|_| {
                Stream::builder()
                    .with_cardinality_limit(CARDINALITY)
                    .build()
                    .ok()
            })
            .build();
        let meter = metrics.meter("junto-server");
        let instruments = Instruments {
            tracer: traces.tracer("junto-server"),
            requests: meter
                .u64_counter("http.server.request.count")
                .with_unit("{request}")
                .build(),
            duration: meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_boundaries(vec![
                    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 10.0,
                ])
                .build(),
        };
        Ok(Self {
            traces,
            metrics,
            instruments,
        })
    }

    pub fn instrument(&self, router: Router) -> Router {
        router.layer(middleware::from_fn_with_state(
            self.instruments.clone(),
            request,
        ))
    }

    pub async fn shutdown(self) {
        let _ = tokio::task::spawn_blocking(move || {
            if self
                .traces
                .shutdown_with_timeout(Duration::from_secs(5))
                .is_err()
            {
                tracing::warn!("some monitoring spans could not be exported at shutdown");
            }
            if self
                .metrics
                .shutdown_with_timeout(Duration::from_secs(5))
                .is_err()
            {
                tracing::warn!("monitoring metrics could not be exported at shutdown");
            }
        })
        .await;
    }
}

async fn request(State(instruments): State<Instruments>, request: Request, next: Next) -> Response {
    let method = match request.method().as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "DELETE" => "DELETE",
        "PATCH" => "PATCH",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        "CONNECT" => "CONNECT",
        "TRACE" => "TRACE",
        _ => "_OTHER",
    };
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let mut attributes = vec![
        KeyValue::new("http.request.method", method),
        KeyValue::new("http.route", route),
    ];
    let mut span = instruments
        .tracer
        .span_builder("HTTP request")
        .with_kind(SpanKind::Server)
        .with_attributes(attributes.clone())
        .start_with_context(&instruments.tracer, &Context::new());
    let started = Instant::now();
    let response = next.run(request).await;
    let status = KeyValue::new(
        "http.response.status_code",
        i64::from(response.status().as_u16()),
    );
    span.set_attribute(status.clone());
    if response.status().is_server_error() {
        span.set_status(Status::error(""));
    }
    attributes.push(status);
    instruments.requests.add(1, &attributes);
    instruments
        .duration
        .record(started.elapsed().as_secs_f64(), &attributes);
    span.end();
    response
}

#[cfg(test)]
mod tests;
