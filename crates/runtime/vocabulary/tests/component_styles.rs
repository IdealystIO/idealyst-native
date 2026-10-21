//! `component_styles!` end to end: the generated surface (styles type, slot
//! accessors, typed open axes, restyle handle, per-instance overrides)
//! driving real style props, plus the resolution order a restyle and an
//! override take.

use std::rc::Rc;

use runtime_macros::{axis_values, component_styles};
use runtime_vocabulary::glue::{
    component_styles as cs, Color, IntoStyleProp, StateBits, StyleProp, StyleRules, StyleSheet, Tokenized, VariantEnum,
};

component_styles! {
    /// A test segmented control.
    pub Seg<()> {
        /// Track density.
        axis density { compact, #[default] comfy }
        slot track {
            base(_t) { background: Color("white".into()), padding: 4 }
            variant density { compact(_t) { padding: 2 } comfy(_t) { padding: 6 } }
        }
        slot segment {
            base(_t) { color: Color("grey".into()) }
            variant selected { on(_t) { color: Color("black".into()) } #[default] off(_t) {} }
            state hovered(_t) { color: Color("dimgrey".into()) }
        }
    }
}

component_styles! {
    pub Pill<()> {
        slot root {
            base(_t) { background: Color("white".into()) }
            variant tone { #[default] calm(_t) {} loud(_t) { background: Color("red".into()) } }
        }
    }
}

fn app_of(prop: StyleProp) -> runtime_vocabulary::glue::StyleApplication {
    match prop {
        StyleProp::Sheet(app) => *app,
        StyleProp::SheetDynamic(f) => f(),
        _ => panic!("expected a sheet application"),
    }
}

fn bg(r: &StyleRules) -> String {
    r.background.as_ref().map(|c| c.value().0.clone()).unwrap_or_default()
}

fn fg(r: &StyleRules) -> String {
    r.color.as_ref().map(|c| c.value().0.clone()).unwrap_or_default()
}

fn pad(r: &StyleRules) -> f32 {
    match r.padding_top.as_ref().map(|p| p.value().clone()) {
        Some(runtime_vocabulary::glue::Length::Px(v)) => v,
        other => panic!("padding {other:?}"),
    }
}

fn reset() {
    cs::__reset::<SegStyles>();
    cs::__reset::<PillStyles>();
}

#[test]
fn slots_resolve_their_declared_sheets_with_typed_axes() {
    reset();
    let app = app_of(SegStyles::track().set(SegDensity::Compact).into_style_prop());
    let r = app.sheet.resolve(&app.variants);
    assert_eq!(bg(&r), "white");
    assert_eq!(pad(&r), 2.0);
    // Unset → the declared default.
    let app = app_of(SegStyles::track().into_style_prop());
    assert_eq!(pad(&app.sheet.resolve(&app.variants)), 6.0);
    assert_eq!(SegDensity::default(), SegDensity::Comfy);
    assert_eq!(SegDensity::BUILTINS, &[SegDensity::Compact, SegDensity::Comfy]);
    assert_eq!(<SegDensity as VariantEnum>::all_variants().len(), 2);
    assert_eq!(SegSelected::from(true), SegSelected::On);
}

#[test]
fn restyle_reaches_every_slot_application_app_wide() {
    reset();
    SegStyles::restyle(|s| {
        s.track().base(|_| StyleRules { background: Some(Tokenized::Literal(Color("ink".into()))), ..Default::default() });
        s.segment()
            .arm(SegSelected::On, |_| StyleRules {
                background: Some(Tokenized::Literal(Color("blue".into()))),
                color: Some(Tokenized::Literal(Color("white".into()))),
                ..Default::default()
            })
            .state(StateBits::HOVERED, |_| StyleRules { color: Some(Tokenized::Literal(Color("navy".into()))), ..Default::default() });
    });
    let app = app_of(SegStyles::track().into_style_prop());
    let r = app.sheet.resolve(&app.variants);
    assert_eq!(bg(&r), "ink");
    assert_eq!(pad(&r), 6.0, "restyling the background keeps the declared padding");

    let app = app_of(SegStyles::segment().set(SegSelected::On).into_style_prop());
    let r = app.sheet.resolve(&app.variants);
    assert_eq!((bg(&r).as_str(), fg(&r).as_str()), ("blue", "white"));
    let hovered = app.variants.clone().with("__state_hovered", "on");
    assert_eq!(fg(&app.sheet.resolve(&hovered)), "navy", "a state overlay layers after the axis arms");
}

#[test]
fn a_custom_variant_value_is_typed_per_component_and_styled_by_a_restyle() {
    reset();
    const HUSHED: SegDensity = SegDensity::define("hushed");
    SegStyles::restyle(|s| {
        s.track().arm(HUSHED, |_| StyleRules { padding_top: Some(0.0.into()), ..Default::default() });
    });
    let app = app_of(SegStyles::track().set(HUSHED).into_style_prop());
    assert_eq!(pad(&app.sheet.resolve(&app.variants)), 0.0);
    assert_eq!(HUSHED.key(), "hushed");
}

// `axis_values!` names app values like the built-ins and derives each key
// from the name. The inherent form needs the axis type's own crate (this
// one); the trait form is how any other crate adds `ButtonVariant::Raised`.
axis_values! { impl SegDensity { Hushed, ExtraRoomy } }
axis_values! { pub trait AppPillTones for PillTone { Alarm } }

#[test]
fn axis_values_are_named_like_builtins_with_keys_from_their_names() {
    reset();
    assert_eq!(SegDensity::Hushed, SegDensity::define("hushed"));
    assert_eq!(SegDensity::ExtraRoomy.key(), "extra_roomy");
    assert_eq!(PillTone::Alarm.key(), "alarm");

    // A restyle arm keyed by the named value styles it.
    SegStyles::restyle(|s| {
        s.track().arm(SegDensity::Hushed, |_| StyleRules { padding_top: Some(0.0.into()), ..Default::default() });
    });
    let app = app_of(SegStyles::track().set(SegDensity::Hushed).into_style_prop());
    assert_eq!(pad(&app.sheet.resolve(&app.variants)), 0.0);

    // Usable as patterns beside the built-ins — trait-provided ones too.
    let name = |d: SegDensity| match d {
        SegDensity::Compact => "compact",
        SegDensity::Hushed => "hushed",
        _ => "other",
    };
    assert_eq!(name(SegDensity::Hushed), "hushed");
    assert_eq!(name(SegDensity::Comfy), "other");
    let alarm = |t: PillTone| matches!(t, PillTone::Alarm);
    assert!(alarm(PillTone::define("alarm")) && !alarm(PillTone::Loud));
}

#[test]
fn an_unstyled_custom_value_warns() {
    reset();
    const NOWHERE: PillTone = PillTone::define("nowhere_styled");
    let before = cs::__warn_count();
    let _ = app_of(PillStyles::root().set(PillTone::Loud).into_style_prop());
    assert_eq!(cs::__warn_count(), before);
    let app = app_of(PillStyles::root().set(NOWHERE).into_style_prop());
    #[cfg(debug_assertions)]
    assert_eq!(cs::__warn_count(), before + 1, "an app value nothing styles warns");
    assert_eq!(bg(&app.sheet.resolve(&app.variants)), "white", "and renders as the base");
}

#[test]
fn set_default_changes_unset_axes_and_the_axis_default() {
    reset();
    SegStyles::set_default(SegDensity::Compact);
    assert_eq!(SegDensity::default(), SegDensity::Compact);
    let app = app_of(SegStyles::track().into_style_prop());
    assert_eq!(pad(&app.sheet.resolve(&app.variants)), 2.0);
}

#[test]
fn per_instance_overrides_layer_last_over_restyles_and_states() {
    reset();
    SegStyles::restyle(|s| {
        s.segment().base(|_| StyleRules { color: Some(Tokenized::Literal(Color("restyled".into()))), ..Default::default() });
    });
    let overrides = SegSlotStyles::default().segment(StyleRules {
        color: Some(Tokenized::Literal(Color("instance".into()))),
        ..Default::default()
    });
    let app = app_of(SegStyles::segment().overridden(overrides.segment.as_ref()).into_style_prop());
    let resolved = runtime_vocabulary::glue::resolve_style(&app);
    assert_eq!(fg(&resolved), "instance");
    // The override is the INLINE layer: the application stays premintable.
    assert!(app.inline().is_some());
    assert!(app.preminted_class_list().is_some(), "an override does not knock the slot off premint");
    // An override with an arm follows the slot's selection: only the
    // selected segment takes this one's background.
    let per_state = cs::SlotOverride::from(StyleRules::default())
        .arm(SegSelected::On, StyleRules { background: Some(Tokenized::Literal(Color("picked".into()))), ..Default::default() });
    let o = SegSlotStyles::default().segment(per_state);
    let on = app_of(SegStyles::segment().set(SegSelected::On).overridden(o.segment.as_ref()).into_style_prop());
    let off = app_of(SegStyles::segment().set(SegSelected::Off).overridden(o.segment.as_ref()).into_style_prop());
    assert_eq!(bg(&runtime_vocabulary::glue::resolve_style(&on)), "picked");
    assert_ne!(bg(&runtime_vocabulary::glue::resolve_style(&off)), "picked");
}

#[test]
fn a_reactive_axis_rides_the_dynamic_path() {
    reset();
    let world = runtime_world::World::new();
    world.enter(|| {
        let sel = runtime_world::signal(SegSelected::Off);
        let prop = SegStyles::segment().set(runtime_vocabulary::glue::derived(move || sel.get())).into_style_prop();
        let StyleProp::SheetDynamic(f) = prop else { panic!("a signal-driven axis is dynamic") };
        assert_eq!(f().variants.0.get("selected").map(String::as_str), Some("off"));
        sel.set(SegSelected::On);
        world.flush();
        assert_eq!(f().variants.0.get("selected").map(String::as_str), Some("on"));
    });
}

#[test]
fn shared_values_reach_every_slot() {
    reset();
    let v = SegStyles::values().set(SegDensity::Compact).set(SegSelected::On);
    let track = app_of(SegStyles::track().values(&v).into_style_prop());
    let seg = app_of(SegStyles::segment().values(&v).into_style_prop());
    assert_eq!(track.variants.0.get("density").map(String::as_str), Some("compact"));
    assert_eq!(seg.variants.0.get("selected").map(String::as_str), Some("on"));
}

#[test]
fn slot_names_and_types_are_per_component() {
    assert_eq!(cs::slot_names::<SegStyles>(), vec!["track", "segment"]);
    assert_ne!(SegStyles::track().sheet().premint_class(), SegStyles::segment().sheet().premint_class());
}

/// `#[props(styles = SegStyles)]` gives a props struct one override field
/// per slot of the component, so no component hand-writes `*_style` props.
#[runtime_macros::props(styles = SegStyles)]
#[derive(Default)]
pub struct SegControlProps {
    pub block: bool,
}

#[test]
fn props_styles_argument_adds_the_generated_override_set() {
    let props = SegControlProps {
        styles: SegSlotStyles::default().segment(StyleRules {
            color: Some(Tokenized::Literal(Color("instance".into()))),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(props.styles.track.is_none());
    assert_eq!(fg(&props.styles.segment.as_ref().unwrap().resolve(&Default::default())), "instance");
    assert!(!props.block.get());
}

/// A `#[props]`-wrapped axis prop (`Reactive<E>`) selects directly: a static
/// one stays on the static path, a dynamic one re-applies.
#[test]
fn reactive_props_select_axes_statically_or_dynamically() {
    use runtime_vocabulary::glue::Reactive;
    reset();
    let prop = SegStyles::track().set(Reactive::Static(SegDensity::Compact)).into_style_prop();
    assert!(matches!(prop, StyleProp::Sheet(_)), "a static Reactive keeps the static path");
    let prop = SegStyles::track()
        .set(Reactive::Dynamic(Rc::new(|| SegDensity::Compact)))
        .into_style_prop();
    let StyleProp::SheetDynamic(f) = prop else { panic!("a dynamic Reactive is dynamic") };
    assert_eq!(f().variants.0.get("density").map(String::as_str), Some("compact"));
}
