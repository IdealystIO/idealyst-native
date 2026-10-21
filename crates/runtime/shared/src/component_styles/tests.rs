//! Registry, composition and resolution-order tests for component styles,
//! against hand-written declarations (the macro lives downstream; its own
//! tests cover the generated surface).

use super::*;
use crate::style::{Color, StyleApplication, Tokenized};
use crate::Length;

// --- a hand-written declaration: two slots, one axis ------------------------

#[derive(Default)]
struct Toks;

struct CardStyles;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct CardTone(&'static str);
#[allow(non_upper_case_globals)]
impl CardTone {
    const Plain: Self = Self("plain");
    const Loud: Self = Self("loud");
    const fn define(k: &'static str) -> Self {
        Self(k)
    }
}
impl VariantEnum for CardTone {
    fn as_variant_str(self) -> &'static str {
        self.0
    }
}
impl AxisKey for CardTone {
    type Styles = CardStyles;
    const AXIS: &'static str = "tone";
}

fn bg(c: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::Literal(Color(c.into())))
}

fn card_root() -> StyleSheet {
    StyleSheet::new(|_| StyleRules { background: bg("white"), padding_top: Some(Length::Px(8.0).into()), ..Default::default() })
        .variant("tone", "plain", |_| StyleRules::default())
        .variant("tone", "loud", |_| StyleRules { background: bg("red"), ..Default::default() })
        .variant_default("tone", "plain")
}

fn card_title() -> StyleSheet {
    StyleSheet::new(|_| StyleRules { color: bg("black"), ..Default::default() })
}

impl ComponentStyles for CardStyles {
    type Tokens = Toks;
    type Overrides = ();
    const NAME: &'static str = "CardStyles";
    fn slots() -> &'static [SlotDef<Self>] {
        static SLOTS: [SlotDef<CardStyles>; 2] = [
            SlotDef { name: "root", build: card_root, class: Some("iy-card00root"), extend: None },
            SlotDef { name: "title", build: card_title, class: Some("iy-card0title"), extend: None },
        ];
        &SLOTS
    }
    fn axes() -> &'static [AxisDef] {
        static AXES: [AxisDef; 1] = [AxisDef { name: "tone", values: &["plain", "loud"], default: Some("plain") }];
        &AXES
    }
}

/// A second library's identically-shaped declaration — same NAME, same
/// slot names, same classes. Only its TYPE differs.
struct OtherCardStyles;
impl ComponentStyles for OtherCardStyles {
    type Tokens = Toks;
    type Overrides = ();
    const NAME: &'static str = "CardStyles";
    fn slots() -> &'static [SlotDef<Self>] {
        static SLOTS: [SlotDef<OtherCardStyles>; 2] = [
            SlotDef { name: "root", build: card_root, class: Some("iy-card00root"), extend: None },
            SlotDef { name: "title", build: card_title, class: Some("iy-card0title"), extend: None },
        ];
        &SLOTS
    }
    fn axes() -> &'static [AxisDef] {
        &[]
    }
}

fn resolve(sheet: &Rc<StyleSheet>, vs: VariantSet) -> StyleRules {
    sheet.resolve(&vs)
}

fn bg_of(r: &StyleRules) -> String {
    r.background.as_ref().map(|c| c.value().0.clone()).unwrap_or_default()
}

fn reset() {
    __reset::<CardStyles>();
    __reset::<OtherCardStyles>();
}

#[test]
fn an_unrestyled_slot_is_the_declaration_with_its_macro_class() {
    reset();
    let root = slot_sheet::<CardStyles>(0);
    assert_eq!(root.premint_class(), Some("iy-card00root"));
    assert_eq!(bg_of(&resolve(&root, VariantSet::new())), "white");
    // Cached: the same Rc until something changes.
    assert!(Rc::ptr_eq(&root, &slot_sheet::<CardStyles>(0)));
}

#[test]
fn restyle_merges_into_the_base_and_keeps_what_it_leaves_unset() {
    reset();
    restyle::<CardStyles>(|s| {
        s.slot(0).base(|_| StyleRules { background: bg("ink"), ..Default::default() });
    });
    let root = slot_sheet::<CardStyles>(0);
    let r = resolve(&root, VariantSet::new());
    assert_eq!(bg_of(&r), "ink");
    assert!(r.padding_top.is_some(), "the declared padding survives a background restyle");
    // The declared arm still layers over the restyled base.
    assert_eq!(bg_of(&resolve(&root, VariantSet::new().with("tone", "loud"))), "red");
}

