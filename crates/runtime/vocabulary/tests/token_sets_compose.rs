//! Token sets composing and coexisting: `#[tokens(flatten)]` (embed a base
//! set under its own names), `rename`, `as`, computed tokens, cross-set
//! name ownership (a second set claiming a name is a hard error), and
//! `current()` (the active theme struct, read reactively).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_macros::Tokens;
use runtime_shared::TokenValue;
use runtime_vocabulary::glue::{Color, Length, TokenTheme, Tokens};
use runtime_world::World;

// ---------------------------------------------------------------------------
// A base set, the way a design system ships one
// ---------------------------------------------------------------------------

#[derive(Tokens, Clone, Debug, PartialEq)]
pub struct Neutrals {
    pub surface: Color,
    pub text: Color,
}

/// Stored as bare `f32` px, installed as `Length` — and one token with no
/// stored field.
#[derive(Tokens, Clone, Debug, PartialEq)]
#[tokens(as = Length, computed(pill: Length = Radius::pill))]
pub struct Radius {
    pub sm: f32,
    pub md: f32,
}

impl Radius {
    fn pill(&self) -> Length {
        Length::Full
    }
}

#[derive(Tokens, Clone, Debug, PartialEq)]
#[tokens(default = base_default)]
pub struct Base {
    /// Stored as `colors`, reached and named as `color`.
    #[tokens(rename = "color")]
    pub colors: Neutrals,
    pub radius: Radius,
    #[tokens(as = Length)]
    pub gap: f32,
}

fn base_default() -> Base {
    Base {
        colors: Neutrals { surface: "#ffffff".into(), text: "#111111".into() },
        radius: Radius { sm: 4.0, md: 8.0 },
        gap: 12.0,
    }
}

/// An app theme that embeds the base and adds its own tokens.
#[derive(Tokens, Clone, Debug, PartialEq)]
#[tokens(default = app_default)]
pub struct App {
    pub brand: Color,
    #[tokens(flatten)]
    pub base: Base,
}

fn app_default() -> App {
    App { brand: "#4f46e5".into(), base: base_default() }
}

/// A second, independent set that happens to name `color-surface`.
#[derive(Tokens, Clone)]
#[tokens(default = rival_default)]
pub struct Rival {
    pub color: Neutrals,
}

fn rival_default() -> Rival {
    Rival { color: Neutrals { surface: "#000000".into(), text: "#ffffff".into() } }
}

fn installed(world: &World, name: &str) -> Option<TokenValue> {
    world.enter(|| runtime_vocabulary::glue::token_value(name))
}

// ---------------------------------------------------------------------------
// rename / as / computed
// ---------------------------------------------------------------------------

#[test]
fn rename_as_and_computed_shape_names_accessors_and_values() {
    assert_eq!(
        Base::NAMES,
        ["color-surface", "color-text", "radius-sm", "radius-md", "radius-pill", "gap"]
    );
    assert_eq!(Base::PATHS[0], "color.surface");
    assert_eq!(Base::PATHS[4], "radius.pill");

    let t = Base::tokens();
    assert_eq!(t.color.surface().name(), Some("color-surface"));
    let md: runtime_shared::Tokenized<Length> = t.radius.md();
    assert_eq!((md.name(), md.value()), (Some("radius-md"), &Length::Px(8.0)), "`as = Length` reads as a Length");
    assert_eq!(t.radius.pill().value(), &Length::Full, "a computed token's fallback is its computed value");

    let entries = base_default().token_entries();
    assert_eq!(entries[3].value, TokenValue::Length(Length::Px(8.0)));
    assert_eq!(entries[4].value, TokenValue::Length(Length::Full));
    assert_eq!(entries[5].value, TokenValue::Length(Length::Px(12.0)));
}

// ---------------------------------------------------------------------------
// flatten
// ---------------------------------------------------------------------------

/// The embedded set keeps its own names and accessor paths; the embedding
/// set adds its own in front.
#[test]
fn a_flattened_base_keeps_its_names_and_accessors() {
    let mut expected = vec!["brand"];
    expected.extend(Base::NAMES);
    assert_eq!(App::NAMES, expected.as_slice());
    assert_eq!(App::PATHS[1], "color.surface", "no `base.` segment in the path");

    let t = App::tokens();
    assert_eq!(t.brand().name(), Some("brand"));
    assert_eq!(t.color.surface().name(), Some("color-surface"), "reached through Deref");
    assert_eq!(t.radius.pill().name(), Some("radius-pill"));
    assert_eq!(t.base.gap().name(), Some("gap"), "the embedding field still reaches it too");
}

/// Installing the embedding set installs the base's tokens, valued from
/// the embedding theme — and `.with` reaches into the base.
#[test]
fn installing_the_embedding_set_installs_the_base() {
    let world = World::new();
    let dark = app_default().with(|t| t.base.colors.surface = "#0a0a0a".into());
    world.enter(|| App::install(dark));
    world.flush();
    assert_eq!(installed(&world, "color-surface"), Some(TokenValue::Color(Color("#0a0a0a".into()))));
    assert_eq!(installed(&world, "brand"), Some(TokenValue::Color(Color("#4f46e5".into()))));
    assert_eq!(installed(&world, "radius-pill"), Some(TokenValue::Length(Length::Full)));
}

