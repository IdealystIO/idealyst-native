//! The files of an over-the-air release, and the choices made from them.
//!
//! A release location (an S3 prefix behind a CDN, a directory) holds:
//!
//! ```text
//! index.json                                  ← rewritten on every publish
//! bundles/<name>/<sha256>.wasm                ← never change: cache forever
//! ```
//!
//! [`Index`] lists, per bundle, its recent [`Release`]s, newest first, each
//! with what it requires of an app (`remote_bundle::Requires`). There is no
//! per-app file and no server logic: an app reads the index and runs the
//! newest release of each bundle it can run, [`choose`], by checking each
//! release's requirements against what it provides itself. A release that
//! needs a newer app is skipped on the apps that lack it, and they learn
//! that one exists ([`Choice::needs_app_update`]).

use std::collections::BTreeMap;

use remote_bundle::{Provides, Requires};
use serde::{Deserialize, Serialize};

/// The index's file name, at the release location's root.
pub const INDEX_FILE: &str = "index.json";
/// The index format this crate reads and writes.
pub const FORMAT: u32 = 1;
/// How many releases of a bundle the index keeps. An app older than all of
/// them gets none of that bundle's updates (it keeps what it has).
pub const KEEP: usize = 20;

/// Everything published.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub format: u32,
    /// By bundle name.
    #[serde(default)]
    pub bundles: BTreeMap<String, Bundle>,
}

/// One bundle's history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    /// Newest first: what apps choose from.
    #[serde(default)]
    pub releases: Vec<Release>,
    /// Releases taken back (`ota rollback`), newest first: kept for the
    /// record and to restore, never chosen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withdrawn: Vec<Release>,
}

/// One published build of a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// The bundle crate's version when it was built.
    pub version: String,
    /// Its path from the release location's root (`bundles/shop/<sha>.wasm`).
    pub file: String,
    pub sha256: String,
    pub size: u64,
    /// The value codec it was built with.
    pub codec: u32,
    /// The id of the key that signed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
    /// When it was published, seconds since the Unix epoch.
    pub published: u64,
    /// What it requires of an app; its remote components are
    /// `requires.remote`'s keys.
    pub requires: Requires,
}

impl Release {
    /// Whether an app providing `provides` can run it.
    pub fn runs_on(&self, provides: &Provides) -> bool {
        remote_bundle::check(&self.requires, self.codec, provides).iter().all(|p| !p.is_error())
    }

    /// The remote components it provides.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.requires.remote.keys().map(String::as_str)
    }

    /// Where a bundle with this hash is stored.
    pub fn path_for(bundle: &str, sha256: &str) -> String {
        format!("bundles/{bundle}/{sha256}.wasm")
    }
}

impl Index {
    pub fn new() -> Index {
        Index { format: FORMAT, bundles: BTreeMap::new() }
    }

    /// Read an index; one in a newer format is refused rather than misread.
    pub fn parse(json: &[u8]) -> Result<Index, String> {
        let index: Index = serde_json::from_slice(json).map_err(|e| format!("the update index doesn't parse: {e}"))?;
        if index.format > FORMAT {
            return Err(format!("the update index is format {}, this app reads format {FORMAT}", index.format));
        }
        Ok(index)
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("an index serializes")
    }

    /// The bundle whose newest release provides remote component
    /// `component`.
    pub fn bundle_for(&self, component: &str) -> Option<&str> {
        self.bundles
            .iter()
            .find(|(_, b)| b.releases.first().is_some_and(|r| r.components().any(|c| c == component)))
            .map(|(name, _)| name.as_str())
    }
}

/// What an app runs of one bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub bundle: String,
    /// The newest release it can run; `None` when it can run none.
    pub release: Option<Release>,
    /// A newer release exists that this app can't run: it needs an app
    /// update to get it.
    pub needs_app_update: bool,
}

/// For each bundle in `index`, the newest release an app providing
/// `provides` can run.
pub fn choose(index: &Index, provides: &Provides) -> Vec<Choice> {
    index
        .bundles
        .iter()
        .map(|(name, bundle)| {
            let at = bundle.releases.iter().position(|r| r.runs_on(provides));
            Choice {
                bundle: name.clone(),
                release: at.map(|i| bundle.releases[i].clone()),
                needs_app_update: at != Some(0) && !bundle.releases.is_empty(),
            }
        })
        .collect()
}

