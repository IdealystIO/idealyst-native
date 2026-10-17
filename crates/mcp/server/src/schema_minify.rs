//! Shrinks the input schemas `schemars` derives for tool arguments before
//! they are advertised.
//!
//! Every byte of a tool schema is resent to the model as context for the
//! whole session, and schemars spells out details no caller needs:
//!
//! - `"$schema": "https://json-schema.org/draft/2020-12/schema"` on every tool.
//! - `Option<T>` as `"type": ["T", "null"]` plus `"default": null`. An
//!   optional argument is already expressed by its absence from `required`,
//!   and serde accepts an explicit `null` for an `Option` either way.
//! - Rust integer widths as `"format": "uint64", "minimum": 0` (and
//!   `"maximum": 65535` for a `u16`), and floats as `"format": "float"`.
//! - `Option<Struct>` as `"anyOf": [{ "$ref": … }, { "type": "null" }]`.
//! - The args struct's name as `"title"` and its Rust doc comment as the
//!   root `"description"`, on the root and on every `$defs` entry. Those
//!   comments are written for maintainers of this crate; the tool's own
//!   description is what a caller reads. Property descriptions stay.
//!
//! Only the advertised schema changes. Arguments are still deserialized
//! into the same typed structs, so validation is unchanged.

use serde_json::{Map, Value};

/// Minify a tool's input schema in place.
pub(crate) fn minify(schema: &mut Map<String, Value>) {
    schema.remove("$schema");
    strip_struct_docs(schema);
    if let Some(Value::Object(defs)) = schema.get_mut("$defs") {
        for def in defs.values_mut() {
            if let Value::Object(d) = def {
                strip_struct_docs(d);
            }
        }
    }
    minify_object(schema);
}

fn strip_struct_docs(obj: &mut Map<String, Value>) {
    obj.remove("title");
    obj.remove("description");
}

fn minify_object(obj: &mut Map<String, Value>) {
    // `["T", "null"]` → `"T"`.
    if let Some(Value::Array(types)) = obj.get("type") {
        let non_null: Vec<Value> =
            types.iter().filter(|t| t.as_str() != Some("null")).cloned().collect();
        if non_null.len() == 1 {
            obj.insert("type".into(), non_null.into_iter().next().unwrap());
        }
    }
    if obj.get("default").is_some_and(Value::is_null) {
        obj.remove("default");
    }
    // `anyOf: [X, {type: null}]` → X's keys merged into this object.
    if let Some(Value::Array(any)) = obj.get("anyOf") {
        let non_null: Vec<&Value> =
            any.iter().filter(|v| v.get("type").and_then(Value::as_str) != Some("null")).collect();
        if non_null.len() == 1 && any.len() == 2 {
            if let Some(inner) = non_null[0].as_object().cloned() {
                obj.remove("anyOf");
                for (k, v) in inner {
                    obj.entry(k).or_insert(v);
                }
            }
        }
    }
    if let Some(Value::String(format)) = obj.get("format") {
        let is_int = format.starts_with("int") || format.starts_with("uint");
        let is_float = format == "float" || format == "double";
        if is_int {
            // The bounds schemars emits for an integer width restate the
            // Rust type, not a constraint the caller must know.
            obj.remove("minimum");
            obj.remove("maximum");
        }
        if is_int || is_float {
            obj.remove("format");
        }
    }
    for (key, value) in obj.iter_mut() {
        match key.as_str() {
            // Maps of name → subschema.
            "properties" | "$defs" | "definitions" => {
                if let Value::Object(children) = value {
                    for child in children.values_mut() {
                        if let Value::Object(c) = child {
                            minify_object(c);
                        }
                    }
                }
            }
            // A single subschema.
            "items" | "additionalProperties" => {
                if let Value::Object(c) = value {
                    minify_object(c);
                }
            }
            // Lists of subschemas.
            "anyOf" | "oneOf" | "allOf" | "prefixItems" => {
                if let Value::Array(list) = value {
                    for item in list {
                        if let Value::Object(c) = item {
                            minify_object(c);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(v: Value) -> Value {
        let Value::Object(mut m) = v else { panic!() };
        minify(&mut m);
        Value::Object(m)
    }

    #[test]
    fn strips_schema_uri_null_unions_and_width_formats() {
        let out = run(json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "app": { "type": ["string", "null"], "default": null, "description": "d" },
                "id": { "type": "integer", "format": "uint64", "minimum": 0 },
                "port": { "type": ["integer", "null"], "format": "uint16", "minimum": 0, "maximum": 65535 },
                "tol": { "type": ["number", "null"], "format": "float" },
                "flag": { "type": "boolean", "default": false }
            },
            "required": ["id"]
        }));
        assert_eq!(
            out,
            json!({
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "d" },
                    "id": { "type": "integer" },
                    "port": { "type": "integer" },
                    "tol": { "type": "number" },
                    "flag": { "type": "boolean", "default": false }
                },
                "required": ["id"]
            })
        );
    }

    #[test]
    fn collapses_optional_ref_and_recurses_into_defs_and_items() {
        let out = run(json!({
            "properties": {
                "aspects": { "type": ["array", "null"], "items": { "$ref": "#/$defs/A" } },
                "one": { "anyOf": [{ "$ref": "#/$defs/A" }, { "type": "null" }], "description": "x" },
                "pts": { "type": "array", "items": { "type": "array", "items": { "type": "number", "format": "float" } } }
            },
            "$defs": { "A": { "properties": { "n": { "type": ["integer", "null"], "format": "int32" } } } }
        }));
        assert_eq!(out["properties"]["aspects"]["type"], "array");
        assert_eq!(out["properties"]["one"], json!({ "$ref": "#/$defs/A", "description": "x" }));
        assert_eq!(out["properties"]["pts"]["items"]["items"], json!({ "type": "number" }));
        assert_eq!(out["$defs"]["A"]["properties"]["n"], json!({ "type": "integer" }));
    }

    #[test]
    fn strips_struct_title_and_doc_but_keeps_property_docs() {
        let out = run(json!({
            "title": "RunDevArgs",
            "description": "Args for `run_dev` — maintainer notes.",
            "properties": { "dir": { "type": "string", "description": "Project dir." } },
            "$defs": { "A": { "title": "A", "description": "struct doc", "properties": {} } }
        }));
        assert_eq!(
            out,
            json!({
                "properties": { "dir": { "type": "string", "description": "Project dir." } },
                "$defs": { "A": { "properties": {} } }
            })
        );
    }

    #[test]
    fn keeps_unions_of_two_real_types() {
        let out = run(json!({ "properties": { "v": { "type": ["string", "integer", "null"] } } }));
        assert_eq!(out["properties"]["v"]["type"], json!(["string", "integer", "null"]));
    }
}
