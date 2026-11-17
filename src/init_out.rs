//! JSON for `eb-pipeline init --list --format json`.

use crate::templates;
use serde_json::{json, Value};

/// Version of [`templates_json`]'s format.
pub const SCHEMA_VERSION: u32 = 1;

/// Every built-in template as one JSON value.
pub fn templates_json() -> Value {
    let rows: Vec<Value> = templates::TEMPLATES
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "summary": t.summary,
            })
        })
        .collect();
    json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "templates",
        "templates": rows,
    })
}
