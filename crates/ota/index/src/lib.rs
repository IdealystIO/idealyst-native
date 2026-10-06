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
//!
//! The same decision can be made away from the app: [`resolve`] gives one
//! app's whole answer (a [`Resolution`]) from the index and the app's
//! manifest (`remote_bundle::Provides`), and an app that knows its
//! manifest's id can fetch that answer instead of the index:
//!
//! ```text
//! manifests.json                              ← builds registered from their build (Registry)
//! reported/<id>.json                          ← builds reported from the field, one marker each
//! manifests/<id>.json                         ← each one's manifest (Manifest)
//! resolved/<id>.json                          ← each one's answer, rewritten with the index
//! ```
//!
//! `resolve` is the only place the decision is made — the app, the
//! publisher's precomputed answers and a resolution service all call it,
//! so they can't disagree.

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
    /// Bumped by every write: which version of the index an answer
    /// ([`Resolution::generation`]) was computed from, so an older answer
    /// never overwrites a newer one. 0 in an index written before it was
    /// recorded.
    #[serde(default)]
    pub generation: u64,
    /// By bundle name.
    #[serde(default)]
    pub bundles: BTreeMap<String, Bundle>,
}

/// One bundle's history.
///
/// `releases` is in the order apps prefer them: newest first, except a
/// [pinned](Bundle::pinned) release, which comes first. Apps take the first
/// one they can run, so the ORDER is what every client follows — a client
/// that predates pinning still serves the pinned release. `pinned` itself
/// only tells the publisher and the console to keep it there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    /// What apps choose from, in preference order.
    #[serde(default)]
    pub releases: Vec<Release>,
    /// Releases taken down, most recently first: kept for the record and to
    /// restore, never chosen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withdrawn: Vec<Withdrawn>,
    /// The release served ahead of newer ones (its `sha256`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<String>,
}

/// A release taken down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Withdrawn {
    #[serde(flatten)]
    pub release: Release,
    /// When, seconds since the Unix epoch (0 for one taken down before this
    /// was recorded).
    #[serde(default)]
    pub withdrawn_at: u64,
    /// The kill switch: apps running it replace it at once — with the
    /// release they'd now choose, their built-in copy, or nothing — instead
    /// of at their next launch.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub urgent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Bundle {
    /// Put `releases` in preference order: newest first, the pinned one
    /// ahead of all.
    pub fn normalize(&mut self) {
        self.releases.sort_by(|a, b| b.published.cmp(&a.published));
        if let Some(pin) = &self.pinned {
            if let Some(i) = self.releases.iter().position(|r| &r.sha256 == pin) {
                let pinned = self.releases.remove(i);
                self.releases.insert(0, pinned);
            } else {
                self.pinned = None;
            }
        }
    }

    /// The urgent take-down of release `sha256`, if it was taken down that way.
    pub fn killed(&self, sha256: &str) -> Option<&Withdrawn> {
        self.withdrawn.iter().find(|w| w.urgent && w.release.sha256 == sha256)
    }
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
        Index { format: FORMAT, generation: 0, bundles: BTreeMap::new() }
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
/// `provides` can run ([`resolve`]'s choices).
pub fn choose(index: &Index, provides: &Provides) -> Vec<Choice> {
    resolve(index, provides).choices()
}

/// Where answers are stored: `resolved/<manifest id>.json`.
pub fn resolved_path(manifest: &str) -> String {
    format!("resolved/{manifest}.json")
}

/// One app's whole answer: for each bundle, what it runs and why. Made by
/// [`resolve`] wherever it runs — on the device from the index, by the
/// publisher into `resolved/<id>.json`, by a resolution service — so an
/// app can act on one it fetched exactly as on one it made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// [`FORMAT`] of the index it was made from.
    pub format: u32,
    /// The compatibility rule that decided it (`remote_bundle::RULE`): an
    /// app whose rule differs makes its own answer from the index.
    pub rule: u32,
    /// The manifest it answers (`Provides::id`).
    pub manifest: String,
    /// The index's [`generation`](Index::generation) it was made from.
    pub generation: u64,
    pub bundles: BTreeMap<String, Resolved>,
}

