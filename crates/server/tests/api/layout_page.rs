//! The layout page of the recorder, served from the built directory (ADR 0615).

use junto_protocol::error::ErrorCode;
use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::support::{Reply, Scratch, TestServer};

const PAGE: &str = "<!doctype html><title>layout</title><script src=\"layout.js\"></script>";

/// A directory with the files that the build of `recording-layout` writes, and beside it a
/// file that is no part of the page.
fn built() -> (Scratch, std::path::PathBuf) {
    let scratch = Scratch::new();
    let directory = scratch.0.join("dist");
    std::fs::create_dir_all(directory.join("fonts")).unwrap();
    for (name, content) in [
        ("index.html", PAGE.as_bytes()),
        ("layout.js", b"export const layout = 1;".as_slice()),
        ("theme.css", b"body { margin: 0 }".as_slice()),
        (
            "livekit-client.umd.js",
            b"// licence notices\nvar LivekitClient = {};".as_slice(),
        ),
        ("livekit-client.LICENSE", b"Apache License".as_slice()),
        ("atkinson.woff2", [0x77, 0x4f, 0x46, 0x32, 0, 1].as_slice()),
        ("fonts/extra.woff2", [0x77, 0x4f, 0x46, 0x32].as_slice()),
        ("notes.md", b"not a file of the page".as_slice()),
        (".hidden.js", b"hidden".as_slice()),
    ] {
        std::fs::write(directory.join(name), content).unwrap();
    }
    std::fs::write(scratch.0.join("secret.txt"), "outside the page").unwrap();
    std::fs::write(scratch.0.join("secret.js"), "outside the page").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        // A link to a file outside, a link to a file of the page, and a linked directory.
        symlink(scratch.0.join("secret.js"), directory.join("linked.js")).unwrap();
        symlink(directory.join("layout.js"), directory.join("alias.js")).unwrap();
        symlink(directory.join("fonts"), directory.join("linked-fonts")).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(directory.join("pipe.js"))
            .status()
            .unwrap();
        assert!(made.success());
    }
    (scratch, directory)
}

async fn fetch(server: &TestServer, method: Method, path: &str) -> Reply {
    server.send::<Value>(method, path, None, None, None).await
}

#[tokio::test]
async fn rule_60_62_the_page_and_its_files_are_served_without_a_credential() {
    let (_scratch, directory) = built();
    let server =
        TestServer::start_with(|config| config.recorder_layout_directory = Some(directory)).await;
    let cases: [(&str, &str, &[u8]); 7] = [
        (
            "/recording-layout/",
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        ),
        (
            "/recording-layout/index.html",
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        ),
        (
            "/recording-layout/layout.js",
            "text/javascript; charset=utf-8",
            b"export const layout = 1;",
        ),
        (
            "/recording-layout/theme.css",
            "text/css; charset=utf-8",
            b"body { margin: 0 }",
        ),
        (
            "/recording-layout/livekit-client.LICENSE",
            "text/plain; charset=utf-8",
            b"Apache License",
        ),
        (
            "/recording-layout/atkinson.woff2",
            "font/woff2",
            &[0x77, 0x4f, 0x46, 0x32, 0, 1],
        ),
        (
            "/recording-layout/fonts/extra.woff2",
            "font/woff2",
            &[0x77, 0x4f, 0x46, 0x32],
        ),
    ];
    for (path, media_type, content) in cases {
        let reply = fetch(&server, Method::GET, path).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}: {}", reply.text());
        assert_eq!(reply.header("content-type"), Some(media_type), "{path}");
        assert_eq!(
            reply.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(reply.bytes(), content, "{path}");
    }
    // The recorder adds the address of the media server, its token and the layout.
    let with_query = "/recording-layout/?url=ws%3A%2F%2Fmedia%3A7880&token=a.b.c&layout=";
    let reply = fetch(&server, Method::GET, with_query).await;
    assert_eq!(reply.bytes(), PAGE.as_bytes());
    // A credential changes nothing, also one that no session has.
    let reply = server
        .send::<Value>(
            Method::GET,
            "/recording-layout/",
            Some("jst_unknown"),
            None,
            None,
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);

    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        fetch(&server, method, "/recording-layout/layout.js")
            .await
            .error(ErrorCode::MethodNotAllowed);
    }
}

#[tokio::test]
async fn rule_61_only_files_of_the_page_are_served() {
    let (_scratch, directory) = built();
    let server =
        TestServer::start_with(|config| config.recorder_layout_directory = Some(directory)).await;
    let absent = fetch(&server, Method::GET, "/recording-layout/absent.js").await;
    absent.error(ErrorCode::NotFound);
    for path in [
        "/recording-layout/%2e%2e/secret.txt",
        "/recording-layout/%2e%2e%2fsecret.js",
        "/recording-layout/..%2Fsecret.js",
        "/recording-layout/fonts/%2e%2e/%2e%2e/secret.js",
        "/recording-layout/fonts%2F..%2F..%2Fsecret.js",
        "/recording-layout/%2Fetc%2Fhosts",
        "/recording-layout//layout.js",
        "/recording-layout/.hidden.js",
        "/recording-layout/notes.md",
        "/recording-layout/fonts",
        "/recording-layout/fonts/",
        "/recording-layout/index.html%00.js",
        "/recording-layout/linked.js",
        "/recording-layout/alias.js",
        "/recording-layout/linked-fonts/extra.woff2",
        "/recording-layout/pipe.js",
        "/recording-layout",
    ] {
        let reply = fetch(&server, Method::GET, path).await;
        assert_eq!(reply.observable(), absent.observable(), "{path}");
        assert!(!reply.text().contains("outside the page"), "{path}");
    }
}

#[tokio::test]
async fn rule_62_a_server_without_the_directory_serves_no_page() {
    let server = TestServer::start().await;
    let absent = fetch(&server, Method::GET, "/api/v1/absent").await;
    absent.error(ErrorCode::NotFound);
    for path in ["/recording-layout/", "/recording-layout/index.html"] {
        let reply = fetch(&server, Method::GET, path).await;
        assert_eq!(reply.observable(), absent.observable(), "{path}");
    }
}

#[test]
fn rule_62_the_setting_names_a_directory_with_the_built_page() {
    use junto_server::config::Config;
    let (_scratch, directory) = built();
    let base = [(
        "JUNTO_DATABASE_URL",
        "postgres://localhost/junto".to_owned(),
    )];
    let read = |value: Option<String>| {
        let mut values = base.to_vec();
        if let Some(value) = value {
            values.push(("JUNTO_RECORDER_LAYOUT_DIR", value));
        }
        Config::from_lookup(|name| {
            values
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, value)| value.clone())
        })
    };
    assert_eq!(read(None).unwrap().recorder_layout_directory, None);
    let path = directory.to_str().unwrap().to_owned();
    assert_eq!(
        read(Some(path)).unwrap().recorder_layout_directory,
        Some(directory.clone())
    );
    let without_page = directory.join("fonts").to_str().unwrap().to_owned();
    for wrong in [
        without_page,
        "recording-layout/dist".to_owned(),
        "/nonexistent/dist".to_owned(),
    ] {
        let error = read(Some(wrong.clone())).err().unwrap().to_string();
        assert!(
            error.contains("JUNTO_RECORDER_LAYOUT_DIR"),
            "{wrong}: {error}"
        );
    }
}
