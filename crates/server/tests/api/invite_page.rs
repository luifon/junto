use crate::support::TestServer;
use reqwest::{Method, StatusCode};
use serde_json::Value;

#[tokio::test]
async fn invite_page_shows_the_server_download_and_code_without_redeeming_it() {
    let server = TestServer::start_with(|config| {
        config.server_name = "Original".to_owned();
        config.client_download_url = Some(
            "https://downloads.example.test/?platform=all&channel=stable"
                .parse()
                .unwrap(),
        );
    })
    .await;
    sqlx::query("update server_state set name = $1")
        .bind("Harbor <Team>")
        .execute(&server.db)
        .await
        .unwrap();
    let reply = server
        .send::<Value>(
            Method::GET,
            "/join?code=ABCD-EFGH-JKMN-PQRS",
            None,
            None,
            None,
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.header("referrer-policy"), Some("no-referrer"));
    let html = reply.text();
    assert!(html.contains("Harbor &lt;Team&gt;"));
    assert!(!html.contains("Harbor <Team>"));
    assert!(html.contains("ABCD-EFGH-JKMN-PQRS"));
    assert!(html.contains(
        "junto://invite?server=https%3A%2F%2Fchat.example.test&amp;code=ABCD-EFGH-JKMN-PQRS"
    ));
    assert!(html.contains("https://downloads.example.test/?platform=all&amp;channel=stable"));
    assert!(!html.contains("<script"));
    // No lookup or redemption occurs: syntactically valid unused and missing codes show the same page.
    let again = server
        .send::<Value>(
            Method::GET,
            "/join?code=ABCD-EFGH-JKMN-PQRS",
            None,
            None,
            None,
        )
        .await;
    assert_eq!(reply.bytes(), again.bytes());
    for path in [
        "/join?code=%3Cscript%3E",
        "/join?code=short",
        "/join?code=ABCD-EFGH-JKMN-PQRS&code=ABCD-EFGH-JKMN-PQRS",
    ] {
        let reply = server
            .send::<Value>(Method::GET, path, None, None, None)
            .await;
        assert!(reply.status.is_client_error(), "{path}");
        assert!(!reply.text().contains("<script>"));
    }
}