/// One bundle's part of a [`Resolution`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    /// The release to run: the first in preference order the app can run.
    pub release: Option<Release>,
    /// The bundle's first-choice release needs a newer app.
    pub needs_app_update: bool,
    /// Releases taken down with the kill switch (`sha256`): an app running
    /// one replaces it at once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub killed: Vec<String>,
    /// The remote components the bundle's first-choice release provides:
    /// which bundle to fetch when one of them is shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// Every live release in preference order, and whether the app can run
    /// it: for the console and for diagnosing an app that doesn't update.
    #[serde(default)]
    pub verdicts: Vec<Verdict>,
}

/// Whether an app can run one release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub version: String,
    pub sha256: String,
    /// Every mismatch, warnings included (`Problem::is_error`): none that
    /// is an error means it runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<remote_bundle::Problem>,
}

impl Verdict {
    pub fn runs(&self) -> bool {
        self.problems.iter().all(|p| !p.is_error())
    }
}

/// The app providing `provides`'s answer from `index`: per bundle, each
/// live release's [`Verdict`] and the first one it can run.
pub fn resolve(index: &Index, provides: &Provides) -> Resolution {
    let bundles = index
        .bundles
        .iter()
        .map(|(name, bundle)| {
            let verdicts: Vec<Verdict> = bundle
                .releases
                .iter()
                .map(|r| Verdict {
                    version: r.version.clone(),
                    sha256: r.sha256.clone(),
                    problems: remote_bundle::check(&r.requires, r.codec, provides),
                })
                .collect();
            let at = verdicts.iter().position(Verdict::runs);
            let resolved = Resolved {
                release: at.map(|i| bundle.releases[i].clone()),
                needs_app_update: at != Some(0) && !bundle.releases.is_empty(),
                killed: bundle.withdrawn.iter().filter(|w| w.urgent).map(|w| w.release.sha256.clone()).collect(),
                components: bundle.releases.first().map(|r| r.components().map(str::to_string).collect()).unwrap_or_default(),
                verdicts,
            };
            (name.clone(), resolved)
        })
        .collect();
    Resolution { format: FORMAT, rule: remote_bundle::RULE, manifest: provides.id(), generation: index.generation, bundles }
}

impl Resolution {
    /// Read an answer; one in a newer format is refused.
    pub fn parse(json: &[u8]) -> Result<Resolution, String> {
        let r: Resolution = serde_json::from_slice(json).map_err(|e| format!("the update answer doesn't parse: {e}"))?;
        if r.format > FORMAT {
            return Err(format!("the update answer is format {}, this app reads format {FORMAT}", r.format));
        }
        Ok(r)
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("an answer serializes")
    }

    /// Whether an app with manifest id `manifest` may act on this answer:
    /// made for it, under the same rule.
    pub fn answers(&self, manifest: &str) -> bool {
        self.manifest == manifest && self.rule == remote_bundle::RULE
    }

    /// Each bundle's [`Choice`].
    pub fn choices(&self) -> Vec<Choice> {
        self.bundles
            .iter()
            .map(|(name, r)| Choice { bundle: name.clone(), release: r.release.clone(), needs_app_update: r.needs_app_update })
            .collect()
    }

    /// The bundle whose first-choice release provides remote component
    /// `component` (as [`Index::bundle_for`]).
    pub fn bundle_for(&self, component: &str) -> Option<&str> {
        self.bundles.iter().find(|(_, r)| r.components.iter().any(|c| c == component)).map(|(name, _)| name.as_str())
    }

    /// Whether release `sha256` of `bundle` was taken down with the kill
    /// switch.
    pub fn killed(&self, bundle: &str, sha256: &str) -> bool {
        self.bundles.get(bundle).is_some_and(|r| r.killed.iter().any(|k| k == sha256))
    }
}

/// The list of registered app builds, at the location's root.
pub const MANIFESTS_FILE: &str = "manifests.json";

