use std::io::Read;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

pub(super) struct Http {
    client: reqwest_otel::blocking::Client,
    url: String,
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
        client: reqwest_otel::blocking::Client,
        endpoint: &super::Endpoint,
        signal: &'static str,
        dispatch: tracing::Dispatch,
    ) -> Self {
        Self {
            client,
            url: endpoint.export_url(signal),
            signal,
            diagnostics: Diagnostics::new(dispatch),
        }
    }

    fn send(&self, request: &impl serde::Serialize) -> OTelSdkResult {
        let result = serde_json::to_vec(request).ok().and_then(|body| {
            self.client
                .post(&self.url)
                .header(reqwest_otel::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .ok()
        });
        let accepted = result.is_some_and(|response| {
            if !response.status().is_success() {
                return false;
            }
            let mut body = Vec::new();
            if response.take(65537).read_to_end(&mut body).is_err() || body.len() > 65536 {
                return false;
            }
            if body.is_empty() {
                return true;
            }
            let Ok(response) = serde_json::from_slice::<serde_json::Value>(&body) else {
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
        });
        if accepted {
            Ok(())
        } else {
            self.diagnostics.failure(self.signal);
            Err(OTelSdkError::InternalFailure(
                "monitoring export failed".to_owned(),
            ))
        }
    }
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
