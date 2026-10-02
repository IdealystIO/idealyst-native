//! The style data model's serde form (feature `remote-serde`): a remote
//! component's style rules cross to the host app with TOKEN NAMES intact,
//! so the app's own theme resolves them.

#![cfg(feature = "remote-serde")]

use runtime_shared::{Color, Length, StyleRules, Tokenized, VariantSet};

fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(v: &T) -> T {
    serde_json::from_slice(&serde_json::to_vec(v).unwrap()).unwrap()
}

#[test]
fn style_rules_round_trip_with_token_names_intact() {
    let rules = StyleRules {
        background: Some(Tokenized::Token { name: "color.surface", fallback: Color("#ffffff".into()) }),
        color: Some(Tokenized::Literal(Color("#112233".into()))),
        padding_top: Some(Tokenized::Token { name: "space.md", fallback: Length::Px(12.0) }),
        width: Some(Tokenized::Literal(Length::Percent(50.0))),
        ..StyleRules::default()
    };
    let back = round_trip(&rules);
    assert_eq!(back, rules);
    match back.background {
        Some(Tokenized::Token { name, .. }) => assert_eq!(name, "color.surface", "the token NAME crossed, not a resolved value"),
        other => panic!("token lost: {other:?}"),
    }
}

#[test]
fn interned_token_names_are_shared_not_leaked_per_decode() {
    let rules = StyleRules {
        background: Some(Tokenized::Token { name: "color.accent", fallback: Color("#000".into()) }),
        ..StyleRules::default()
    };
    let name_ptr = |r: &StyleRules| match &r.background {
        Some(Tokenized::Token { name, .. }) => name.as_ptr(),
        _ => unreachable!(),
    };
    let a = round_trip(&rules);
    let b = round_trip(&rules);
    assert_eq!(name_ptr(&a), name_ptr(&b), "one interned string per distinct token name");
}

#[test]
fn variant_sets_round_trip() {
    let mut v = VariantSet::new();
    v.0.insert("tone".into(), "danger".into());
    v.0.insert("hovered".into(), "on".into());
    assert_eq!(round_trip(&v), v);
}

// ---------------------------------------------------------------------------
// Proxied sheets: a bundle's StyleSheet rebuilt on the host from its SHAPE,
// every closure answered by the bundle. Everything crossing is serialized
// here, exactly as it crosses the wasm boundary.
// ---------------------------------------------------------------------------

use runtime_shared::{SheetPart, SheetShape, StyleSheet};
use std::rc::Rc;

fn token(name: &'static str, hex: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::Token { name, fallback: Color(hex.into()) })
}

/// A sheet using every kind of part: a base that reads the variant set, an
/// author axis with a default, a state axis, a breakpoint axis, a container
/// axis, and a compound across an author and a state axis.
fn bundle_sheet() -> StyleSheet {
    StyleSheet::new(|vs| StyleRules {
        background: token("color.surface", "#fff"),
        width: (vs.0.get("size").map(String::as_str) == Some("wide")).then_some(Tokenized::Literal(Length::Percent(100.0))),
        ..StyleRules::default()
    })
    .variant("tone", "neutral", |_| StyleRules { color: token("color.text", "#111"), ..StyleRules::default() })
    .variant("tone", "danger", |_| StyleRules { color: token("color.danger", "#c00"), ..StyleRules::default() })
    .variant_default("tone", "neutral")
    .variant("size", "wide", |_| StyleRules { padding_top: Some(Tokenized::Literal(Length::Px(4.0))), ..StyleRules::default() })
    .variant("__state_hovered", "on", |_| StyleRules { background: token("color.hover", "#eee"), ..StyleRules::default() })
    .variant("__bp_md", "on", |_| StyleRules { padding_top: Some(Tokenized::Literal(Length::Px(16.0))), ..StyleRules::default() })
    .variant("__cq_minw_400", "on", |_| StyleRules { padding_top: Some(Tokenized::Literal(Length::Px(24.0))), ..StyleRules::default() })
    .compound(vec![("tone", "danger"), ("__state_hovered", "on")], |_| StyleRules {
        background: token("color.danger_hover", "#f00"),
        ..StyleRules::default()
    })
}

/// The host's copy: shape and every evaluation round-trip through bytes.
fn host_copy(bundle: Rc<StyleSheet>) -> StyleSheet {
    let shape: SheetShape = round_trip(&bundle.shape());
    StyleSheet::from_shape(
        &shape,
        Rc::new(move |part: &SheetPart, vs: &VariantSet| {
            let (part, vs): (SheetPart, VariantSet) = round_trip(&(part.clone(), vs.clone()));
            round_trip(&bundle.eval_part(&part, &vs).expect("the host only asks for parts the shape named"))
        }),
    )
}

#[test]
fn proxied_sheet_resolves_exactly_like_the_bundles_sheet() {
    let original = Rc::new(bundle_sheet());
    let proxy = host_copy(original.clone());
    let sets = [
        VariantSet::new(),
        VariantSet::new().with("tone", "danger"),
        VariantSet::new().with("tone", "danger").with("__state_hovered", "on"),
        VariantSet::new().with("size", "wide").with("__bp_md", "on"),
        VariantSet::new().with("__cq_minw_400", "on").with("__state_hovered", "on"),
        VariantSet::new().with("tone", "nonexistent"),
    ];
    for vs in &sets {
        assert_eq!(proxy.resolve(vs), original.resolve(vs), "variant set {vs:?}");
    }
}

/// The derived axis metadata the native engine routes on (state bits,
/// breakpoints, container widths, premint author axes) must come out the
/// same — the host builds through the ordinary builders, not by copying
/// fields, so this is what proves the shape carried enough.
#[test]
fn proxied_sheet_derives_the_same_axis_metadata() {
    let original = Rc::new(bundle_sheet());
    let proxy = host_copy(original.clone());
    assert_eq!(proxy.state_axes(), original.state_axes());
    assert_eq!(proxy.breakpoint_axes(), original.breakpoint_axes());
    assert_eq!(proxy.container_axes(), original.container_axes());
    assert_eq!(proxy.premint_author_axes(), original.premint_author_axes());
    assert_eq!(proxy.shape(), original.shape(), "a proxied sheet re-exports the same shape");
}

#[test]
fn eval_part_refuses_parts_the_sheet_does_not_have() {
    let sheet = bundle_sheet();
    let vs = VariantSet::new();
    assert!(sheet.eval_part(&SheetPart::Axis("tone".into(), "loud".into()), &vs).is_none());
    assert!(sheet.eval_part(&SheetPart::Axis("nope".into(), "on".into()), &vs).is_none());
    assert!(sheet.eval_part(&SheetPart::Compound(9), &vs).is_none());
    assert!(sheet.eval_part(&SheetPart::Base, &vs).is_some());
}
