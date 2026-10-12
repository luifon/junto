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
        "delete": { "channels": [id], "categories": [id], "roles": [id],
                    "server_roles": [id], "workspaces": [id] },
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

/// What serde does with one name of an enum: `Ok` when it reads the name as a variant that
/// it writes with the same name. `tag` is the field that names the variant of an enum with
/// fields.
type Wire = fn(Option<&str>, &str) -> Result<(), String>;

fn wire<T>(tag: Option<&str>, name: &str) -> Result<(), String>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let probe = match tag {
        None => serde_json::json!(name),
        Some(tag) => serde_json::json!({ tag: name }),
    };
    match serde_json::from_value::<T>(probe) {
        Ok(read) => {
            let written = serde_json::to_value(&read)
                .map_err(|_| "serde reads it as a variant that it never writes".to_owned())?;
            let written = match tag {
                None => written,
                Some(tag) => written[tag].clone(),
            };
            if written == serde_json::json!(name) {
                Ok(())
            } else {
                Err(format!("serde writes it as {written}"))
            }
        }
        // A variant with fields: the name is known, and the probe lacks its fields.
        Err(error) if !error.to_string().starts_with("unknown variant") => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// The names of the variants of `T` as serde states them for a name that it does not know.
/// `None` for an enum that reads an unknown name as a variant of its own.
fn listed<T: serde::de::DeserializeOwned>(tag: Option<&str>) -> Option<Vec<String>> {
    let probe = match tag {
        None => serde_json::json!("?"),
        Some(tag) => serde_json::json!({ tag: "?" }),
    };
    let error = serde_json::from_value::<T>(probe).err()?.to_string();
    let names = error.strip_prefix("unknown variant `?`, expected ")?;
    let mut names: Vec<String> = names
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    names.sort();
    Some(names)
}

type Listed = fn(Option<&str>) -> Option<Vec<String>>;

macro_rules! enums {
    ($($type:ident $(by $tag:literal)?,)+) => {
        &[$((stringify!($type), None $(.or(Some($tag)))?, wire::<$type> as Wire,
             listed::<$type> as Listed),)+]
    };
}

/// Every place of a document that holds a list of enum values, as the path to it.
fn enum_places(value: &serde_json::Value, path: &str, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, inner) in fields {
                if name == "enum" && inner.is_array() {
                    found.push(path.to_owned());
                }
                enum_places(inner, &format!("{path}/{name}"), found);
            }
        }
        serde_json::Value::Array(items) => {
            for inner in items {
                enum_places(inner, &format!("{path}/*"), found);
            }
        }
        _ => {}
    }
}

