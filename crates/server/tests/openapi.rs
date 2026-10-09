use std::path::Path;

#[test]
fn committed_openapi_document_matches_the_routes() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(junto_server::OPENAPI_DOCUMENT_PATH);
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let generated = junto_server::openapi_document().unwrap();
    assert!(
        committed == generated,
        "{} differs from the routes. Run `cargo run -p junto-server --bin junto-openapi` and \
         commit the result.",
        path.display()
    );
}

#[test]
fn every_operation_documents_a_success_and_the_error_body() {
    let document: serde_json::Value =
        serde_json::from_str(&junto_server::openapi_document().unwrap()).unwrap();
    let paths = document["paths"].as_object().unwrap();
    assert!(!paths.is_empty());
    for (path, operations) in paths {
        assert!(
            path == "/api/server" || path.starts_with(&format!("{}/", junto_protocol::API_PREFIX)),
            "{path} is outside the version prefix"
        );
        for (method, operation) in operations.as_object().unwrap() {
            let responses = operation["responses"].as_object().unwrap();
            assert!(
                responses.keys().any(|status| status.starts_with('2')),
                "{method} {path} documents no success"
            );
            assert_eq!(
                responses["default"]["content"]["application/json"]["schema"]["$ref"],
                "#/components/schemas/ErrorBody",
                "{method} {path}"
            );
        }
    }
}

#[test]
fn routes_without_a_session_are_the_ones_a_device_uses_before_it_has_one() {
    let document: serde_json::Value =
        serde_json::from_str(&junto_server::openapi_document().unwrap()).unwrap();
    let mut open = Vec::new();
    for (path, operations) in document["paths"].as_object().unwrap() {
        for (method, operation) in operations.as_object().unwrap() {
            if operation.get("security").is_none() {
                open.push(format!("{method} {path}"));
            }
        }
    }
    open.sort();
    assert_eq!(
        open,
        [
            "get /api/server",
            "post /api/v1/auth/challenges",
            "post /api/v1/auth/sessions",
            "post /api/v1/device-links",
            "post /api/v1/join",
            "post /api/v1/join-requests",
            "post /api/v1/join-requests/status",
            "post /api/v1/owner/claim",
        ]
    );
}
