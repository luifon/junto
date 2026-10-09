//! The generated description of the event stream (ADR 0202).

use std::collections::BTreeMap;

use junto_protocol::API_VERSION;
use junto_protocol::events::{ClientMessage, CloseCode, STREAM_PATH, ServerMessage};
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(components(schemas(ServerMessage, ClientMessage)))]
struct StreamTypes;

/// The committed file `docs/api/events.json`.
#[derive(Serialize)]
pub struct Document {
    title: &'static str,
    api_version: u32,
    path: &'static str,
    /// Every message is one JSON object in a text frame.
    server_message: Value,
    client_message: Value,
    /// The schema of every event, by the value of its field `type`.
    events: BTreeMap<String, Value>,
    /// The codes of the close frames that the server sends.
    close_codes: BTreeMap<u16, &'static str>,
    components: Value,
}

fn reference(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

/// The name of the event that one alternative of the `Event` schema describes.
fn event_name(alternative: &Value) -> Option<&str> {
    alternative
        .pointer("/properties/type/enum/0")
        .and_then(Value::as_str)
}

pub fn document() -> Result<Document, serde_json::Error> {
    let components = serde_json::to_value(StreamTypes::openapi().components)?;
    let alternatives = components
        .pointer("/schemas/Event/oneOf")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let events = alternatives
        .into_iter()
        .filter_map(|alternative| {
            let name = event_name(&alternative)?.to_owned();
            Some((name, alternative))
        })
        .collect();
    Ok(Document {
        title: "Junto event stream",
        api_version: API_VERSION,
        path: STREAM_PATH,
        server_message: reference("ServerMessage"),
        client_message: reference("ClientMessage"),
        events,
        close_codes: CloseCode::ALL
            .iter()
            .map(|code| (code.code(), code.name()))
            .collect(),
        components,
    })
}

#[cfg(test)]
mod tests {
    use junto_protocol::events::Event;

    use super::*;

    #[test]
    fn every_event_has_a_schema() {
        let document = serde_json::to_value(document().unwrap()).unwrap();
        let described: Vec<&str> = document["events"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut known = Event::NAMES.to_vec();
        known.sort_unstable();
        assert_eq!(described, known);
        for name in Event::NAMES {
            let schema = &document["events"][name];
            assert_eq!(schema["type"], "object", "{name}");
            assert!(
                schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("type")),
                "{name}"
            );
        }
    }

    #[test]
    fn every_message_and_every_referenced_type_has_a_schema() {
        let text = serde_json::to_string(&document().unwrap()).unwrap();
        let document: Value = serde_json::from_str(&text).unwrap();
        let schemas = document["components"]["schemas"].as_object().unwrap();
        for name in ["ServerMessage", "ClientMessage", "Event", "Cursor", "Level"] {
            assert!(schemas.contains_key(name), "{name}");
        }
        let server_messages = schemas["ServerMessage"]["oneOf"].as_array().unwrap();
        let mut described: Vec<&str> = server_messages.iter().filter_map(event_name).collect();
        described.sort_unstable();
        let mut known = ServerMessage::NAMES.to_vec();
        known.sort_unstable();
        assert_eq!(described, known);

        for (start, _) in text.match_indices("#/components/schemas/") {
            let name: String = text[start + "#/components/schemas/".len()..]
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric())
                .collect();
            assert!(schemas.contains_key(&name), "{name} has no schema");
        }
        assert_eq!(document["close_codes"]["4003"], "slow");
    }

    #[test]
    fn the_schema_states_what_a_client_does_after_a_resync() {
        let document = serde_json::to_value(document().unwrap()).unwrap();
        let messages = document["components"]["schemas"]["ServerMessage"]["oneOf"]
            .as_array()
            .unwrap();
        let resync = messages
            .iter()
            .find(|message| event_name(message) == Some("resync"))
            .unwrap();
        let rule = resync["description"].as_str().unwrap();
        for stated in ["discards", "every", "snapshot", "invites"] {
            assert!(rule.contains(stated), "{stated:?} is not in: {rule}");
        }
        let snapshot = serde_json::to_value(utoipa::schema!(
            #[inline]
            junto_protocol::snapshot::Snapshot
        ))
        .unwrap();
        let carried = snapshot["description"].as_str().unwrap();
        assert!(carried.contains("on demand"), "{carried}");
    }
}
