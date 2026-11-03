//! App-component props crossing from a bundle (`ImportArg` / `ImportProps`)
//! and as plain data (`RemoteValue`), both halves in one process on the
//! loopback engine — plus the static crossing check (`CrossCheck`) that
//! finds a prop type that can't cross before a bundle sets it.
//!
//! The app component here is declared with the vocabulary's emission
//! macros directly (`__remote_props!` / `__remote_app_component!` and, on
//! the bundle side, `__remote_import!`): they are exactly what `#[props]` /
//! `#[component]` expand to, and the component macros themselves emit
//! `::runtime_core` paths this crate's tests can't name.
//!
//! ```sh
//! cargo test -p runtime-vocabulary --features remote-loopback --test remote_props
//! ```

#![cfg(feature = "remote-loopback")]

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::Element;
use runtime_shared::assets::{kinds, Asset, AssetId, AssetSource};
use runtime_shared::primitives::image::ImageSource;
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{image_from, IntoElement, Reactive};
use runtime_vocabulary::remote::host::{decode, uncrossable_app_props, Link};
use runtime_vocabulary::remote::{bundle, to_bytes, uncrossable_import, uncrossable_value, Cb, RemoteValue};

struct InProc;

impl Link for InProc {
    fn call(&self, cb: Cb, args: &[u8]) -> Option<Vec<u8>> {
        Some(bundle::invoke(cb, args))
    }
    fn release(&self, cb: Cb) {
        bundle::release(cb)
    }
}

/// Encode as a bundle would, decode as the host would.
fn cross(element: Element) -> Element {
    decode(Rc::new(InProc), &to_bytes(&bundle::tree(element))).expect("decodes")
}

/// A row item like idea-ui's `Tab` / `MenuEntry` / `RadioOption`: an id, and
/// a robot test id that is a literal.
#[derive(Clone, Debug, PartialEq, runtime_macros::Remote)]
pub struct Row {
    pub id: String,
    pub test_id: Option<&'static str>,
}

/// Shaped like idea-ui's `ButtonProps` / `ImageProps` after idea-ui 16.
pub struct TaggedProps {
    pub label: Reactive<String>,
    pub test_id: Option<&'static str>,
    pub src: ImageSource,
    pub rows: Vec<Row>,
}

runtime_vocabulary::__remote_props!(TaggedProps {
    label: Reactive<String>,
    test_id: Option<&'static str>,
    src: ImageSource,
    rows: Vec<Row>,
});

/// What the app's copy of the component received, for the assertions.
struct Received {
    test_id: Option<&'static str>,
    src: ImageSource,
    rows: Vec<Row>,
}

thread_local! {
    static RECEIVED: RefCell<Option<Received>> = const { RefCell::new(None) };
}

fn tagged(p: TaggedProps) -> Element {
    RECEIVED.with(|r| *r.borrow_mut() = Some(Received { test_id: p.test_id, src: p.src.clone(), rows: p.rows.clone() }));
    view()
        .child(text().content(p.label.get()).build())
        .child(image_from(p.src).into_element())
        .build()
}

fn defaults() -> TaggedProps {
    TaggedProps { label: Reactive::Static(String::new()), test_id: None, src: ImageSource::from(""), rows: Vec::new() }
}

runtime_vocabulary::__remote_app_component!("remote_props::Tagged", TaggedProps, defaults(), |p| tagged(p));

/// What the bundle side of `Tagged(…)` in a remote component's `ui!` does:
/// send the props the call site set, import the app's copy by name.
fn import_tagged(props: TaggedProps, set: &'static [&'static str]) -> Element {
    runtime_vocabulary::__remote_import!("remote_props::Tagged", TaggedProps, props, Some(set))
}

static PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn png_asset() -> Asset<kinds::Image> {
    Asset::new(AssetId(0x5eed), AssetSource::Embedded { bytes: &PNG, extension: "png" })
}

