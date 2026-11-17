//! Device classes: what kind of device the app is running as, in the terms
//! an idealyst app itself branches on. Its breakpoint (from the app's own
//! breakpoint table) and its primary pointer, not a device name. A test
//! declares the classes it means something on
//! (`#[robot_test(form = phone)]`, `input = touch`), and runs unchanged on
//! an iPhone, a Pixel and a phone-sized browser alike.
//!
//! | form      | breakpoint |
//! |-----------|------------|
//! | `phone`   | xs, sm     |
//! | `tablet`  | md         |
//! | `desktop` | lg, xl     |
//!
//! `input` is `touch` when the primary pointer is coarse (a finger),
//! `pointer` when it is fine (a mouse, a trackpad).
//!
//! A test whose class doesn't match the app is **not applicable**: it prints
//! `[robot_test] N/A <test>: <reason>` and returns, and `idealyst test`
//! counts it separately from passes (`IDEALYST_TEST_RESULTS`, below).

use std::io::Write;

use serde_json::json;

use crate::App;

/// The env var naming the file `idealyst test` reads a run's not-applicable
/// tests from: one JSON object per line, `{"test", "outcome": "na", "reason"}`.
pub const RESULTS_ENV: &str = "IDEALYST_TEST_RESULTS";

/// A device's form factor, from its breakpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Phone,
    Tablet,
    Desktop,
}

impl Form {
    /// The form a breakpoint name (`xs` … `xl`) belongs to.
    pub fn of_breakpoint(breakpoint: &str) -> Option<Form> {
        Some(match breakpoint {
            "xs" | "sm" => Form::Phone,
            "md" => Form::Tablet,
            "lg" | "xl" => Form::Desktop,
            _ => return None,
        })
    }

    /// `phone` / `tablet` / `desktop`.
    pub fn name(self) -> &'static str {
        match self {
            Form::Phone => "phone",
            Form::Tablet => "tablet",
            Form::Desktop => "desktop",
        }
    }

    /// The form called `name`.
    pub fn parse(name: &str) -> Option<Form> {
        [Form::Phone, Form::Tablet, Form::Desktop].into_iter().find(|f| f.name() == name)
    }
}

/// How the user mainly points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// A finger: the primary pointer is coarse.
    Touch,
    /// A mouse or trackpad: the primary pointer is fine.
    Pointer,
}

impl Input {
    pub fn name(self) -> &'static str {
        match self {
            Input::Touch => "touch",
            Input::Pointer => "pointer",
        }
    }
}

/// What the app reports about its device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceClass {
    /// `None` for a breakpoint this crate doesn't know.
    pub form: Option<Form>,
    /// `None` when the app has no pointing device.
    pub input: Option<Input>,
    pub breakpoint: String,
    pub width: u32,
    pub height: u32,
}

impl DeviceClass {
    fn describe(&self) -> String {
        let form = self.form.map_or("an unknown form", Form::name);
        let input = self.input.map_or("no pointer", Input::name);
        format!("the app is a {form} ({}, {}x{}, {input})", self.breakpoint, self.width, self.height)
    }
}

impl App {
    /// The app's device class, as it reports it now.
    pub fn device_class(&mut self) -> DeviceClass {
        let v = self.call("get_viewport", json!({}));
        let breakpoint = v["breakpoint"].as_str().unwrap_or_default().to_string();
        DeviceClass {
            form: Form::of_breakpoint(&breakpoint),
            input: match v["pointer"].as_str() {
                Some("coarse") => Some(Input::Touch),
                Some("fine") => Some(Input::Pointer),
                _ => None,
            },
            breakpoint,
            width: v["width"].as_u64().unwrap_or(0) as u32,
            height: v["height"].as_u64().unwrap_or(0) as u32,
        }
    }
}

/// Why a test that declared `forms` / `inputs` does not apply to `class`,
/// or `None` when it does. An empty list accepts anything.
pub fn not_applicable(class: &DeviceClass, forms: &[&str], inputs: &[&str]) -> Option<String> {
    let form_ok = forms.is_empty() || class.form.is_some_and(|f| forms.contains(&f.name()));
    let input_ok = inputs.is_empty() || class.input.is_some_and(|i| inputs.contains(&i.name()));
    if form_ok && input_ok {
        return None;
    }
    let mut needs = Vec::new();
    if !forms.is_empty() {
        needs.push(forms.join(" or "));
    }
    if !inputs.is_empty() {
        needs.push(format!("{} input", inputs.join(" or ")));
    }
    Some(format!("needs {}; {}", needs.join(", "), class.describe()))
}

/// What a `#[robot_test(form = …, input = …)]` expansion calls before its
/// body: `Some(reason)` when the test does not apply to the app, after
/// recording it for the runner.
#[doc(hidden)]
pub fn __not_applicable(app: &mut App, test: &str, forms: &[&str], inputs: &[&str]) -> Option<String> {
    let reason = not_applicable(&app.device_class(), forms, inputs)?;
    record_not_applicable(test, &reason);
    Some(reason)
}

/// Append a not-applicable result to the runner's results file, if it named
/// one. Best-effort: the console line is the fallback.
fn record_not_applicable(test: &str, reason: &str) {
    let Some(path) = std::env::var_os(RESULTS_ENV) else { return };
    let line = json!({ "test": test, "outcome": "na", "reason": reason }).to_string();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(bp: &str, input: Option<Input>) -> DeviceClass {
        DeviceClass { form: Form::of_breakpoint(bp), input, breakpoint: bp.into(), width: 390, height: 844 }
    }

    #[test]
    fn breakpoints_map_to_forms() {
        assert_eq!(Form::of_breakpoint("xs"), Some(Form::Phone));
        assert_eq!(Form::of_breakpoint("sm"), Some(Form::Phone));
        assert_eq!(Form::of_breakpoint("md"), Some(Form::Tablet));
        assert_eq!(Form::of_breakpoint("xl"), Some(Form::Desktop));
        assert_eq!(Form::of_breakpoint("huge"), None);
    }

    #[test]
    fn a_class_test_applies_only_where_it_matches_and_says_why_not() {
        let phone = class("xs", Some(Input::Touch));
        assert_eq!(not_applicable(&phone, &["phone"], &[]), None);
        assert_eq!(not_applicable(&phone, &["phone", "tablet"], &["touch"]), None);
        assert_eq!(not_applicable(&phone, &[], &[]), None, "no class declared: everywhere");
        let why = not_applicable(&phone, &["desktop"], &[]).unwrap();
        assert!(why.contains("needs desktop") && why.contains("the app is a phone (xs, 390x844, touch)"), "{why}");
        let why = not_applicable(&class("xl", Some(Input::Pointer)), &[], &["touch"]).unwrap();
        assert!(why.contains("needs touch input") && why.contains("pointer"), "{why}");
    }
}
