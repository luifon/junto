use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn emit(event: &str, mut fields: Value) {
    if let Some(map) = fields.as_object_mut() {
        map.insert("event".into(), json!(event));
        map.insert("unix_ms".into(), json!(unix_ms()));
    }
    println!("{fields}");
}
