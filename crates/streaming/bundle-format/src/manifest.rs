//! What a bundle requires of an app, what an app provides, and whether
//! they fit.
//!
//! The release build writes a bundle's [`Requires`] into the bundle
//! (`idealyst.requires`, a custom section the signature covers). The app
//! builds its [`Provides`] at run time from what it registered
//! (`remote_host::remote::provides`). [`check`] compares them, before a
//! bundle runs or before an app downloads one.
//!
//! The rule is a subset: every component, prop, host function and remote
//! component the bundle uses must be in the app, with the same shape. Shapes
//! are text (`runtime_vocabulary::remote::shape`), equal when the types
//! cross the same way; `?` stands for a type whose shape isn't known, and
//! matches anything. So an app that adds a component or a prop still runs
//! every older bundle, and a bundle that starts using something newer
//! fails only on the apps that lack it, each problem named.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// What a bundle uses from an app.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requires {
    /// App components it builds: name → each prop some call site sets → its shape.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, BTreeMap<String, String>>,
    /// Host functions it calls: import name (`path#schema`) → signature shape.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub host_fns: BTreeMap<String, String>,
    /// Remote components it provides: name → parameters in order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub remote: BTreeMap<String, Vec<Param>>,
    /// Types it can read as context: name → shape. Every `#[derive(Remote)]`
    /// type compiled into the bundle, used or not, so a mismatch here is a
    /// warning (see [`Problem::is_error`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub contexts: BTreeMap<String, String>,
}

/// What an app offers bundles. The same maps as [`Requires`], for
/// everything the app registered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provides {
    /// The value codec (`CODEC_VERSION`).
    pub codec: u32,
    pub components: BTreeMap<String, BTreeMap<String, String>>,
    pub host_fns: BTreeMap<String, String>,
    pub remote: BTreeMap<String, Vec<Param>>,
    pub contexts: BTreeMap<String, String>,
}

/// One parameter of a remote component: its name and shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub shape: String,
}

/// One way a bundle doesn't fit an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The bundle was built with another value codec.
    Codec { bundle: u32, app: u32 },
    /// The bundle builds an app component the app doesn't have.
    MissingComponent { component: String },
    /// The bundle sets a prop the app's component doesn't have.
    MissingProp { component: String, prop: String },
    /// A prop crosses differently.
    PropShape { component: String, prop: String, bundle: String, app: String },
    /// The bundle calls a host function the app doesn't export (`changed`:
    /// the app exports one by that path with another signature).
    MissingHostFn { path: String, changed: bool },
    /// A host function's arguments or result cross differently.
    HostFnShape { path: String, bundle: String, app: String },
    /// The app mounts a remote component with parameters the bundle's
    /// doesn't take.
    RemoteParams { component: String, bundle: String, app: String },
    /// A context type crosses differently (a warning: the bundle may never
    /// read it).
    ContextShape { name: String, bundle: String, app: String },
}

impl Problem {
    /// Whether the bundle can't run on the app. The one warning is a
    /// context type, which the bundle lists whether or not it reads it.
    pub fn is_error(&self) -> bool {
        !matches!(self, Problem::ContextShape { .. })
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Codec { bundle, app } => {
                write!(f, "the bundle uses value codec {bundle}, the app codec {app}")
            }
            Problem::MissingComponent { component } => write!(f, "the app has no component `{component}`"),
            Problem::MissingProp { component, prop } => {
                write!(f, "`{component}` has no prop `{prop}` in the app")
            }
            Problem::PropShape { component, prop, bundle, app } => {
                write!(f, "`{component}.{prop}`: the bundle sends `{bundle}`, the app takes `{app}`")
            }
            Problem::MissingHostFn { path, changed: false } => write!(f, "the app has no host function `{path}`"),
            Problem::MissingHostFn { path, changed: true } => {
                write!(f, "host function `{path}`: the app's has another signature")
            }
            Problem::HostFnShape { path, bundle, app } => {
                write!(f, "host function `{path}`: the bundle calls `{bundle}`, the app has `{app}`")
            }
            Problem::RemoteParams { component, bundle, app } => {
                write!(f, "remote component `{component}`: the app sends `{app}`, the bundle takes `{bundle}`")
            }
            Problem::ContextShape { name, bundle, app } => {
                write!(f, "context `{name}`: the bundle reads `{bundle}`, the app provides `{app}`")
            }
        }
    }
}