/// Where a manifest is stored: `manifests/<id>.json`.
pub fn manifest_path(id: &str) -> String {
    format!("manifests/{id}.json")
}

/// Where builds reported from the field are marked: `reported/<id>.json`,
/// one file each, so reports never contend for a shared file.
pub const REPORTED_DIR: &str = "reported/";

/// The marker for a build reported from the field.
pub fn reported_path(id: &str) -> String {
    format!("{REPORTED_DIR}{id}.json")
}

/// A stored app manifest: what an app build offers bundles, under its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub rule: u32,
    /// `provides.id()`.
    pub id: String,
    pub provides: Provides,
}

impl Manifest {
    pub fn new(provides: Provides) -> Manifest {
        Manifest { rule: remote_bundle::RULE, id: provides.id(), provides }
    }

    /// Read one, checking that its id is its content's: a manifest sent by
    /// an app is never trusted to name itself.
    pub fn parse(json: &[u8]) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_slice(json).map_err(|e| format!("the manifest doesn't parse: {e}"))?;
        m.verify()?;
        Ok(m)
    }

    /// Its id matches its content, under this rule.
    pub fn verify(&self) -> Result<(), String> {
        if self.rule != remote_bundle::RULE {
            return Err(format!("the manifest is for compatibility rule {}, this is rule {}", self.rule, remote_bundle::RULE));
        }
        let actual = self.provides.id();
        if actual != self.id {
            return Err(format!("the manifest's id is {}, its content's is {actual}", self.id));
        }
        Ok(())
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("a manifest serializes")
    }
}

/// The app builds a location knows ([`MANIFESTS_FILE`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub manifests: Vec<Registered>,
}

/// One known app build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    /// Its manifest's id.
    pub id: String,
    pub source: ManifestSource,
    /// What to call it: `my-app 2.4.1 (ios)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// When it was registered, seconds since the Unix epoch.
    pub registered_at: u64,
}

/// How a manifest became known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManifestSource {
    /// Captured from the app's build (`idealyst ota manifest`): known before
    /// any install runs it, and its answer is precomputed.
    Build,
    /// Sent by an installed app to a resolution service: seen in the field.
    Reported,
}

impl Registry {
    pub fn parse(json: &[u8]) -> Result<Registry, String> {
        serde_json::from_slice(json).map_err(|e| format!("the manifest list doesn't parse: {e}"))
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("the manifest list serializes")
    }