/// Regression: idea-ui 16 gave every interactive component (and the row
/// items `Tab`, `MenuEntry`, `RadioOption`, `SelectOption`,
/// `SegmentOption`) a `test_id: Option<&'static str>`, and idea-ui's
/// `Image.src` became an `ImageSource` — neither type could cross, so every
/// remote component using `Button`, `Stack`, `Tabs`, `Menu`, `RadioGroup`
/// or `Image` failed to load ("a prop of type `Option<&str>` can't cross
/// from a remote component yet"). The same component now imports with its
/// test id, its rows' test ids, and an asset image source (bytes included).
#[test]
fn regression_idea_ui_16_test_id_and_image_source_props_cross_from_a_bundle() {
    let h = Harness::new();
    let tree = h.world.enter(|| {
        import_tagged(
            TaggedProps {
                label: Reactive::Static("Save".into()),
                test_id: Some("save-button"),
                src: ImageSource::from(png_asset()),
                rows: vec![Row { id: "a".into(), test_id: Some("row-a") }, Row { id: "b".into(), test_id: None }],
            },
            &["label", "test_id", "src", "rows"],
        )
    });
    let realized = h.mount(cross(tree));
    h.flush();
    let shown = h.tree(realized.collect_nodes()[0]);
    assert!(shown.contains(r#"text "Save""#), "{shown}");
    assert!(!shown.contains('⚠'), "{shown}");

    let got = RECEIVED.with(|r| r.borrow_mut().take()).expect("the app built its copy");
    assert_eq!(got.test_id, Some("save-button"));
    assert_eq!(got.rows, vec![Row { id: "a".into(), test_id: Some("row-a") }, Row { id: "b".into(), test_id: None }]);
    match got.src {
        ImageSource::Asset(a) => {
            assert_eq!(a.id, AssetId(0x5eed));
            match a.source {
                AssetSource::Embedded { bytes, extension } => {
                    assert_eq!(bytes, &PNG[..]);
                    assert_eq!(extension, "png");
                }
                other => panic!("the asset's source changed kind: {other:?}"),
            }
        }
        _ => panic!("the image source changed kind"),
    }
}

/// A URL source crosses as the `Reactive<String>` it is: a live one stays
/// live (a getter into the bundle), and the app's image follows it.
#[test]
fn a_live_image_url_crosses_as_a_getter_into_the_bundle() {
    let h = Harness::new();
    let url = h.world.enter(|| runtime_world::signal("https://example.com/a.png".to_string()));
    let tree = h.world.enter(|| {
        import_tagged(
            TaggedProps {
                src: ImageSource::from(url),
                ..defaults()
            },
            &["src"],
        )
    });
    let realized = h.mount(cross(tree));
    h.flush();
    let root = realized.collect_nodes()[0];
    assert!(h.tree(root).contains(r#"image "https://example.com/a.png""#), "{}", h.tree(root));
    h.take_log();
    h.world.enter(|| url.set("https://example.com/b.png".into()));
    h.flush();
    let log = h.take_log().join("\n");
    assert!(log.contains("https://example.com/b.png"), "the app's image didn't follow the bundle's url:\n{log}");
    drop(realized);
}

/// `&'static str` as plain data (a signal holding rows, a callback's
/// argument): the receiving side interns it — equal text, one allocation.
#[test]
fn a_static_str_crosses_as_plain_data_and_is_interned() {
    let row = Row { id: "x".into(), test_id: Some("tab-x") };
    let mut bytes = Vec::new();
    row.encode(&mut bytes);
    let a = Row::decode(&mut &bytes[..]).expect("decodes");
    let b = Row::decode(&mut &bytes[..]).expect("decodes");
    assert_eq!(a, row);
    assert!(std::ptr::eq(a.test_id.unwrap(), b.test_id.unwrap()), "each distinct string is leaked once");

    // The same bytes as a `String`: its shape is `str` either way.
    let mut as_string = Vec::new();
    "tab-x".to_string().encode(&mut as_string);
    let mut as_static = Vec::new();
    "tab-x".encode(&mut as_static);
    assert_eq!(as_string, as_static);
}

#[test]
fn an_image_source_crosses_as_plain_data() {
    for src in [ImageSource::from("https://example.com/a.png"), ImageSource::from(png_asset())] {
        let mut bytes = Vec::new();
        src.encode(&mut bytes);
        let back = ImageSource::decode(&mut &bytes[..]).expect("decodes");
        match (src, back) {
            (ImageSource::Url(a), ImageSource::Url(b)) => assert_eq!(a.get(), b.get()),
            (ImageSource::Asset(a), ImageSource::Asset(b)) => {
                assert_eq!(a.id, b.id);
                assert_eq!(format!("{:?}", a.source), format!("{:?}", b.source));
            }
            _ => panic!("the variant changed"),
        }
    }
    // A variant this side has no encoding for is refused by name.
    let mut bytes = Vec::new();
    runtime_vocabulary::remote::__send_value(&2u8, &mut bytes);
    runtime_vocabulary::remote::__send_value(&"Decoded", &mut bytes);
    let err = ImageSource::decode(&mut &bytes[..]).err().expect("refused");
    assert!(err.contains("`Decoded`"), "{err}");
}

// ---- the static crossing check ----

/// A field type with no crossing. The derive still compiles (the probe
/// defers the error to the moment a value crosses); the check names it.
#[derive(PartialEq)]
pub struct Opaque;

#[derive(PartialEq, runtime_macros::Remote)]
pub struct WithOpaque {
    pub ok: String,
    pub bad: Option<Rc<Opaque>>,
}

/// A derived type that contains itself: the walk must terminate.
#[derive(Clone, PartialEq, runtime_macros::Remote)]
pub struct Tree {
    pub label: &'static str,
    pub children: Vec<Tree>,
}

#[test]
fn the_crossing_check_names_a_field_that_cannot_cross() {
    let failed = uncrossable_import::<Vec<WithOpaque>>();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("Option<alloc::rc::Rc<remote_props::Opaque>>"), "{failed:?}");
    assert!(failed[0].contains("in remote_props::WithOpaque"), "{failed:?}");
    // As plain data, through a signal of rows — the path a `Tabs(tabs = …)`
    // signal takes.
    let failed = uncrossable_import::<runtime_world::Signal<Vec<WithOpaque>>>();
    assert!(failed.iter().any(|f| f.contains("as plain data")), "{failed:?}");
}

#[test]
fn the_crossing_check_passes_the_types_that_cross() {
    assert_eq!(uncrossable_import::<Option<&'static str>>(), Vec::<String>::new());
    assert_eq!(uncrossable_import::<ImageSource>(), Vec::<String>::new());
    assert_eq!(uncrossable_import::<Reactive<runtime_shared::primitives::text_input::KeyboardKind>>(), Vec::<String>::new());
    assert_eq!(uncrossable_import::<Vec<Row>>(), Vec::<String>::new());
    assert_eq!(uncrossable_value::<Vec<Row>>(), Vec::<String>::new());
    assert_eq!(uncrossable_value::<Tree>(), Vec::<String>::new(), "a recursive type is walked once");
}

/// An app component with a prop that can't cross, for the registry sweep.
pub struct HoldsOpaqueProps {
    pub raw: Rc<Opaque>,
    pub item: WithOpaque,
}

runtime_vocabulary::__remote_props!(HoldsOpaqueProps { raw: Rc<Opaque>, item: WithOpaque });
runtime_vocabulary::__remote_app_component!(
    "remote_props::HoldsOpaque",
    HoldsOpaqueProps,
    HoldsOpaqueProps { raw: Rc::new(Opaque), item: WithOpaque { ok: String::new(), bad: None } },
    |p| {
        let _ = (p.raw, p.item.ok, p.item.bad);
        view().build()
    }
);

/// The registry sweep a component library's test asserts on: every
/// registered app component, and what in its props can't cross.
#[test]
fn every_registered_app_component_reports_its_uncrossable_props() {
    let ours: Vec<_> = uncrossable_app_props().into_iter().filter(|(n, _)| n.starts_with("remote_props::")).collect();
    assert_eq!(ours.len(), 1, "only `HoldsOpaque` has a prop that can't cross: {ours:?}");
    let (name, failed) = &ours[0];
    assert_eq!(*name, "remote_props::HoldsOpaque");
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(failed[0].contains("alloc::rc::Rc<remote_props::Opaque>") && !failed[0].contains(" (in "), "{failed:?}");
    assert!(failed[1].contains("in remote_props::WithOpaque"), "{failed:?}");
}