/// Every way `requires` (a bundle built with value codec `codec`) doesn't
/// fit `provides`. Empty of errors ([`Problem::is_error`]) means the bundle
/// runs on that app.
pub fn check(requires: &Requires, codec: u32, provides: &Provides) -> Vec<Problem> {
    let mut out = Vec::new();
    if codec != provides.codec {
        out.push(Problem::Codec { bundle: codec, app: provides.codec });
    }
    for (component, props) in &requires.components {
        let Some(app_props) = provides.components.get(component) else {
            out.push(Problem::MissingComponent { component: component.clone() });
            continue;
        };
        for (prop, shape) in props {
            match app_props.get(prop) {
                None => out.push(Problem::MissingProp { component: component.clone(), prop: prop.clone() }),
                Some(app) if !shapes_match(shape, app) => out.push(Problem::PropShape {
                    component: component.clone(),
                    prop: prop.clone(),
                    bundle: shape.clone(),
                    app: app.clone(),
                }),
                Some(_) => {}
            }
        }
    }
    for (import, shape) in &requires.host_fns {
        let path = import.rsplit_once('#').map_or(import.as_str(), |(p, _)| p);
        match provides.host_fns.get(import) {
            None => {
                let changed = provides.host_fns.keys().any(|k| k.rsplit_once('#').is_some_and(|(p, _)| p == path));
                out.push(Problem::MissingHostFn { path: path.to_string(), changed });
            }
            Some(app) if !shapes_match(shape, app) => out.push(Problem::HostFnShape {
                path: path.to_string(),
                bundle: shape.clone(),
                app: app.clone(),
            }),
            Some(_) => {}
        }
    }
    // Only the remote components this app mounts: a bundle may provide
    // more, and the app may mount others from other bundles.
    for (component, params) in &requires.remote {
        let Some(app) = provides.remote.get(component) else { continue };
        let fits = params.len() == app.len()
            && params.iter().zip(app).all(|(b, a)| b.name == a.name && shapes_match(&b.shape, &a.shape));
        if !fits {
            out.push(Problem::RemoteParams {
                component: component.clone(),
                bundle: param_list(params),
                app: param_list(app),
            });
        }
    }
    for (name, shape) in &requires.contexts {
        if let Some(app) = provides.contexts.get(name) {
            if !shapes_match(shape, app) {
                out.push(Problem::ContextShape { name: name.clone(), bundle: shape.clone(), app: app.clone() });
            }
        }
    }
    out
}

fn param_list(params: &[Param]) -> String {
    let parts: Vec<String> = params.iter().map(|p| format!("{}: {}", p.name, p.shape)).collect();
    format!("({})", parts.join(", "))
}

/// Whether two shapes describe the same crossing: equal, where `?` on
/// either side matches one whole term on the other.
pub fn shapes_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    loop {
        match (a.get(i), b.get(j)) {
            (None, None) => return true,
            (Some(b'?'), _) if j < b.len() => {
                i += 1;
                j = skip_term(b, j);
            }
            (_, Some(b'?')) if i < a.len() => {
                j += 1;
                i = skip_term(a, i);
            }
            (Some(x), Some(y)) if x == y => {
                i += 1;
                j += 1;
            }
            _ => return false,
        }
    }
}