#[test]
fn a_restyled_slot_premints_under_a_deterministic_content_identity() {
    reset();
    let register = || {
        restyle::<CardStyles>(|s| {
            s.slot(0).base(|_| StyleRules { background: bg("ink"), ..Default::default() });
        })
    };
    register();
    let first = slot_sheet::<CardStyles>(0).premint_class().map(str::to_string);
    let first = first.expect("a restyled premintable slot keeps a premint class");
    assert_ne!(first, "iy-card00root", "a restyled slot must not reuse the declaration's class");
    // A fresh process composing the same restyle (the dump vs the shipped
    // bundle) arrives at the same class.
    reset();
    register();
    assert_eq!(slot_sheet::<CardStyles>(0).premint_class(), Some(first.as_str()));
    // The untouched slot keeps the declaration's class.
    assert_eq!(slot_sheet::<CardStyles>(1).premint_class(), Some("iy-card0title"));
}

#[test]
fn restyle_merges_into_an_existing_arm() {
    reset();
    restyle::<CardStyles>(|s| {
        s.slot(0).arm(CardTone::Loud, |_| StyleRules { color: bg("white"), ..Default::default() });
    });
    let r = resolve(&slot_sheet::<CardStyles>(0), VariantSet::new().with("tone", "loud"));
    assert_eq!(bg_of(&r), "red", "the declared arm's background survives");
    assert_eq!(r.color.unwrap().value().0, "white");
}

#[test]
fn a_custom_axis_value_gets_its_rules_from_a_restyle_arm() {
    reset();
    const PILL: CardTone = CardTone::define("pill");
    restyle::<CardStyles>(|s| {
        s.slot(0).arm(PILL, |_| StyleRules { background: bg("pink"), ..Default::default() });
    });
    let root = slot_sheet::<CardStyles>(0);
    assert_eq!(bg_of(&resolve(&root, VariantSet::new().with("tone", "pill"))), "pink");
    assert!(root.premint_author_axes().iter().any(|(a, _)| a == "tone"));
}

#[test]
fn compounds_and_states_from_a_restyle_layer_after_the_arms() {
    reset();
    restyle::<CardStyles>(|s| {
        s.slot(0)
            .state(StateBits::HOVERED, |_| StyleRules { background: bg("hover"), ..Default::default() })
            .compound((CardTone::Loud, StateBits::HOVERED), |_| StyleRules { background: bg("loud-hover"), ..Default::default() });
    });
    let root = slot_sheet::<CardStyles>(0);
    let hovered = VariantSet::new().with("__state_hovered", "on");
    assert_eq!(bg_of(&resolve(&root, hovered.clone())), "hover");
    assert_eq!(bg_of(&resolve(&root, hovered.with("tone", "loud"))), "loud-hover");
    assert!(root.state_axes().iter().any(|(b, _)| *b == StateBits::HOVERED));
}

#[test]
fn a_compound_declares_the_axes_it_names_so_premint_can_stamp_them() {
    reset();
    // `title` declares no `tone` axis and no state.
    restyle::<CardStyles>(|s| {
        s.slot(1).compound((CardTone::Loud, StateBits::PRESSED), |_| StyleRules { color: bg("pressed"), ..Default::default() });
    });
    let title = slot_sheet::<CardStyles>(1);
    assert!(title.premint_author_axes().iter().any(|(a, _)| a == "tone"), "the tone class gets stamped");
    assert!(title.state_axes().iter().any(|(b, _)| *b == StateBits::PRESSED), "the pressed state gets tracked");
    let vs = VariantSet::new().with("tone", "loud").with("__state_pressed", "on");
    assert_eq!(resolve(&title, vs).color.unwrap().value().0, "pressed");
}

#[test]
fn later_restyles_win_over_earlier_ones() {
    reset();
    restyle::<CardStyles>(|s| {
        s.slot(0).base(|_| StyleRules { background: bg("first"), ..Default::default() });
    });
    restyle::<CardStyles>(|s| {
        s.slot(0).base(|_| StyleRules { background: bg("second"), ..Default::default() });
    });
    assert_eq!(bg_of(&resolve(&slot_sheet::<CardStyles>(0), VariantSet::new())), "second");
}