// ---------------------------------------------------------------------------
// Cross-set ownership
// ---------------------------------------------------------------------------

/// Two different sets claiming one name is a hard error naming both sets
/// and the token — never a silent overwrite.
#[test]
#[should_panic(expected = "token `color-surface` is already owned by token set")]
fn regression_two_sets_claiming_one_name_panic() {
    let world = World::new();
    world.enter(|| {
        Base::install(base_default());
        Rival::install(rival_default());
    });
}

/// The message names both sets.
#[test]
fn the_ownership_error_names_both_sets() {
    let world = World::new();
    world.enter(|| Base::install(base_default()));
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Rival::install(rival_default());
    }))
    .expect_err("a second owner must panic");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("token_sets_compose::Base"), "{msg}");
    assert!(msg.contains("token_sets_compose::Rival"), "{msg}");
}

/// The same set re-installing and swapping is not a collision, and an
/// untyped write (a theme editor's live edit) claims nothing.
#[test]
fn the_same_set_may_reinstall_and_swap_and_raw_writes_are_unchecked() {
    let world = World::new();
    world.enter(|| {
        Base::install(base_default());
        Base::install(base_default());
        Base::set(base_default().with(|t| t.gap = 20.0));
        runtime_vocabulary::glue::update_tokens(&[runtime_shared::TokenEntry {
            name: "color-surface",
            value: TokenValue::Color(Color("#123456".into())),
        }]);
    });
    world.flush();
    assert_eq!(installed(&world, "gap"), Some(TokenValue::Length(Length::Px(20.0))));
    assert_eq!(installed(&world, "color-surface"), Some(TokenValue::Color(Color("#123456".into()))));
}

/// Ownership is per world: an SSR request world installing another set
/// does not collide with the app world's.
#[test]
fn ownership_is_per_world() {
    let a = World::new();
    let b = World::new();
    a.enter(|| Base::install(base_default()));
    b.enter(|| Rival::install(rival_default()));
}

// ---------------------------------------------------------------------------
// current()
// ---------------------------------------------------------------------------

#[test]
fn current_is_the_active_theme_and_none_before_install() {
    let world = World::new();
    world.enter(|| assert!(App::try_current().is_none()));
    world.enter(|| App::install(app_default()));
    world.enter(|| assert_eq!(*App::current(), app_default()));
}

/// A reader subscribes: an effect reading `current()` re-runs on a swap,
/// and the swap is visible to a read in the same turn.
#[test]
fn current_is_a_reactive_read() {
    let world = World::new();
    let seen: Rc<RefCell<Vec<String>>> = Rc::default();
    world.enter(|| {
        App::install(app_default());
        let seen = seen.clone();
        let _e = runtime_world::effect(move || seen.borrow_mut().push(App::current().brand.0.clone()));
    });
    world.flush();
    assert_eq!(seen.borrow().last().map(String::as_str), Some("#4f46e5"));

    world.enter(|| {
        App::set(app_default().with(|t| t.brand = "#db2777".into()));
        assert_eq!(App::current().brand, Color("#db2777".into()), "visible in the same turn");
    });
    world.flush();
    assert_eq!(seen.borrow().last().map(String::as_str), Some("#db2777"), "the reader re-ran: {:?}", seen.borrow());
}

/// From an event handler (outside `World::enter`) `set` and `current`
/// reach the world that installed the theme.
#[test]
fn current_and_set_work_from_an_event_handler() {
    let world = World::new();
    world.enter(|| App::install(app_default()));
    world.flush();
    App::set(app_default().with(|t| t.brand = "#000000".into()));
    assert_eq!(App::current().brand, Color("#000000".into()));
    world.flush();
    world.enter(|| assert_eq!(App::current().brand, Color("#000000".into())));
}

// ---------------------------------------------------------------------------
// Ownership along an embedding
// ---------------------------------------------------------------------------

/// A second app set built on the same base.
#[derive(Tokens, Clone)]
#[tokens(default = other_default)]
pub struct OtherApp {
    pub accent: Color,
    #[tokens(flatten)]
    pub base: Base,
}

fn other_default() -> OtherApp {
    OtherApp { accent: "#000000".into(), base: base_default() }
}

/// The embedding set and the set it embeds may both install the shared
/// names, in either order — an app theme built on a design system's, with
/// the design system's own install still running.
#[test]
fn an_embedding_set_and_its_base_share_names_in_either_order() {
    let world = World::new();
    world.enter(|| {
        Base::install(base_default());
        App::install(app_default());
        Base::set(base_default());
    });
    let world = World::new();
    world.enter(|| {
        App::install(app_default());
        Base::install(base_default());
    });
}

/// Two sets embedding the SAME base are still two owners of its names:
/// the embedding set takes ownership, so the second one collides.
#[test]
#[should_panic(expected = "is already owned by token set")]
fn regression_two_sets_embedding_one_base_collide() {
    let world = World::new();
    world.enter(|| {
        Base::install(base_default());
        App::install(app_default());
        OtherApp::install(other_default());
    });
}