    pub fn get(&self, id: &str) -> Option<&Registered> {
        self.manifests.iter().find(|m| m.id == id)
    }
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
            Bundle { releases: vec![release("3", "subtitle"), release("2", "title"), release("1", "title")], ..Bundle::default() },
        );
        i
    }

    /// A pinned release leads the order, whatever its age; a pin naming a
    /// release that is gone is dropped.
    #[test]
    fn normalizing_puts_the_pinned_release_first() {
        let mut b = Bundle::default();
        for (v, t) in [("1", 10), ("3", 30), ("2", 20)] {
            b.releases.push(Release { published: t, ..release(v, "title") });
        }
        b.normalize();
        assert_eq!(b.releases.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["3", "2", "1"]);
        b.pinned = Some("1".into());
        b.normalize();
        assert_eq!(b.releases.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["1", "3", "2"]);
        b.pinned = Some("gone".into());
        b.normalize();
        assert_eq!((b.releases[0].version.as_str(), b.pinned.clone()), ("3", None));
    }

    /// An index written before take-downs recorded when and how (a plain
    /// release under `withdrawn`) still reads.
    #[test]
    fn an_older_withdrawn_entry_still_parses() {
        let mut old = serde_json::to_value(index()).unwrap();
        old["bundles"]["shop"]["withdrawn"] = serde_json::json!([serde_json::to_value(release("0", "title")).unwrap()]);
        let parsed = Index::parse(&serde_json::to_vec(&old).unwrap()).unwrap();
        let w = &parsed.bundles["shop"].withdrawn[0];
        assert_eq!((w.release.version.as_str(), w.withdrawn_at, w.urgent), ("0", 0, false));
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

    /// The whole answer names, per release, why an app can't run it; the
    /// choice is the first release it can; urgent take-downs travel with
    /// it; and it survives the trip as JSON to the app unchanged.
    #[test]
    fn resolving_explains_every_release() {
        let mut i = index();
        i.generation = 7;
        let shop = i.bundles.get_mut("shop").unwrap();
        shop.withdrawn.push(Withdrawn { release: release("0", "title"), withdrawn_at: 1, urgent: true, reason: None });
        shop.withdrawn.push(Withdrawn { release: release("00", "title"), withdrawn_at: 1, urgent: false, reason: None });

        let old_app = app(&["title"]);
        let r = resolve(&i, &old_app);
        assert_eq!((r.manifest.as_str(), r.rule, r.generation), (old_app.id().as_str(), remote_bundle::RULE, 7));
        let shop = &r.bundles["shop"];
        assert_eq!(shop.release.as_ref().unwrap().version, "2");
        assert!(shop.needs_app_update);
        assert_eq!(shop.killed, ["0"], "only the kill switch's take-downs");
        assert_eq!(shop.components, ["shop::Offer"]);
        let runs: Vec<(&str, bool)> = shop.verdicts.iter().map(|v| (v.version.as_str(), v.runs())).collect();
        assert_eq!(runs, [("3", false), ("2", true), ("1", true)]);
        assert_eq!(
            shop.verdicts[0].problems,
            [remote_bundle::Problem::MissingProp { component: "ui::Card".into(), prop: "subtitle".into() }]
        );

        assert_eq!(r.choices(), choose(&i, &old_app));
        assert_eq!((r.bundle_for("shop::Offer"), r.bundle_for("nope")), (Some("shop"), None));
        assert!(r.killed("shop", "0") && !r.killed("shop", "00") && !r.killed("other", "0"));
        assert_eq!(Resolution::parse(&r.to_json()).unwrap(), r);
    }

    /// An app acts only on an answer made for its own manifest, under its
    /// own rule: anything else (another build's file, a newer rule) falls
    /// back to resolving from the index.
    #[test]
    fn an_answer_serves_only_the_manifest_and_rule_it_was_made_for() {
        let r = resolve(&index(), &app(&["title"]));
        assert!(r.answers(&app(&["title"]).id()));
        assert!(!r.answers(&app(&["title", "subtitle"]).id()));
        let other_rule = Resolution { rule: remote_bundle::RULE + 1, ..r.clone() };
        assert!(!other_rule.answers(&app(&["title"]).id()));
        let newer = Resolution { format: FORMAT + 1, ..r };
        assert!(Resolution::parse(&newer.to_json()).unwrap_err().contains("format"));
    }

    /// A manifest's id is checked against its content: one sent with
    /// another's id is refused, not stored under it.
    #[test]
    fn a_manifest_is_refused_under_an_id_not_its_own() {
        let m = Manifest::new(app(&["title"]));
        assert_eq!(Manifest::parse(&m.to_json()).unwrap(), m);
        let forged = Manifest { id: app(&["subtitle"]).id(), ..m.clone() };
        assert!(Manifest::parse(&forged.to_json()).unwrap_err().contains("its content's is"));
        let other_rule = Manifest { rule: remote_bundle::RULE + 1, ..m };
        assert!(Manifest::parse(&other_rule.to_json()).unwrap_err().contains("rule"));
    }

    /// An index written before generations were recorded reads as 0.
    #[test]
    fn an_index_without_a_generation_reads_as_zero() {
        let mut old = serde_json::to_value(index()).unwrap();
        old.as_object_mut().unwrap().remove("generation");
        assert_eq!(Index::parse(&serde_json::to_vec(&old).unwrap()).unwrap().generation, 0);
    }

    #[test]
    fn a_newer_index_format_is_refused() {
        let mut i = index();
        i.format = FORMAT + 1;
        assert!(Index::parse(&i.to_json()).unwrap_err().contains("format"));
        assert_eq!(Index::parse(&index().to_json()).unwrap(), index());
    }
}
