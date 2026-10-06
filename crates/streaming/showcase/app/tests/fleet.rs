//! A fleet of app versions against a run of releases: which release each
//! installed version can run, and which it chooses.
//!
//! Each host version below is real code — `#[host_fn]`s, a
//! `#[derive(Remote)]` struct and a `#[component]`, changing from version
//! to version the way an app does: a prop added, a host function added, an
//! argument type widened, a field added to a struct. The manifests are made
//! from what the macros generate: each host function's schema and shape,
//! each prop's shape as the component registered it, each struct's shape.
//! Only the NAMES are shared by hand: in one test binary each version lives
//! in its own module, and both host functions and components are keyed by
//! module path, so `v1::sort` and `v3::sort` are renamed `app::sort` — what
//! they are when each version is its own build of the same crate.
//!
//! A release built against a version requires that version's shapes, for
//! the parts it uses: exactly what the release build reads out of a bundle
//! (`build_remote::requires`, the same shape code compiled into the
//! bundle; `tests/requirements.rs` checks that path for real).

#![cfg(not(feature = "inline"))]

use std::collections::BTreeMap;

use ota::index::{resolve, Bundle, Index, Release};
use remote_bundle::{Problem, Provides, Requires};
use runtime_vocabulary::remote::host_fn::import_name;
use runtime_vocabulary::remote::{HostFnDef, CODEC_VERSION};

/// v1: a card with a title; sort `u32`s; save `Prefs { dark }`.
mod v1 {
    use runtime_core::{component, host_fn, ui, Element, Remote};

    #[derive(Clone, Debug, PartialEq, Remote)]
    pub struct Prefs {
        pub dark: bool,
    }

    #[component]
    pub fn Card(title: String) -> Element {
        let _ = title;
        ui! { view() {} }
    }

    #[host_fn]
    pub fn sort(mut xs: Vec<u32>) -> Vec<u32> {
        xs.sort();
        xs
    }

    #[host_fn]
    pub fn save_prefs(prefs: Prefs) {
        let _ = prefs;
    }
}

/// v2: the card gains a subtitle; a new host function, `greet`.
mod v2 {
    use runtime_core::{component, host_fn, ui, Element, Remote};

    #[derive(Clone, Debug, PartialEq, Remote)]
    pub struct Prefs {
        pub dark: bool,
    }

    #[component]
    pub fn Card(title: String, #[prop(default = None)] subtitle: Option<String>) -> Element {
        let _ = (title, subtitle);
        ui! { view() {} }
    }

    #[host_fn]
    pub fn sort(mut xs: Vec<u32>) -> Vec<u32> {
        xs.sort();
        xs
    }

    #[host_fn]
    pub fn save_prefs(prefs: Prefs) {
        let _ = prefs;
    }

    #[host_fn]
    pub fn greet(name: String) -> String {
        format!("hello {name}")
    }
}

/// v3: `sort` widened to `u64` (a new signature: a new schema).
mod v3 {
    use runtime_core::{component, host_fn, ui, Element, Remote};

    #[derive(Clone, Debug, PartialEq, Remote)]
    pub struct Prefs {
        pub dark: bool,
    }

    #[component]
    pub fn Card(title: String, #[prop(default = None)] subtitle: Option<String>) -> Element {
        let _ = (title, subtitle);
        ui! { view() {} }
    }

    #[host_fn]
    pub fn sort(mut xs: Vec<u64>) -> Vec<u64> {
        xs.sort();
        xs
    }

    #[host_fn]
    pub fn save_prefs(prefs: Prefs) {
        let _ = prefs;
    }

    #[host_fn]
    pub fn greet(name: String) -> String {
        format!("hello {name}")
    }
}

/// v4: `Prefs` gains a field. `save_prefs` is spelled the same, so its
/// schema doesn't change — only its shape does.
mod v4 {
    use runtime_core::{component, host_fn, ui, Element, Remote};

    #[derive(Clone, Debug, PartialEq, Remote)]
    pub struct Prefs {
        pub dark: bool,
        pub size: u8,
    }

    #[component]
    pub fn Card(title: String, #[prop(default = None)] subtitle: Option<String>) -> Element {
        let _ = (title, subtitle);
        ui! { view() {} }
    }