/// The end of the term starting at `at`: up to the first separator or
/// closing bracket outside any brackets it opens.
fn skip_term(s: &[u8], mut at: usize) -> usize {
    let mut depth = 0usize;
    while let Some(&c) = s.get(at) {
        match c {
            b'<' | b'(' | b'{' | b'[' => depth += 1,
            b'>' | b')' | b'}' | b']' if depth == 0 => return at,
            b'>' | b')' | b'}' | b']' => depth -= 1,
            b',' | b';' | b'|' if depth == 0 => return at,
            // `->` inside a term is part of it; at depth 0 it ends the
            // arguments, which a term never spans.
            b'-' if depth == 0 && s.get(at + 1) == Some(&b'>') => return at,
            _ => {}
        }
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn app() -> Provides {
        let mut p = Provides { codec: 2, ..Default::default() };
        p.components.insert("ui::Card".into(), props(&[("title", "reactive<str>"), ("tone", "key<ToneRef>"), ("children", "list<Element>")]));
        p.host_fns.insert("app::sort#00000000000000aa".into(), "fn(list<u32>)->list<u32>".into());
        p.remote.insert("shop::Offer".into(), vec![Param { name: "id".into(), shape: "u64".into() }]);
        p.contexts.insert("app::Prefs".into(), "Prefs{dark:bool}".into());
        p
    }

    #[test]
    fn a_bundle_using_a_subset_fits() {
        let mut r = Requires::default();
        r.components.insert("ui::Card".into(), props(&[("title", "reactive<str>")]));
        r.host_fns.insert("app::sort#00000000000000aa".into(), "fn(list<u32>)->list<u32>".into());
        r.remote.insert("shop::Offer".into(), vec![Param { name: "id".into(), shape: "u64".into() }]);
        // A remote component the app doesn't mount is the bundle's business.
        r.remote.insert("shop::Extra".into(), vec![]);
        assert_eq!(check(&r, 2, &app()), vec![]);
    }

    #[test]
    fn each_mismatch_is_named() {
        let mut r = Requires::default();
        r.components.insert("ui::Card".into(), props(&[("title", "reactive<f64>"), ("elevation", "u8")]));
        r.components.insert("ui::Chart".into(), props(&[]));
        r.host_fns.insert("app::sort#00000000000000bb".into(), "fn(list<u32>)->list<u32>".into());
        r.host_fns.insert("app::gone#0000000000000001".into(), "fn()->()".into());
        r.remote.insert("shop::Offer".into(), vec![Param { name: "id".into(), shape: "str".into() }]);
        r.contexts.insert("app::Prefs".into(), "Prefs{dark:bool,size:u8}".into());
        let problems = check(&r, 3, &app());
        let text: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            text,
            [
                "the bundle uses value codec 3, the app codec 2",
                "`ui::Card` has no prop `elevation` in the app",
                "`ui::Card.title`: the bundle sends `reactive<f64>`, the app takes `reactive<str>`",
                "the app has no component `ui::Chart`",
                "the app has no host function `app::gone`",
                "host function `app::sort`: the app's has another signature",
                "remote component `shop::Offer`: the app sends `(id: u64)`, the bundle takes `(id: str)`",
                "context `app::Prefs`: the bundle reads `Prefs{dark:bool,size:u8}`, the app provides `Prefs{dark:bool}`",
            ]
        );
        assert_eq!(problems.iter().filter(|p| !p.is_error()).count(), 1, "only the context is a warning");
    }

    #[test]
    fn an_unknown_shape_matches_one_whole_term() {
        assert!(shapes_match("list<?>", "list<opt<(u32,str)>>"));
        assert!(shapes_match("(?,u8)", "(Person{a:u8},u8)"));
        assert!(shapes_match("fn(?)->bool", "fn(list<u8>)->bool"));
        assert!(shapes_match("?", "Person{a:u8}"));
        assert!(!shapes_match("(?,u8)", "(str,u16)"));
        assert!(!shapes_match("list<?>", "opt<u8>"));
        assert!(!shapes_match("Person{a:u8}", "Person{a:u8,b:u8}"));
    }

    #[test]
    fn requires_round_trips_and_omits_what_is_empty() {
        let mut r = Requires::default();
        r.components.insert("ui::Card".into(), props(&[("title", "reactive<str>")]));
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"components":{"ui::Card":{"title":"reactive<str>"}}}"#);
        assert_eq!(serde_json::from_str::<Requires>(&json).unwrap(), r);
    }
}
