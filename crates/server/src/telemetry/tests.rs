#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use axum::body::Bytes;
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, Semaphore};

#[test]
fn telemetry_endpoint_has_no_credentials_query_or_fragment() {
    for value in [
        "not a URL",
        "file:///collector",
        "ftp://collector.example.test",
        "https://secret@collector.example.test",
        "https://collector.example.test?secret=key",
        "https://collector.example.test/#secret",
    ] {
        assert!(Endpoint::parse(value).is_err());
    }
    for base in [
        "https://collector.example.test/prefix",
        "https://collector.example.test/prefix/",
    ] {
        assert_eq!(
            Endpoint::parse(base).unwrap().export_url("traces"),
            "https://collector.example.test/prefix/v1/traces"
        );
    }
}

struct Collector {
    endpoint: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    headers: Arc<Mutex<Vec<axum::http::HeaderMap>>>,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}

impl Collector {
    async fn start(blocked: bool) -> Self {
        Self::with_status(blocked, StatusCode::OK).await
    }

    async fn with_status(blocked: bool, status: StatusCode) -> Self {
        Self::with_response(blocked, status, json!({"error": "secret-response-body"})).await
    }

    async fn with_response(blocked: bool, status: StatusCode, reply: Value) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let headers = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let app = Router::new()
            .route(
                "/v1/{signal}",
                post({
                    let requests = requests.clone();
                    let headers = headers.clone();
                    let entered = entered.clone();
                    let release = release.clone();
                    move |axum::extract::Path(signal): axum::extract::Path<String>,
                          incoming: axum::http::HeaderMap,
                          body: Bytes| {
                        let reply = reply.clone();
                        let headers = headers.clone();
                        let requests = requests.clone();
                        let entered = entered.clone();
                        let release = release.clone();
                        async move {
                            headers.lock().await.push(incoming);
                            requests
                                .lock()
                                .await
                                .push((signal, serde_json::from_slice(&body).unwrap()));
                            entered.notify_one();
                            if blocked {
                                let _ = release.acquire().await;
                            }
                            (status, axum::Json(reply))
                        }
                    }
                }),
            )
            .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            endpoint,
            requests,
            headers,
            entered,
            release,
            task,
        }
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.release.close();
        self.task.abort();
    }
}

async fn runtime(collector: &Collector) -> Telemetry {
    let endpoint = Endpoint::parse(&collector.endpoint).unwrap();
    Telemetry::start(Some(endpoint)).await.unwrap().unwrap()
}

async fn application(telemetry: Option<&Telemetry>) -> (String, tokio::task::JoinHandle<()>) {
    let router = Router::new()
        .route(
            "/messages/{message_id}",
            post(|| async { StatusCode::CREATED }),
        )
        .route(
            "/failure",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        );
    let router = match telemetry {
        Some(telemetry) => telemetry.instrument(router),
        None => router,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (address, task)
}

fn attributes(value: &Value) -> std::collections::BTreeMap<String, Value> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|attribute| {
            (
                attribute["key"].as_str().unwrap().to_owned(),
                attribute["value"].clone(),
            )
        })
        .collect()
}

fn integer(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap()
}