/// What `next` requires that `previous` didn't: each new component, prop,
/// host function or remote component parameter list, and each that changed
/// type. An app that had what `previous` needed may lack these, and would
/// keep the previous release.
pub fn new_requirements(previous: &Requires, next: &Requires) -> Vec<String> {
    let mut out = Vec::new();
    for (component, props) in &next.components {
        let before = previous.components.get(component);
        for (prop, shape) in props {
            match before.and_then(|b| b.get(prop)) {
                Some(old) if old == shape => {}
                Some(old) => out.push(format!("`{component}.{prop}` is now `{shape}` (was `{old}`)")),
                None if before.is_none() => {
                    out.push(format!("app component `{component}`"));
                    break;
                }
                None => out.push(format!("prop `{component}.{prop}`")),
            }
        }
        if props.is_empty() && before.is_none() {
            out.push(format!("app component `{component}`"));
        }
    }
    for (import, shape) in &next.host_fns {
        if previous.host_fns.contains_key(import) && previous.host_fns[import] == *shape {
            continue;
        }
        let path = import.rsplit_once('#').map_or(import.as_str(), |(p, _)| p);
        let existed = previous.host_fns.keys().any(|k| k.rsplit_once('#').is_some_and(|(p, _)| p == path));
        out.push(if existed {
            format!("host function `{path}` with a new signature (`{shape}`)")
        } else {
            format!("host function `{path}`")
        });
    }
    for (component, params) in &next.remote {
        if previous.remote.get(component).is_some_and(|p| p != params) {
            out.push(format!("new parameters for remote component `{component}`"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use remote_bundle::manifest::Param;

    fn release(version: &str, prop: &str) -> Release {
        let mut requires = Requires::default();
        requires.components.insert("ui::Card".into(), [(prop.to_string(), "str".to_string())].into());
        requires.remote.insert("shop::Offer".into(), vec![Param { name: "id".into(), shape: "u64".into() }]);
        Release {
            version: version.into(),
            file: Release::path_for("shop", version),
            sha256: version.into(),
            size: 1,
            codec: 2,
            signed_by: None,
            published: 0,
            requires,
        }
    }

    fn app(props: &[&str]) -> Provides {
        let mut p = Provides { codec: 2, ..Default::default() };
        p.components.insert("ui::Card".into(), props.iter().map(|p| (p.to_string(), "str".to_string())).collect());
        p
    }

    fn index() -> Index {
        let mut i = Index::new();
        // Newest first: 3 needs `subtitle`, 2 and 1 only `title`.
        i.bundles.insert(
            "shop".into(),
            Bundle { releases: vec![release("3", "subtitle"), release("2", "title"), release("1", "title")], withdrawn: vec![] },
        );
        i
    }

    #[test]
    fn an_app_runs_the_newest_release_it_can() {
        let new_app = choose(&index(), &app(&["title", "subtitle"]));
        assert_eq!(new_app[0].release.as_ref().unwrap().version, "3");
        assert!(!new_app[0].needs_app_update);

        let old_app = choose(&index(), &app(&["title"]));
        assert_eq!(old_app[0].release.as_ref().unwrap().version, "2", "skips the release it can't run");
        assert!(old_app[0].needs_app_update, "and learns a newer one exists");

        let ancient = choose(&index(), &app(&[]));
        assert_eq!(ancient[0].release, None);
        assert!(ancient[0].needs_app_update);
    }

    #[test]
    fn the_bundle_for_a_component_is_found_by_its_newest_release() {
        assert_eq!(index().bundle_for("shop::Offer"), Some("shop"));
        assert_eq!(index().bundle_for("shop::Missing"), None);
    }

    #[test]
    fn new_requirements_name_what_older_apps_may_lack() {
        let mut next = release("4", "title").requires;
        next.components.get_mut("ui::Card").unwrap().insert("elevation".into(), "u8".into());
        next.components.insert("ui::Chart".into(), [("data".to_string(), "list<f64>".to_string())].into());
        next.host_fns.insert("app::sort#01".into(), "fn()->()".into());
        let got = new_requirements(&release("3", "title").requires, &next);
        assert_eq!(got, ["prop `ui::Card.elevation`", "app component `ui::Chart`", "host function `app::sort`"]);
        assert!(new_requirements(&next, &next).is_empty());
    }

    #[test]
    fn a_newer_index_format_is_refused() {
        let mut i = index();
        i.format = FORMAT + 1;
        assert!(Index::parse(&i.to_json()).unwrap_err().contains("format"));
        assert_eq!(Index::parse(&index().to_json()).unwrap(), index());
    }
}
