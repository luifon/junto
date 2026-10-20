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
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}

impl Collector {
    async fn start(blocked: bool) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let app = Router::new()
            .route(
                "/v1/{signal}",
                post({
                    let requests = requests.clone();
                    let entered = entered.clone();
                    let release = release.clone();
                    move |axum::extract::Path(signal): axum::extract::Path<String>, body: Bytes| {
                        let requests = requests.clone();
                        let entered = entered.clone();
                        let release = release.clone();
                        async move {
                            requests
                                .lock()
                                .await
                                .push((signal, serde_json::from_slice(&body).unwrap()));
                            entered.notify_one();
                            if blocked {
                                let _ = release.acquire().await;
                            }
                            axum::Json(json!({}))
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
    let duration = metrics
        .iter()
        .find(|metric| metric["name"] == "http.server.request.duration")
        .unwrap();
    assert_eq!(duration["unit"], "s");
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
    telemetry.shutdown().await;
    let exports = collector.requests.lock().await;
    let points: Vec<_> = exports
        .iter()
        .filter(|(signal, _)| signal == "metrics")
        .flat_map(|(_, request)| request["resourceMetrics"].as_array().unwrap())
        .flat_map(|resource| resource["scopeMetrics"].as_array().unwrap())
        .flat_map(|scope| scope["metrics"].as_array().unwrap())
        .filter(|metric| metric["name"] == "http.server.request.count")
        .flat_map(|metric| metric["sum"]["dataPoints"].as_array().unwrap())
        .collect();
    assert!(points.len() <= CARDINALITY + 1);
    assert!(points.iter().any(|point| {
        attributes(&point["attributes"]).get("otel.metric.overflow")
            == Some(&json!({"boolValue": true}))
    }));
    assert_eq!(
        points.iter().map(|p| integer(&p["asInt"])).sum::<u64>(),
        (CARDINALITY + 100) as u64
    );
}

#[tokio::test]
async fn telemetry_requests_continue_while_a_metric_export_is_blocked() {
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
    flushing.await.unwrap().unwrap();
    telemetry.shutdown().await;
}

#[tokio::test]
async fn telemetry_drops_spans_when_a_collector_stops_reading() {
    let collector = Collector::start(true).await;
    let telemetry = runtime(&collector).await;
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