/// The generated documents take the names of an enum from attributes that the schema
/// derive reads apart from serde, so the two can differ. Every enum of the three documents
/// lists exactly the names that serde reads and writes; an enum that this test does not
/// know fails it.
#[test]
fn every_enum_of_the_generated_documents_lists_the_names_that_serde_writes() {
    use junto_protocol::access_token::TokenAction;
    use junto_protocol::account::{AccountStatus, DeviceOrigin, ImageType};
    use junto_protocol::audit::AuditEvent;
    use junto_protocol::auth::ChallengeOperation;
    use junto_protocol::call::{
        BlockReason, CallPlace, EntryOutcome, InvitationOutcome, LeaveReason, MeetingEnd,
        MoveOutcome, ShareAudio,
    };
    use junto_protocol::discovery::{NotificationKind, NotificationLevel};
    use junto_protocol::error::ErrorCode;
    use junto_protocol::events::{
        ClientMessage, EndReason, Event, Level, ResyncReason, ServerMessage,
    };
    use junto_protocol::export::{ExportFailure, ExportRemoval, ExportState};
    use junto_protocol::invite::{InviteRevocation, JoinRequestState};
    use junto_protocol::message::{Block, Inline, Mark, MessagePlace};
    use junto_protocol::recording::{
        FileCheck, FileProblem, RecordingFailure, RecordingState, RecordingStopReason,
        SegmentFailure, SegmentSource, SegmentState,
    };
    use junto_protocol::structure::ChannelKind;
    use junto_protocol::structure_file::{StructureChange, StructureKind};
    use junto_protocol::transcript::{
        LineSource, SegmentReason, SegmentTranscription, SpeakerSeparation, SpeakersReason,
        TranscriptFailure, TranscriptState,
    };
    use junto_protocol::{ServerPermission, WorkspacePermission};

    /// The names of an enum in a schema of the generated documents: the values of a plain
    /// enum, or the values of the one field that names the variant of an enum with fields.
    fn names_of(schema: &serde_json::Value) -> Option<(Option<String>, Vec<String>)> {
        let texts = |values: &serde_json::Value| -> Vec<String> {
            let values = values.as_array().unwrap().iter();
            values
                .map(|value| value.as_str().unwrap().to_owned())
                .collect()
        };
        if let Some(values) = schema.get("enum") {
            return Some((None, texts(values)));
        }
        let mut tags = std::collections::BTreeSet::new();
        let mut names = Vec::new();
        for variant in schema.get("oneOf")?.as_array().unwrap() {
            let fields = variant
                .get("properties")
                .and_then(|fields| fields.as_object());
            for (field, described) in fields.into_iter().flatten() {
                if let Some(values) = described.get("enum") {
                    tags.insert(field.clone());
                    names.extend(texts(values));
                }
            }
        }
        let tag = tags.pop_first()?;
        assert!(tags.is_empty(), "two fields name the variant: {schema}");
        Some((Some(tag), names))
    }

    let checked: &[(&str, Option<&str>, Wire, Listed)] = enums![
        AccountStatus,
        AuditEvent by "action",
        Block by "kind",
        BlockReason,
        CallPlace by "kind",
        ChallengeOperation,
        ChannelKind,
        ClientMessage by "type",
        DeviceOrigin,
        EndReason,
        EntryOutcome,
        ErrorCode,
        Event by "type",
        ExportFailure,
        ExportRemoval,
        ExportState,
        FileCheck,
        FileProblem,
        ImageType,
        Inline by "kind",
        InvitationOutcome,
        InviteRevocation,
        JoinRequestState,
        LeaveReason,
        Level,
        LineSource,
        Mark,
        MeetingEnd,
        MessagePlace by "kind",
        MoveOutcome,
        NotificationKind,
        NotificationLevel,
        RecordingFailure,
        RecordingState,
        RecordingStopReason,
        ResyncReason,
        SegmentFailure,
        SegmentReason,
        SegmentSource,
        SegmentState,
        SegmentTranscription,
        ServerMessage by "type",
        ServerPermission,
        ShareAudio,
        SpeakerSeparation,
        SpeakersReason,
        StructureChange by "change",
        StructureKind,
        TokenAction,
        TranscriptFailure,
        TranscriptState,
        WorkspacePermission,
    ];
    let documents = [
        ("openapi.json", junto_server::openapi_document().unwrap()),
        ("events.json", junto_server::events_document().unwrap()),
        (
            "structure-file.json",
            junto_server::structure_file_document().unwrap(),
        ),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (document, text) in documents {
        let document_value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let schemas = document_value["components"]["schemas"].as_object().unwrap();
        for (name, schema) in schemas {
            let Some((tag, names)) = names_of(schema) else {
                continue;
            };
            let Some((_, known_tag, wire, listed)) =
                checked.iter().find(|(known, ..)| known == name)
            else {
                panic!("{document}: the enum {name} is not in the table of this test");
            };
            assert_eq!(tag.as_deref(), *known_tag, "{document}: {name}");
            for value in &names {
                if let Err(reason) = wire(*known_tag, value) {
                    panic!("{document}: {name} lists {value:?}: {reason}");
                }
            }
            if let Some(of_serde) = listed(*known_tag) {
                let mut names = names.clone();
                names.sort();
                assert_eq!(names, of_serde, "{document}: {name}");
            }
            seen.insert(name.clone());
        }

        // The event stream document describes each event once more under its name.
        if let Some(events) = document_value.get("events") {
            for (name, described) in events.as_object().unwrap() {
                assert_eq!(
                    described["properties"]["type"]["enum"],
                    serde_json::json!([name]),
                    "{document}: {name}"
                );
                wire::<Event>(Some("type"), name).unwrap();
            }
        }

        // No list of enum values is at a place that the steps above do not read.
        let mut places = Vec::new();
        enum_places(&document_value, "", &mut places);
        for place in places {
            let parts: Vec<&str> = place.split('/').skip(1).collect();
            let read = matches!(
                parts.as_slice(),
                ["components", "schemas", _]
                    | ["components", "schemas", _, "oneOf", "*", "properties", _]
                    | ["events", _, "properties", "type"]
            );
            assert!(read, "{document}: an enum at {place} is not checked");
        }
    }
    for (name, ..) in checked {
        assert!(seen.contains(*name), "no document has the enum {name}");
    }
}
