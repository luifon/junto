use std::sync::Arc;

use axum::Router;
use axum::routing::post;
use junto_server::telemetry::Endpoint;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use super::support::TestServer;

#[tokio::test]
async fn telemetry_exports_requests_from_the_running_server_and_flushes_at_stop() {
    let exports = Arc::new(Mutex::new(Vec::new()));
    let collector = Router::new().route(
        "/v1/{signal}",
        post({
            let exports = exports.clone();
            move |axum::Json(body): axum::Json<Value>| {
                let exports = exports.clone();
                async move {
                    exports.lock().await.push(body);
                    axum::Json(serde_json::json!({}))
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let collector = tokio::spawn(async move { axum::serve(listener, collector).await.unwrap() });
    let mut server = TestServer::start_with(|config| config.otel_endpoint = Some(endpoint)).await;
    let code = server.owner_code.clone();
    assert_eq!(
        reqwest::get(format!("{}/api/server", server.base))
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    server.stop().await;
    collector.abort();
    let exports = exports.lock().await;
    let text = serde_json::to_string(&*exports).unwrap();
    assert!(!text.contains(&code));
    assert!(
        exports
            .iter()
            .any(|body| body.get("resourceSpans").is_some())
    );
    assert!(
        exports
            .iter()
            .any(|body| body.get("resourceMetrics").is_some())
    );
    assert!(text.contains("/api/server"));
    assert!(text.contains("http.server.request.duration"));
}
