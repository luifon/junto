//! Opt-in request monitoring with a fixed set of attributes (ADR 0007).

use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use opentelemetry::metrics::{Counter, Histogram, MeterProvider};
use opentelemetry::trace::{Span, SpanKind, Status, Tracer, TracerProvider};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Stream};
use opentelemetry_sdk::trace::{
    BatchConfigBuilder, BatchSpanProcessor, SdkTracer, SdkTracerProvider,
};

mod export;

const QUEUE_SIZE: usize = 1024;
const BATCH_SIZE: usize = 256;
const EXPORT_TIMEOUT: Duration = Duration::from_secs(2);
const EXPORT_INTERVAL: Duration = Duration::from_secs(5);
const CARDINALITY: usize = 4096;

#[derive(Clone)]
pub struct Endpoint {
    url: reqwest::Url,
    ca_file: Option<std::path::PathBuf>,
}

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
        Ok(Self { url, ca_file: None })
    }

    pub fn with_ca_file(mut self, path: std::path::PathBuf) -> Self {
        self.ca_file = Some(path);
        self
    }

    fn export_url(&self, kind: &str) -> String {
        format!("{}/v1/{kind}", self.url.as_str().trim_end_matches('/'))
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
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        tokio::task::spawn_blocking(move || Self::build(endpoint, dispatch))
            .await
            .map_err(|_| std::io::Error::other("monitoring initialization task failed"))?
            .map(Some)
    }

    fn build(endpoint: Endpoint, dispatch: tracing::Dispatch) -> std::io::Result<Self> {
        let mut builder = reqwest_otel::blocking::Client::builder()
            .timeout(EXPORT_TIMEOUT)
            .redirect(reqwest_otel::redirect::Policy::none())
            .no_proxy();
        if let Some(path) = &endpoint.ca_file {
            let invalid =
                || std::io::Error::other("JUNTO_OTEL_CA_FILE: expected a readable PEM CA bundle");
            let pem = std::fs::read(path).map_err(|_| invalid())?;
            let certificates =
                reqwest_otel::Certificate::from_pem_bundle(&pem).map_err(|_| invalid())?;
            if certificates.is_empty() {
                return Err(invalid());
            }
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let client = builder.build().map_err(|_| {
            std::io::Error::other(if endpoint.ca_file.is_some() {
                "monitoring HTTP client initialization failed with JUNTO_OTEL_CA_FILE configured"
            } else {
                "monitoring HTTP client initialization failed"
            })
        })?;
        let queue = std::sync::Arc::new(export::Queue::default());
        let traces = export::Traces {
            http: export::Http::new(client.clone(), &endpoint, "traces", dispatch.clone()),
            queue: queue.clone(),
            resource: Default::default(),
        };
        let metrics = export::Metrics(export::Http::new(client, &endpoint, "metrics", dispatch));
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
            .with_span_processor(export::BoundedProcessor { batch, queue })
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
    let attributes = vec![
        KeyValue::new("http.request.method", method),
        KeyValue::new("http.route", route),
    ];
    let span = instruments
        .tracer
        .span_builder("HTTP request")
        .with_kind(SpanKind::Server)
        .with_attributes(attributes.clone())
        .start_with_context(&instruments.tracer, &Context::new());
    let mut observation = RequestObservation {
        span,
        instruments,
        attributes,
        started: Instant::now(),
        status: None,
    };
    let response = next.run(request).await;
    observation.status = Some(response.status());
    response
}

struct RequestObservation {
    span: opentelemetry_sdk::trace::Span,
    instruments: Instruments,
    attributes: Vec<KeyValue>,
    started: Instant,
    status: Option<axum::http::StatusCode>,
}

impl Drop for RequestObservation {
    fn drop(&mut self) {
        let attribute = if let Some(status) = self.status {
            if status.is_server_error() {
                self.span.set_status(Status::error(""));
            }
            KeyValue::new("http.response.status_code", i64::from(status.as_u16()))
        } else {
            self.span.set_status(Status::error(""));
            KeyValue::new("error.type", "cancelled")
        };
        self.span.set_attribute(attribute.clone());
        self.attributes.push(attribute);
        self.instruments.requests.add(1, &self.attributes);
        self.instruments
            .duration
            .record(self.started.elapsed().as_secs_f64(), &self.attributes);
        self.span.end();
    }
}

#[cfg(test)]
mod tests;
