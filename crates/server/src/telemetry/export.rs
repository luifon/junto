use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioTimer};
use opentelemetry::Context;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::transform::common::tonic::ResourceAttributesWithSchema;
use opentelemetry_proto::transform::trace::tonic::group_spans_by_resource_and_scope;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::metrics::Temporality;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::trace::{BatchSpanProcessor, Span, SpanData, SpanExporter, SpanProcessor};
use tokio_rustls::rustls::crypto::ring;
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

const REPORT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub(super) struct Queue {
    pending: AtomicUsize,
    dropped: AtomicU64,
}

pub(super) struct Diagnostics {
    dispatch: tracing::Dispatch,
    last_failure: Mutex<Option<Instant>>,
    last_drop: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for Diagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Diagnostics")
    }
}

impl Diagnostics {
    pub(super) fn new(dispatch: tracing::Dispatch) -> Self {
        Self {
            dispatch,
            last_failure: Mutex::new(None),
            last_drop: Mutex::new(None),
        }
    }

    fn due(last: &Mutex<Option<Instant>>, now: Instant) -> bool {
        let Ok(mut last) = last.lock() else {
            return false;
        };
        if last.is_some_and(|last| now.duration_since(last) < REPORT_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }

    fn failure(&self, signal: &'static str) {
        if Self::due(&self.last_failure, Instant::now()) {
            tracing::dispatcher::with_default(&self.dispatch, || {
                tracing::warn!(signal, "monitoring export failed");
            });
        }
    }

    fn drops(&self, queue: &Queue) {
        if queue.dropped.load(Ordering::Relaxed) != 0 && Self::due(&self.last_drop, Instant::now())
        {
            let dropped = queue.dropped.swap(0, Ordering::Relaxed);
            tracing::dispatcher::with_default(&self.dispatch, || {
                tracing::warn!(dropped, "monitoring span queue full");
            });
        }
    }
}

pub(super) const RESPONSE_LIMIT: usize = 64 * 1024;

#[derive(Clone)]
pub(super) struct Transport {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    runtime: Arc<Background>,
}

struct Background(Option<tokio::runtime::Runtime>);

impl Drop for Background {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

impl Transport {
    pub(super) fn new(ca_file: Option<&Path>) -> std::io::Result<Self> {
        let tls = tls_config(ca_file)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .thread_name("monitoring-export")
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| std::io::Error::other("monitoring HTTP client initialization failed"))?;
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .build(connector);
        Ok(Self {
            client,
            runtime: Arc::new(Background(Some(runtime))),
        })
    }
}

fn tls_config(ca_file: Option<&Path>) -> std::io::Result<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    if let Some(path) = ca_file {
        let invalid =
            || std::io::Error::other("JUNTO_OTEL_CA_FILE: expected a readable PEM CA bundle");
        let pem = std::fs::read(path).map_err(|_| invalid())?;
        let mut added = 0;
        for certificate in CertificateDer::pem_slice_iter(&pem) {
            roots
                .add(certificate.map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
            added += 1;
        }
        if added == 0 {
            return Err(invalid());
        }
    }
    Ok(
        ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|_| std::io::Error::other("monitoring TLS initialization failed"))?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

pub(super) struct Http {
    transport: Transport,
    url: hyper::Uri,
    signal: &'static str,
    diagnostics: Diagnostics,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("signal", &self.signal)
            .finish_non_exhaustive()
    }
}

impl Http {
    pub(super) fn new(
        transport: Transport,
        endpoint: &super::Endpoint,
        signal: &'static str,
        dispatch: tracing::Dispatch,
    ) -> std::io::Result<Self> {
        let url = endpoint
            .export_url(signal)
            .parse()
            .map_err(|_| std::io::Error::other("monitoring HTTP client initialization failed"))?;
        Ok(Self {
            transport,
            url,
            signal,
            diagnostics: Diagnostics::new(dispatch),
        })
    }

    fn send(&self, request: &impl serde::Serialize) -> OTelSdkResult {
        let accepted = self.exchange(request).is_some_and(|body| accepted(&body));
        if accepted {
            Ok(())
        } else {
            self.diagnostics.failure(self.signal);
            Err(OTelSdkError::InternalFailure(
                "monitoring export failed".to_owned(),
            ))
        }
    }

    fn exchange(&self, request: &impl serde::Serialize) -> Option<Bytes> {
        let body = serde_json::to_vec(request).ok()?;
        let request = hyper::Request::post(&self.url)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .ok()?;
        let exchange = async {
            let response = self.transport.client.request(request).await.ok()?;
            if !response.status().is_success() {
                return None;
            }
            let body = Limited::new(response.into_body(), RESPONSE_LIMIT)
                .collect()
                .await
                .ok()?;
            Some(body.to_bytes())
        };
        let runtime = self.transport.runtime.0.as_ref()?;
        runtime
            .block_on(async { tokio::time::timeout(super::EXPORT_TIMEOUT, exchange).await })
            .ok()?
    }
}

fn accepted(body: &[u8]) -> bool {
    if body.is_empty() {
        return true;
    }
    let Ok(response) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let partial = &response["partialSuccess"];
    ["rejectedSpans", "rejectedDataPoints"].iter().all(|key| {
        partial[*key]
            .as_u64()
            .or_else(|| partial[*key].as_str()?.parse().ok())
            .unwrap_or(0)
            == 0
    }) && partial["errorMessage"].as_str().is_none_or(str::is_empty)
}

#[derive(Debug)]
pub(super) struct Traces {
    pub(super) http: Http,
    pub(super) queue: Arc<Queue>,
    pub(super) resource: ResourceAttributesWithSchema,
}

impl SpanExporter for Traces {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        self.queue.pending.fetch_sub(batch.len(), Ordering::Relaxed);
        self.http.diagnostics.drops(&self.queue);
        self.http.send(&ExportTraceServiceRequest {
            resource_spans: group_spans_by_resource_and_scope(batch, &self.resource),
        })
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource = resource.into();
    }
}

pub(super) struct Metrics(pub(super) Http);

impl PushMetricExporter for Metrics {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        self.0.send(&ExportMetricsServiceRequest::from(metrics))
    }
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        Ok(())
    }
    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}

#[derive(Debug)]
pub(super) struct BoundedProcessor {
    pub(super) batch: BatchSpanProcessor,
    pub(super) queue: Arc<Queue>,
}

impl SpanProcessor for BoundedProcessor {
    fn on_start(&self, span: &mut Span, context: &Context) {
        self.batch.on_start(span, context);
    }
    fn on_end(&self, span: SpanData) {
        if self
            .queue
            .pending
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |pending| {
                (pending < super::QUEUE_SIZE).then_some(pending + 1)
            })
            .is_ok()
        {
            self.batch.on_end(span);
        } else {
            self.queue.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.batch.force_flush()
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.batch.shutdown_with_timeout(timeout)
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.batch.set_resource(resource);
    }
}