    #[host_fn]
    pub fn sort(mut xs: Vec<u64>) -> Vec<u64> {
        xs.sort();
        xs
    }

    #[host_fn]
    pub fn save_prefs(prefs: Prefs) {
        let _ = prefs;
    }

    #[host_fn]
    pub fn greet(name: String) -> String {
        format!("hello {name}")
    }
}

const CARD: &str = "app::Card";
const PREFS: &str = "app::Prefs";

/// One host version, as the macros generated it.
struct Host {
    /// `module` of its `Card` registration (`fleet::v1`).
    module: &'static str,
    host_fns: Vec<(&'static str, HostFnDef)>,
    prefs: String,
}

impl Host {
    /// The prop shapes its `Card` registered (`host::APP_COMPONENTS`).
    fn card(&self) -> BTreeMap<String, String> {
        let name = format!("{}::Card", self.module);
        let entry = runtime_vocabulary::remote::host::APP_COMPONENTS
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("`{name}` registered"));
        (entry.props)().into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    /// Its manifest: what this version offers bundles.
    fn provides(&self) -> Provides {
        let mut p = Provides { codec: CODEC_VERSION, ..Default::default() };
        p.components.insert(CARD.into(), self.card());
        for (name, def) in &self.host_fns {
            p.host_fns.insert(import_name(&format!("app::{name}"), def.schema), (def.shape)());
        }
        p.contexts.insert(PREFS.into(), self.prefs.clone());
        p
    }

