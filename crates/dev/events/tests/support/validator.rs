//! A strict validator for the JSON Schema subset `schemars` emits for the
//! event types.
//!
//! Hand-written rather than a validator crate: the full JSON Schema
//! validators pull a large dependency tree for keywords this schema never
//! uses. STRICT is what makes that safe — a keyword this file does not
//! implement fails the test instead of being ignored, so a schema change
//! that introduces one cannot validate vacuously.

use serde_json::Value;

/// Keywords that describe rather than constrain.
const ANNOTATIONS: &[&str] = &["description", "title", "$schema", "$id", "x-schema-version", "format", "$defs"];

pub struct Validator {
    root: Value,
}

impl Validator {
    pub fn new(root: Value) -> Self {
        Self { root }
    }

    /// Every violation of the schema by `instance`, as `path: why`.
    pub fn errors(&self, instance: &Value) -> Vec<String> {
        let mut out = Vec::new();
        self.check(&self.root, instance, "$", &mut out);
        out
    }

    fn resolve<'a>(&'a self, reference: &str) -> &'a Value {
        let pointer = reference.strip_prefix('#').unwrap_or_else(|| panic!("non-local $ref {reference}"));
        self.root.pointer(pointer).unwrap_or_else(|| panic!("dangling $ref {reference}"))
    }

    fn check(&self, schema: &Value, v: &Value, path: &str, out: &mut Vec<String>) {
        let Some(obj) = schema.as_object() else {
            match schema {
                Value::Bool(true) => return,
                Value::Bool(false) => {
                    out.push(format!("{path}: the schema admits nothing here"));
                    return;
                }
                _ => panic!("not a schema at {path}: {schema}"),
            }
        };
        for (k, s) in obj {
            match k.as_str() {
                k if ANNOTATIONS.contains(&k) => {}
                "$ref" => self.check(self.resolve(s.as_str().unwrap()), v, path, out),
                "type" => {
                    let allowed: Vec<&str> = match s {
                        Value::String(t) => vec![t.as_str()],
                        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
                        _ => panic!("bad type at {path}"),
                    };
                    if !allowed.iter().any(|t| type_matches(t, v)) {
                        out.push(format!("{path}: expected {allowed:?}, got {v}"));
                    }
                }
                "const" => {
                    if v != s {
                        out.push(format!("{path}: expected {s}, got {v}"));
                    }
                }
                "enum" => {
                    if !s.as_array().unwrap().contains(v) {
                        out.push(format!("{path}: {v} is not one of {s}"));
                    }
                }
                "minimum" => {
                    if let (Some(n), Some(min)) = (v.as_f64(), s.as_f64()) {
                        if n < min {
                            out.push(format!("{path}: {n} < {min}"));
                        }
                    }
                }
                "required" => {
                    if let Some(o) = v.as_object() {
                        for name in s.as_array().unwrap().iter().filter_map(Value::as_str) {
                            if !o.contains_key(name) {
                                out.push(format!("{path}: missing `{name}`"));
                            }
                        }
                    }
                }
                "properties" => {
                    if let Some(o) = v.as_object() {
                        for (name, sub) in s.as_object().unwrap() {
                            if let Some(x) = o.get(name) {
                                self.check(sub, x, &format!("{path}.{name}"), out);
                            }
                        }
                    }
                }
                "items" => {
                    if let Some(a) = v.as_array() {
                        for (i, x) in a.iter().enumerate() {
                            self.check(s, x, &format!("{path}[{i}]"), out);
                        }
                    }
                }
                "oneOf" | "anyOf" => {
                    let branches = s.as_array().unwrap();
                    let passing = branches
                        .iter()
                        .filter(|b| {
                            let mut e = Vec::new();
                            self.check(b, v, path, &mut e);
                            e.is_empty()
                        })
                        .count();
                    let ok = if k == "oneOf" { passing == 1 } else { passing >= 1 };
                    if !ok {
                        out.push(format!("{path}: {passing} of {} `{k}` branches match", branches.len()));
                    }
                }
                "allOf" => {
                    for b in s.as_array().unwrap() {
                        self.check(b, v, path, out);
                    }
                }
                other => panic!(
                    "the schema uses `{other}` at {path}, which this validator does not \
                     implement — teach it, rather than let the keyword pass unchecked"
                ),
            }
        }
    }
}

fn type_matches(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        other => panic!("unknown JSON Schema type {other}"),
    }
}