#[tokio::test]
async fn telemetry_exports_only_request_metadata_and_aggregated_metrics() {
    let collector = Collector::start(false).await;
    let telemetry = runtime(&collector).await;
    let (address, task) = application(Some(&telemetry)).await;
    let client = reqwest::Client::new();
    for id in ["secret-message-one", "secret-message-two"] {
        assert_eq!(
            client
                .post(format!("{address}/messages/{id}?key=secret-query"))
                .header("authorization", "Bearer secret-token")
                .header("baggage", "name=secret-name")
                .header(
                    "traceparent",
                    "00-11111111111111111111111111111111-2222222222222222-01",
                )
                .body("secret-message-content")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let metrics = telemetry.metrics.clone();
        tokio::task::spawn_blocking(move || metrics.force_flush())
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(
        client
            .get(format!("{address}/secret-unknown"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .request(
                reqwest::Method::from_bytes(b"SECRET-METHOD").unwrap(),
                format!("{address}/failure")
            )
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        client
            .get(format!("{address}/failure"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    task.abort();
    telemetry.shutdown().await;
    let exports = collector.requests.lock().await;
    let text = serde_json::to_string(&*exports).unwrap();
    assert!(!text.contains("secret"), "{text}");
    assert!(!text.contains("SECRET"), "{text}");
    let mut spans = Vec::new();
    let mut metrics = Vec::new();
    for (signal, request) in exports.iter() {
        if signal == "metrics" {
            metrics.clear();
        }
        let (resource_key, scope_key, item_key) = match signal.as_str() {
            "traces" => ("resourceSpans", "scopeSpans", "spans"),
            "metrics" => ("resourceMetrics", "scopeMetrics", "metrics"),
            other => panic!("unexpected signal: {other}"),
        };
        for resource in request[resource_key].as_array().unwrap() {
            assert_eq!(
                attributes(&resource["resource"]["attributes"]),
                std::collections::BTreeMap::from([(
                    "service.name".to_owned(),
                    json!({"stringValue": "junto-server"})
                ),])
            );
            for scope in resource[scope_key].as_array().unwrap() {
                for item in scope[item_key].as_array().unwrap() {
                    if signal == "traces" {
                        spans.push(item);
                    } else {
                        metrics.push(item);
                    }
                }
            }
        }
    }
    assert_eq!(spans.len(), 5);
    for span in &spans {
        let attrs = attributes(&span["attributes"]);
        assert_eq!(
            attrs.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "http.request.method",
                "http.response.status_code",
                "http.route"
            ]
        );
        assert_eq!(span["kind"], 2);
        assert_ne!(span["traceId"], "00000000000000000000000000000000");
        assert_ne!(span["traceId"], "11111111111111111111111111111111");
        assert_eq!(span["parentSpanId"], "");
        assert!(span["events"].as_array().is_none_or(Vec::is_empty));
    }
    let failed = spans
        .iter()
        .find(|span| {
            integer(&attributes(&span["attributes"])["http.response.status_code"]["intValue"])
                == 500
        })
        .unwrap();
    assert_eq!(failed["status"]["code"], 2);
    let duration = metrics
        .iter()
        .find(|metric| metric["name"] == "http.server.request.duration")
        .unwrap();
    assert_eq!(duration["unit"], "s");
    for point in duration["histogram"]["dataPoints"].as_array().unwrap() {
        assert_eq!(
            point["explicitBounds"],
            json!([
                0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 10.0
            ])
        );
    }
    let count = metrics
        .iter()
        .find(|metric| metric["name"] == "http.server.request.count")
        .unwrap();
    let points = count["sum"]["dataPoints"].as_array().unwrap();
    assert_eq!(points.iter().map(|p| integer(&p["asInt"])).sum::<u64>(), 5);
    let messages = points
        .iter()
        .find(|p| {
            attributes(&p["attributes"])["http.route"]["stringValue"] == "/messages/{message_id}"
        })
        .unwrap();
    assert_eq!(integer(&messages["asInt"]), 2);
    assert_eq!(
        duration["histogram"]["dataPoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| integer(&p["count"]))
            .sum::<u64>(),
        5
    );
}

#[tokio::test]
async fn telemetry_is_off_without_an_endpoint() {
    assert!(Telemetry::start(None).await.unwrap().is_none());
}

#[tokio::test]
async fn telemetry_bounds_the_number_of_metric_attribute_combinations() {
    let collector = Collector::start(false).await;
    let telemetry = runtime(&collector).await;
    for index in 0..CARDINALITY + 100 {
        telemetry
            .instruments
            .requests
            .add(1, &[KeyValue::new("http.route", format!("/route-{index}"))]);
    }
    let metrics = telemetry.metrics.clone();
    tokio::task::spawn_blocking(move || metrics.force_flush())
        .await
        .unwrap()
        .unwrap();
    telemetry.shutdown().await;
    let exports = collector.requests.lock().await;
    let points: Vec<_> = exports
        .iter()
        .rfind(|(signal, _)| signal == "metrics")
        .into_iter()
        .flat_map(|(_, request)| request["resourceMetrics"].as_array().unwrap())
        .flat_map(|resource| resource["scopeMetrics"].as_array().unwrap())
        .flat_map(|scope| scope["metrics"].as_array().unwrap())
        .filter(|metric| metric["name"] == "http.server.request.count")
        .flat_map(|metric| metric["sum"]["dataPoints"].as_array().unwrap())
        .collect();
    assert_eq!(points.len(), CARDINALITY + 1);
    assert!(points.iter().any(|point| {
        attributes(&point["attributes"]).get("otel.metric.overflow")
            == Some(&json!({"boolValue": true}))
    }));
    assert_eq!(
        points.iter().map(|p| integer(&p["asInt"])).sum::<u64>(),
        (CARDINALITY + 100) as u64
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telemetry_requests_continue_while_a_metric_export_is_blocked() {
    if bounded_child("telemetry_requests_continue_while_a_metric_export_is_blocked").await {
        return;
    }
    let collector = Collector::start(true).await;
    let telemetry = runtime(&collector).await;
    telemetry.instruments.requests.add(1, &[]);
    let metrics = telemetry.metrics.clone();
    let flushing = tokio::task::spawn_blocking(move || metrics.force_flush());
    tokio::time::timeout(Duration::from_secs(5), collector.entered.notified())
        .await
        .unwrap();
    let (address, task) = application(Some(&telemetry)).await;
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        reqwest::get(format!("{address}/failure")),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    task.abort();
    collector.release.close();
    let _ = flushing.await.unwrap();
    telemetry.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telemetry_drops_spans_when_a_collector_stops_reading() {
    if bounded_child("telemetry_drops_spans_when_a_collector_stops_reading").await {
        return;
    }
    use tracing::instrument::WithSubscriber;
    let logs = LogCapture::default();
    let collector = Collector::start(true).await;
    let telemetry = runtime(&collector).with_subscriber(logs.subscriber()).await;
    let instruments = telemetry.instruments.clone();
    for _ in 0..BATCH_SIZE {
        instruments.tracer.start("request").end();
    }
    tokio::time::timeout(Duration::from_secs(5), collector.entered.notified())
        .await
        .unwrap();
    let (address, task) = application(Some(&telemetry)).await;
    let client = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        for _ in 0..QUEUE_SIZE * 2 {
            instruments.tracer.start("request").end();
        }
        assert_eq!(
            client
                .get(format!("{address}/failure"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    })
    .await
    .unwrap();
    task.abort();
    collector.release.close();
    telemetry.shutdown().await;
    assert_eq!(logs.text().matches("monitoring span queue full").count(), 1);
    let exports = collector.requests.lock().await;
    let count: usize = exports
        .iter()
        .filter(|(signal, _)| signal == "traces")
        .flat_map(|(_, request)| request["resourceSpans"].as_array().unwrap())
        .flat_map(|resource| resource["scopeSpans"].as_array().unwrap())
        .map(|scope| scope["spans"].as_array().unwrap().len())
        .sum();
    assert!(
        count < BATCH_SIZE + QUEUE_SIZE * 2 + 1,
        "a full queue must drop spans"
    );
}

#[tokio::test]
async fn telemetry_ignores_standard_exporter_environment() {
    const CHILD: &str = "JUNTO_TEST_OTEL_ENV_CHILD";
    if std::env::var_os(CHILD).is_none() {
        for compression in [None, Some("gzip"), Some("zstd")] {
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "telemetry::tests::telemetry_ignores_standard_exporter_environment",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env(
                    "OTEL_EXPORTER_OTLP_HEADERS",
                    "authorization=secret-general-header",
                )
                .env(
                    "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
                    "x-secret=secret-trace-header",
                )
                .env(
                    "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
                    "x-secret=secret-metric-header",
                )
                .env(
                    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                    "http://invalid.invalid",
                )
                .env(
                    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
                    "http://invalid.invalid",
                )
                .env("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc")
                .env("OTEL_EXPORTER_OTLP_TIMEOUT", "0")
                .env(
                    "OTEL_EXPORTER_OTLP_CERTIFICATE",
                    "missing-secret-certificate.pem",
                )
                .env("OTEL_RESOURCE_ATTRIBUTES", "secret=secret-resource")
                .env("OTEL_SERVICE_NAME", "secret-service")
                .env("OTEL_TRACES_SAMPLER", "always_off")
                .kill_on_drop(true);
            for key in [
                "OTEL_EXPORTER_OTLP_COMPRESSION",
                "OTEL_EXPORTER_OTLP_TRACES_COMPRESSION",
                "OTEL_EXPORTER_OTLP_METRICS_COMPRESSION",
            ] {
                if let Some(value) = compression {
                    command.env(key, value);
                } else {
                    command.env_remove(key);
                }
            }
            run_child(command).await;
        }
        return;
    }
    let collector = Collector::start(false).await;
    let telemetry = runtime(&collector).await;
    telemetry.instruments.tracer.start("request").end();
    telemetry.instruments.requests.add(1, &[]);
    telemetry.shutdown().await;
    assert_both_signals(&collector.requests.lock().await);
    assert!(
        !serde_json::to_string(&*collector.requests.lock().await)
            .unwrap()
            .contains("secret")
    );
    for headers in collector.headers.lock().await.iter() {
        assert!(!headers.contains_key("authorization"));
        assert!(!headers.contains_key("x-secret"));
        assert!(!headers.contains_key("content-encoding"));
    }
}

#[tokio::test]
async fn telemetry_counts_cancelled_requests() {
    use tower::ServiceExt;
    let collector = Collector::start(false).await;
    let telemetry = runtime(&collector).await;
    let entered = Arc::new(Notify::new());
    let router = telemetry.instrument(Router::new().route(
        "/cancel",
        get({
            let entered = entered.clone();
            move || {
                let entered = entered.clone();
                async move {
                    entered.notify_one();
                    std::future::pending::<StatusCode>().await
                }
            }
        }),
    ));
    let request = tokio::spawn(
        router.oneshot(
            Request::builder()
                .uri("/cancel")
                .body(axum::body::Body::empty())
                .unwrap(),
        ),
    );
    entered.notified().await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    telemetry.shutdown().await;
    let exports = collector.requests.lock().await;
    let span = &exports
        .iter()
        .find(|(signal, _)| signal == "traces")
        .unwrap()
        .1["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
    assert_eq!(span["status"]["code"], 2);
    let attrs = attributes(&span["attributes"]);
    assert_eq!(attrs["error.type"]["stringValue"], "cancelled");
    assert!(!attrs.contains_key("http.response.status_code"));
    let metrics = &exports
        .iter()
        .rfind(|(signal, _)| signal == "metrics")
        .unwrap()
        .1["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
    for metric in metrics.as_array().unwrap() {
        let point = if metric["name"] == "http.server.request.count" {
            let point = &metric["sum"]["dataPoints"][0];
            assert_eq!(integer(&point["asInt"]), 1);
            point
        } else {
            let point = &metric["histogram"]["dataPoints"][0];
            assert_eq!(integer(&point["count"]), 1);
            point
        };
        assert_eq!(
            attributes(&point["attributes"])["error.type"]["stringValue"],
            "cancelled"
        );
    }
}

#[derive(Clone, Default)]
struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl LogCapture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let capture = self.clone();
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || capture.clone())
            .finish()
    }
}

#[tokio::test]
async fn telemetry_reports_export_failures_without_collector_data() {
    use tracing::instrument::WithSubscriber;
    let logs = LogCapture::default();
    async {
        let collector = Collector::with_status(false, StatusCode::BAD_REQUEST).await;
        let telemetry = runtime(&collector).await;
        for _ in 0..3 {
            telemetry.instruments.tracer.start("request").end();
            telemetry.instruments.requests.add(1, &[]);
            let traces = telemetry.traces.clone();
            let metrics = telemetry.metrics.clone();
            tokio::task::spawn_blocking(move || {
                assert!(traces.force_flush().is_err());
                assert!(metrics.force_flush().is_err());
            })
            .await
            .unwrap();
        }
        let text = logs.text();
        assert_eq!(
            text.matches("monitoring export failed").count(),
            2,
            "{text}"
        );
        assert!(text.contains("traces"));
        assert!(text.contains("metrics"));
        assert!(!text.contains(&collector.endpoint));
        assert!(!text.contains("secret-response-body"));
        telemetry.shutdown().await;
    }
    .with_subscriber(logs.subscriber())
    .await;
}

async fn bounded_child(name: &str) -> bool {
    const CHILD: &str = "JUNTO_TEST_OTEL_BLOCK_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            &format!("telemetry::tests::{name}"),
            "--nocapture",
        ])
        .env(CHILD, "1")
        .kill_on_drop(true);
    run_child(command).await;
    true
}

async fn run_child(mut command: tokio::process::Command) {
    let mut child = command.spawn().unwrap();
    match tokio::time::timeout(Duration::from_secs(20), child.wait()).await {
        Ok(status) => assert!(status.unwrap().success(), "exporter subprocess failed"),
        Err(_) => {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
            panic!("exporter subprocess exceeded its execution bound");
        }
    }
}

#[tokio::test]
async fn telemetry_reports_collector_partial_rejections() {
    use tracing::instrument::WithSubscriber;
    let logs = LogCapture::default();
    let collector = Collector::with_response(false, StatusCode::OK, json!({
        "partialSuccess": {"rejectedSpans": "1", "rejectedDataPoints": "1", "errorMessage": "secret-rejection"}
    })).await;
    let telemetry = runtime(&collector).with_subscriber(logs.subscriber()).await;
    telemetry.instruments.tracer.start("request").end();
    telemetry.instruments.requests.add(1, &[]);
    telemetry.shutdown().await;
    let text = logs.text();
    assert_eq!(text.matches("monitoring export failed").count(), 2);
    assert!(!text.contains("secret-rejection"));
    assert!(!text.contains(&collector.endpoint));
}

#[tokio::test]
async fn telemetry_disabled_keeps_http_clients_usable() {
    if bounded_child("telemetry_disabled_keeps_http_clients_usable").await {
        return;
    }
    assert!(Telemetry::start(None).await.unwrap().is_none());
    let collector = Collector::start(false).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/v1/probe", collector.endpoint))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

fn assert_both_signals(exports: &[(String, Value)]) {
    for expected in ["traces", "metrics"] {
        assert!(exports.iter().any(|(signal, _)| signal == expected));
    }
}

#[path = "transport_tests.rs"]
mod transport;