#[test]
fn registry_is_keyed_by_type_so_same_named_declarations_never_collide() {
    reset();
    restyle::<CardStyles>(|s| {
        s.slot(0).base(|_| StyleRules { background: bg("ink"), ..Default::default() });
    });
    assert_eq!(bg_of(&resolve(&slot_sheet::<OtherCardStyles>(0), VariantSet::new())), "white");
    assert_eq!(bg_of(&resolve(&slot_sheet::<CardStyles>(0), VariantSet::new())), "ink");
}

#[test]
fn re_registering_from_the_same_call_site_replaces_rather_than_stacks() {
    reset();
    for colour in ["one", "two"] {
        let c = colour.to_string();
        restyle::<CardStyles>(move |s| {
            s.slot(0).base(|_| StyleRules { background: bg(&c), ..Default::default() });
        });
    }
    let n = REGISTRY.with(|r| r.borrow()[&TypeId::of::<CardStyles>()].restyles.len());
    assert_eq!(n, 1, "one call site, one registration");
    assert_eq!(bg_of(&resolve(&slot_sheet::<CardStyles>(0), VariantSet::new())), "two");
}

#[test]
fn a_restyle_after_first_use_warns_but_an_identical_rerun_does_not() {
    reset();
    let register = |c: &'static str| {
        restyle::<CardStyles>(move |s| {
            s.slot(0).base(|_| StyleRules { background: bg(c), ..Default::default() });
        })
    };
    register("boot");
    let _ = slot_sheet::<CardStyles>(0);
    let before = __warn_count();
    // The same composition again (a second SSR request running app()).
    register("boot");
    let _ = slot_sheet::<CardStyles>(0);
    assert_eq!(__warn_count(), before, "an identical re-registration is not a change");
    // A genuinely different restyle after use.
    register("late");
    let late = slot_sheet::<CardStyles>(0);
    assert_eq!(__warn_count(), before + 1, "a late restyle warns");
    assert_eq!(bg_of(&resolve(&late, VariantSet::new())), "late", "and still applies to new elements");
}

#[test]
fn an_unstyled_custom_value_warns_in_debug_and_a_styled_one_does_not() {
    reset();
    const GHOST: CardTone = CardTone::define("ghost_unstyled_test");
    const PILL: CardTone = CardTone::define("pill_styled_test");
    restyle::<CardStyles>(|s| {
        s.slot(0).arm(PILL, |_| StyleRules { background: bg("pink"), ..Default::default() });
    });
    let before = __warn_count();
    check_axis_value::<CardStyles>("tone", PILL.as_variant_str());
    check_axis_value::<CardStyles>("tone", CardTone::Plain.as_variant_str());
    assert_eq!(__warn_count(), before);
    check_axis_value::<CardStyles>("tone", GHOST.as_variant_str());
    #[cfg(debug_assertions)]
    assert_eq!(__warn_count(), before + 1);
    // Once per value.
    check_axis_value::<CardStyles>("tone", GHOST.as_variant_str());
    #[cfg(debug_assertions)]
    assert_eq!(__warn_count(), before + 1);
}

#[test]
fn set_axis_default_moves_the_sheet_default_and_the_axis_default() {
    reset();
    assert_eq!(axis_default::<CardStyles>("tone"), None);
    set_axis_default(CardTone::Loud);
    assert_eq!(axis_default::<CardStyles>("tone"), Some("loud"));
    let root = slot_sheet::<CardStyles>(0);
    assert_eq!(bg_of(&resolve(&root, VariantSet::new())), "red", "an unset axis now means `loud`");
    // The title slot has no `tone` axis: the default is not forced onto it.
    assert!(!slot_sheet::<CardStyles>(1).declares_axis("tone"));
    let app = StyleApplication::new(root);
    assert!(app.preminted_class_list().unwrap().ends_with("-tone-loud"));
}

// --- extend hooks ------------------------------------------------------------

thread_local! {
    static EXTRA_TONES: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

struct ChipStyles;
impl ComponentStyles for ChipStyles {
    type Tokens = Toks;
    type Overrides = ();
    const NAME: &'static str = "ChipStyles";
    fn slots() -> &'static [SlotDef<Self>] {
        fn extend(s: &mut SlotRestyle<'_, ChipStyles>) {
            for t in EXTRA_TONES.with(|e| e.borrow().clone()) {
                s.raw_arm("tone", t, StyleRules { background: bg(t), ..Default::default() });
            }
            s.raw_arm("tone", "base", StyleRules { background: bg("from-extend"), ..Default::default() });
        }
        static SLOTS: [SlotDef<ChipStyles>; 1] =
            [SlotDef { name: "root", build: card_title, class: Some("iy-chip00root"), extend: Some(extend) }];
        &SLOTS
    }
    fn axes() -> &'static [AxisDef] {
        &[]
    }
}

