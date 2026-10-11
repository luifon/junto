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
fn committed_event_stream_document_matches_the_event_types() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(junto_server::EVENTS_DOCUMENT_PATH);
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let generated = junto_server::events_document().unwrap();
    assert!(
        committed == generated,
        "{} differs from the event types. Run `cargo run -p junto-server --bin junto-openapi` \
         and commit the result.",
        path.display()
    );
}

#[test]
fn committed_structure_file_schema_matches_the_file_types() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(junto_server::STRUCTURE_FILE_DOCUMENT_PATH);
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let generated = junto_server::structure_file_document().unwrap();
    assert!(
        committed == generated,
        "{} differs from the file types. Run `cargo run -p junto-server --bin junto-openapi` \
         and commit the result.",
        path.display()
    );
}

/// Every field that a structure file can hold is described, each entry refuses a field that
/// the schema does not name, and every reference of the schema resolves.
#[test]
fn the_structure_file_schema_describes_every_field_of_a_file() {
    use junto_protocol::structure_file::StructureFile;
    let text = junto_server::structure_file_document().unwrap();
    let schema: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["$ref"], "#/components/schemas/StructureFile");
    let schemas = schema["components"]["schemas"].as_object().unwrap();
    for (start, _) in text.match_indices("#/components/schemas/") {
        let name: String = text[start + "#/components/schemas/".len()..]
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric())
            .collect();
        assert!(schemas.contains_key(&name), "{name} has no schema");
    }

    let id = "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f";
    let full: StructureFile = serde_json::from_value(serde_json::json!({
        "format": 1,
        "server_roles": [{ "id": id, "name": "r", "permissions": ["administrator"] }],
        "workspaces": [{
            "id": id, "name": "w", "default": true,
            "roles": [{ "id": id, "name": "r", "everyone": true, "permissions": [] }],
            "categories": [{ "id": id, "name": "c", "position": 1 }],
            "channels": [{ "id": id, "name": "c", "kind": "voice", "category": id,
                           "position": 1, "permanent_messages": true, "user_limit": 2,
                           "hides_occupants": true,
                           "overrides": [{ "role": id, "allow": [], "deny": [] }] }],
        }],
        "delete": { "channels": [id] },
    }))
    .unwrap();
    let written = serde_json::to_value(&full).unwrap();
    let described = |name: &str, value: &serde_json::Value| {
        let entry = &schemas[name];
        assert_eq!(entry["additionalProperties"], false, "{name}");
        let properties = entry["properties"].as_object().unwrap();
        for field in value.as_object().unwrap().keys() {
            assert!(
                properties.contains_key(field),
                "{name}.{field} is not described"
            );
        }
        assert_eq!(properties.len(), value.as_object().unwrap().len(), "{name}");
    };
    let workspace = &written["workspaces"][0];
    described("StructureFile", &written);
    described("Deletions", &written["delete"]);
    described("ServerRoleEntry", &written["server_roles"][0]);
    described("WorkspaceEntry", workspace);
    described("RoleEntry", &workspace["roles"][0]);
    described("CategoryEntry", &workspace["categories"][0]);
    described("ChannelEntry", &workspace["channels"][0]);
    described("OverrideEntry", &workspace["channels"][0]["overrides"][0]);
    let required = |name: &str| schemas[name]["required"].clone();
    assert_eq!(required("StructureFile"), serde_json::json!(["format"]));
    assert_eq!(
        required("ChannelEntry"),
        serde_json::json!(["id", "name", "kind"])
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
            let succeeds = |status: &String| {
                status.starts_with('2')
                    || (status == "101" && path == junto_protocol::events::STREAM_PATH)
            };
            assert!(
                responses.keys().any(succeeds),
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
            "get /api/v1/health",
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

#[test]
fn the_avatar_routes_describe_the_image_as_binary_content() {
    let document: serde_json::Value =
        serde_json::from_str(&junto_server::openapi_document().unwrap()).unwrap();
    let binary = serde_json::json!({ "type": "string", "format": "binary" });
    let upload = &document["paths"]["/api/v1/account/avatar"]["put"]["requestBody"]["content"];
    let upload = upload.as_object().unwrap();
    assert!(!upload.is_empty());
    for (media_type, content) in upload {
        assert_eq!(content["schema"], binary, "{media_type}");
    }
    let served = &document["paths"]["/api/v1/accounts/{account_id}/avatar"]["get"]["responses"]["200"]
        ["content"];
    for media_type in ["image/png", "image/jpeg", "image/webp"] {
        assert_eq!(served[media_type]["schema"], binary, "{media_type}");
    }
}