    /// What a release built against this version requires, using the
    /// card's `props` and the host functions `calls`.
    fn requires(&self, props: &[&str], calls: &[&str]) -> Requires {
        let mut r = Requires::default();
        let card = self.card();
        r.components.insert(CARD.into(), props.iter().map(|p| (p.to_string(), card[*p].clone())).collect());
        for call in calls {
            let (_, def) = self.host_fns.iter().find(|(n, _)| n == call).unwrap_or_else(|| panic!("{call} in this version"));
            r.host_fns.insert(import_name(&format!("app::{call}"), def.schema), (def.shape)());
        }
        // Every derived type compiled into the bundle is listed.
        r.contexts.insert(PREFS.into(), self.prefs.clone());
        r
    }
}

fn hosts() -> [Host; 4] {
    use runtime_vocabulary::__shape_of;
    [
        Host {
            module: "fleet::v1",
            host_fns: vec![("sort", v1::sort::export()), ("save_prefs", v1::save_prefs::export())],
            prefs: __shape_of!(v1::Prefs),
        },
        Host {
            module: "fleet::v2",
            host_fns: vec![("sort", v2::sort::export()), ("save_prefs", v2::save_prefs::export()), ("greet", v2::greet::export())],
            prefs: __shape_of!(v2::Prefs),
        },
        Host {
            module: "fleet::v3",
            host_fns: vec![("sort", v3::sort::export()), ("save_prefs", v3::save_prefs::export()), ("greet", v3::greet::export())],
            prefs: __shape_of!(v3::Prefs),
        },
        Host {
            module: "fleet::v4",
            host_fns: vec![("sort", v4::sort::export()), ("save_prefs", v4::save_prefs::export()), ("greet", v4::greet::export())],
            prefs: __shape_of!(v4::Prefs),
        },
    ]
}

/// A release: its version, its codec, its requirements.
fn release(version: &str, published: u64, codec: u32, requires: Requires) -> Release {
    Release {
        version: version.into(),
        file: Release::path_for("screens", version),
        sha256: version.into(),
        size: 1,
        codec,
        signed_by: None,
        published,
        requires,
    }
}

/// The run of releases, oldest first: each built against one host version
/// and using part of it.
fn releases(h: &[Host; 4]) -> Vec<Release> {
    vec![
        // Built against v1: title, sort, save_prefs.
        release("1.0", 10, CODEC_VERSION, h[0].requires(&["title"], &["sort", "save_prefs"])),
        // Against v2: the subtitle, and greet.
        release("1.1", 11, CODEC_VERSION, h[1].requires(&["title", "subtitle"], &["sort", "save_prefs", "greet"])),
        // Against v3: sort over u64.
        release("1.2", 12, CODEC_VERSION, h[2].requires(&["title"], &["sort", "save_prefs"])),
        // Against v4: the new Prefs.
        release("1.3", 13, CODEC_VERSION, h[3].requires(&["title"], &["sort", "save_prefs"])),
        // Against v4, using only the title: nothing version-specific.
        release("1.4", 14, CODEC_VERSION, h[3].requires(&["title"], &[])),
        // Built with a newer value codec.
        release("2.0", 20, CODEC_VERSION + 1, h[3].requires(&["title"], &[])),
    ]
}

/// The errors (not warnings) running `r` on `host` would hit.
fn errors(r: &Release, host: &Provides) -> Vec<Problem> {
    remote_bundle::check(&r.requires, r.codec, host).into_iter().filter(Problem::is_error).collect()
}

/// The shapes are the macros' own: what changed between versions shows up
/// in them, and what didn't, doesn't.
#[test]
fn each_version_changes_its_manifest_where_its_code_changed() {
    let h = hosts();
    let p: Vec<Provides> = h.iter().map(Host::provides).collect();
    assert_eq!(h[0].prefs, "Prefs{dark:bool}");
    assert_eq!(h[3].prefs, "Prefs{dark:bool,size:u8}");
    assert!(!p[0].components[CARD].contains_key("subtitle") && p[1].components[CARD].contains_key("subtitle"));
    let sort = |p: &Provides| p.host_fns.iter().find(|(k, _)| k.starts_with("app::sort#")).map(|(k, v)| (k.clone(), v.clone())).unwrap();
    assert_eq!(sort(&p[0]), sort(&p[1]), "v2 didn't touch sort");
    assert_ne!(sort(&p[1]).0, sort(&p[2]).0, "v3's u64 is a new schema");
    let save = |p: &Provides| p.host_fns.iter().find(|(k, _)| k.starts_with("app::save_prefs#")).map(|(k, v)| (k.clone(), v.clone())).unwrap();
    assert_eq!(save(&p[2]).0, save(&p[3]).0, "Prefs is spelled the same: the schema can't see the new field");
    assert_ne!(save(&p[2]).1, save(&p[3]).1, "the shape can");
    // Four different builds, four ids.
    let ids: std::collections::BTreeSet<String> = p.iter().map(Provides::id).collect();
    assert_eq!(ids.len(), 4);
}

/// The whole table: which release each version can run, and the reason
/// whenever it can't.
#[test]
fn the_fleet_runs_exactly_the_releases_built_for_what_it_has() {
    let h = hosts();
    let p: Vec<Provides> = h.iter().map(Host::provides).collect();
    let rs = releases(&h);
    let table: Vec<String> = p
        .iter()
        .enumerate()
        .map(|(i, host)| {
            let cells: Vec<&str> = rs.iter().map(|r| if errors(r, host).is_empty() { "✓" } else { "✗" }).collect();
            format!("v{} {}", i + 1, cells.join(" "))
        })
        .collect();
    //               1.0 1.1 1.2 1.3 1.4 2.0
    assert_eq!(
        table,
        [
            "v1 ✓ ✗ ✗ ✗ ✓ ✗", // lacks the subtitle and greet; sort and Prefs are older
            "v2 ✓ ✓ ✗ ✗ ✓ ✗",
            "v3 ✗ ✗ ✓ ✗ ✓ ✗", // its sort takes u64: bundles calling the u32 one fail
            "v4 ✗ ✗ ✗ ✓ ✓ ✗", // its Prefs has a field older bundles don't send
        ]
    );

    // Why, for the cells that tell the story.
    assert_eq!(
        errors(&rs[1], &p[0]),
        [
            Problem::MissingProp { component: CARD.into(), prop: "subtitle".into() },
            Problem::MissingHostFn { path: "app::greet".into(), changed: false },
        ],
        "1.1 on v1: what v2 added"
    );
    assert_eq!(
        errors(&rs[0], &p[2]),
        [Problem::MissingHostFn { path: "app::sort".into(), changed: true }],
        "1.0 on v3: sort exists, with another signature"
    );
    assert_eq!(
        errors(&rs[2], &p[3]),
        [Problem::HostFnShape {
            path: "app::save_prefs".into(),
            bundle: "fn(Prefs{dark:bool})->()".into(),
            app: "fn(Prefs{dark:bool,size:u8})->()".into(),
        }],
        "1.2 on v4: same schema, but the struct crosses differently"
    );
    assert_eq!(errors(&rs[5], &p[0]), [Problem::Codec { bundle: CODEC_VERSION + 1, app: CODEC_VERSION }]);
    // A struct that changed is also a warning wherever it is only listed.
    let warnings = remote_bundle::check(&rs[4].requires, rs[4].codec, &p[0]);
    assert!(warnings.iter().all(|w| !w.is_error()) && warnings.iter().any(|w| matches!(w, Problem::ContextShape { .. })));
}

/// What each version chooses (`ota_index::resolve`): the newest release it
/// can run, and whether a newer one needs an app update.
#[test]
fn each_version_chooses_the_newest_release_it_can_run() {
    let h = hosts();
    let p: Vec<Provides> = h.iter().map(Host::provides).collect();
    let rs = releases(&h);
    let index_of = |live: &[Release]| {
        let mut b = Bundle { releases: live.to_vec(), ..Bundle::default() };
        b.normalize();
        let mut i = Index::new();
        i.bundles.insert("screens".into(), b);
        i
    };
    let choices = |index: &Index| -> Vec<(Option<String>, bool)> {
        p.iter()
            .map(|host| {
                let r = &resolve(index, host).bundles["screens"];
                (r.release.as_ref().map(|r| r.version.clone()), r.needs_app_update)
            })
            .collect()
    };
    let v = |s: &str| Some(s.to_string());

    // 1.0 to 1.3 live: each version runs the one built for it; all but v4
    // learn a newer one needs an app update.
    assert_eq!(choices(&index_of(&rs[..4])), [(v("1.0"), true), (v("1.1"), true), (v("1.2"), true), (v("1.3"), false)]);

    // 1.4 uses only what every version has: everyone moves to it.
    assert_eq!(choices(&index_of(&rs[..5])), [(v("1.4"), false), (v("1.4"), false), (v("1.4"), false), (v("1.4"), false)]);

    // 2.0 (a newer codec) reaches no one: everyone keeps 1.4 and is told
    // an app update would bring it.
    assert_eq!(choices(&index_of(&rs)), [(v("1.4"), true), (v("1.4"), true), (v("1.4"), true), (v("1.4"), true)]);

    // Taking 1.4 down returns each version to its own.
    let without_14: Vec<Release> = rs.iter().filter(|r| r.version != "1.4").cloned().collect();
    assert_eq!(choices(&index_of(&without_14)), [(v("1.0"), true), (v("1.1"), true), (v("1.2"), true), (v("1.3"), true)]);

    // Only 1.2 and 1.3 live: v1 and v2 have nothing they can run.
    assert_eq!(choices(&index_of(&rs[2..4])), [(None, true), (None, true), (v("1.2"), true), (v("1.3"), false)]);
}

/// The fleet as a release location, to look at in the console: with
/// `IDEALYST_FLEET_OUT=<dir>`, publish the six releases there (as minimal
/// release builds carrying these requirements) and register the four
/// versions. Then `OTA_CONSOLE_LOCATION=<dir>` shows the grid. Skipped
/// otherwise.
#[test]
fn the_fleet_as_a_location_for_the_console() {
    let Some(out) = std::env::var_os("IDEALYST_FLEET_OUT") else {
        eprintln!("skipped: set IDEALYST_FLEET_OUT to a directory to write the fleet there");
        return;
    };
    let target = ota_publish::Target::Dir(out.into());
    let h = hosts();
    for r in releases(&h) {
        let meta = remote_bundle::Metadata { name: "screens".into(), package: "app".into(), version: r.version.clone(), codec: r.codec };
        let wasm = remote_bundle::with_metadata(b"\0asm\x01\0\0\0", &meta).unwrap();
        let wasm = remote_bundle::with_requires(&wasm, &r.requires).unwrap();
        ota_publish::publish(&target, &[ota_publish::Upload { name: "screens".into(), wasm }], r.published, "fleet").unwrap();
    }
    for (i, host) in h.iter().enumerate() {
        let label = format!("app v{}", i + 1);
        ota_publish::register(&target, &ota::index::Manifest::new(host.provides()), ota::index::ManifestSource::Build, Some(label), None).unwrap();
    }
}