#[derive(Clone, Copy)]
struct ChipTone(&'static str);
impl VariantEnum for ChipTone {
    fn as_variant_str(self) -> &'static str {
        self.0
    }
}
impl AxisKey for ChipTone {
    type Styles = ChipStyles;
    const AXIS: &'static str = "tone";
}

#[test]
fn extend_hooks_run_first_and_restyles_override_them() {
    __reset::<ChipStyles>();
    EXTRA_TONES.with(|e| e.borrow_mut().clear());
    let root = slot_sheet::<ChipStyles>(0);
    assert_eq!(bg_of(&resolve(&root, VariantSet::new().with("tone", "base"))), "from-extend");
    assert_ne!(root.premint_class(), Some("iy-chip00root"), "an extended slot carries a composed identity");
    restyle::<ChipStyles>(|s| {
        s.slot(0).arm(ChipTone("base"), |_| StyleRules { background: bg("from-app"), ..Default::default() });
    });
    assert_eq!(
        bg_of(&resolve(&slot_sheet::<ChipStyles>(0), VariantSet::new().with("tone", "base"))),
        "from-app"
    );
}

#[test]
fn invalidate_all_recomposes_with_what_extend_hooks_now_read() {
    __reset::<ChipStyles>();
    EXTRA_TONES.with(|e| e.borrow_mut().clear());
    let before = slot_sheet::<ChipStyles>(0);
    assert!(!before.variant_keys().iter().any(|(_, v)| v == "brand"));
    EXTRA_TONES.with(|e| e.borrow_mut().push("brand"));
    // Still cached until invalidated.
    assert!(Rc::ptr_eq(&before, &slot_sheet::<ChipStyles>(0)));
    invalidate_all();
    let after = slot_sheet::<ChipStyles>(0);
    assert_eq!(bg_of(&resolve(&after, VariantSet::new().with("tone", "brand"))), "brand");
    EXTRA_TONES.with(|e| e.borrow_mut().clear());
}

#[test]
fn slot_override_arms_resolve_against_the_slot_selection() {
    let o = SlotOverride::from(StyleRules { color: bg("green"), ..Default::default() })
        .arm(CardTone::Loud, StyleRules { background: bg("on"), ..Default::default() });
    assert!(o.has_arms());
    let quiet = o.resolve(&VariantSet::new());
    assert_eq!(quiet.color.as_ref().unwrap().value().0, "green");
    assert!(quiet.background.is_none(), "an arm applies only while its value is selected");
    let loud = o.resolve(&VariantSet::new().with("tone", "loud"));
    assert_eq!(bg_of(&loud), "on");
    assert_eq!(loud.color.as_ref().unwrap().value().0, "green", "the arm merges over the base");
    let fixed = SlotOverride::from(StyleRules { background: bg("x"), ..Default::default() });
    assert_eq!(bg_of(&fixed.resolve(&VariantSet::new().with("tone", "loud"))), "x");
}

#[test]
fn composed_slots_names_every_composed_slot_with_its_sheet() {
    reset();
    __compose_for_dump::<CardStyles>();
    let slots: Vec<_> = composed_slots()
        .into_iter()
        .filter(|(component, _, _)| *component == CardStyles::NAME)
        .collect();
    let names: Vec<_> = slots.iter().map(|(_, slot, _)| *slot).collect();
    assert_eq!(names, ["root", "title"], "declaration order");
    assert_eq!(slots[0].2.premint_class(), Some("iy-card00root"), "the composed sheet itself");
}

/// An axis a slot only meets through a compound leg (an `extend` hook or a
/// restyle declared it with empty arms) takes the component's declared
/// default, so an application that never selects the axis stamps the
/// default's class like every other slot of the component.
#[test]
fn a_compound_only_axis_takes_the_components_declared_default() {
    reset();
    restyle::<CardStyles>(|s| {
        s.slot(1).compound((CardTone::Loud, StateBits::PRESSED), |_| StyleRules { color: bg("pressed"), ..Default::default() });
    });
    let title = slot_sheet::<CardStyles>(1);
    assert_eq!(title.axis_default("tone").map(String::as_str), Some("plain"), "the declared `#[default]` reaches the slot");
}
