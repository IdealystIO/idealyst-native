//! `#[derive(Tokens)]` end to end: an app's own theme declared as plain
//! structs, installed, swapped, declared as palettes, driven by a signal,
//! listed in the catalog, and referenced from a `stylesheet!` whose
//! mounted node re-resolves on a swap.
//!
//! The theme below is the ergonomics proof: a realistic todo-app theme
//! (canvas, surface, text, muted text, an accent and a danger swatch, the
//! done-row tint, spacing, radius) and its stylesheets, with every value
//! written exactly once — in `light()`, and as deltas in `dark()`. No hex
//! appears outside those two functions, and no token name is spelled as a
//! string anywhere but the assertions that pin them.

use host_mock::Harness;
use runtime_macros::{stylesheet, Tokens};
use runtime_scene::realize;
use runtime_shared::{ColorScheme, StyleApplication, TokenValue};
use runtime_vocabulary::builders::view;
use runtime_vocabulary::glue::{Color, Length, TokenLeaf, TokenTheme, Tokens};
use runtime_world::{signal, World};

// ===========================================================================
// The todo app's theme — authored the way an app writes it
// ===========================================================================

/// A colour role in three strengths. A SHAPE: no default, so it can only
/// be used inside a theme — here twice, as `accent` and `danger`.
#[derive(Tokens, Clone, Debug, PartialEq)]
pub struct Swatch {
    /// Filled background (a primary button).
    pub solid_bg: Color,
    /// Text on `solid_bg`.
    pub solid_text: Color,
    /// Tinted background (a selected row, a badge).
    pub soft_bg: Color,
}

/// The spacing scale.
#[derive(Tokens, Clone, Debug, PartialEq)]
pub struct Spacing {
    pub sm: Length,
    pub md: Length,
    pub lg: Length,
}

/// A domain newtype that installs as a colour: proves leaf types are open.
#[derive(Clone, Debug, PartialEq)]
pub struct Brand(pub Color);

impl TokenLeaf for Brand {
    type Value = Color;
    fn token_value(&self) -> Color {
        self.0.clone()
    }
}

/// The todo app's theme.
#[derive(Tokens, Clone, Debug, PartialEq)]
#[tokens(default = light)]
pub struct TodoTheme {
    /// The canvas behind every screen.
    #[tokens(app_background)]
    pub background: Color,
    /// Cards and rows.
    pub surface: Color,
    pub text: Color,
    /// Secondary text — timestamps, the "n left" counter.
    pub muted: Color,
    pub accent: Swatch,
    pub danger: Swatch,
    /// A completed row's background.
    pub done_row: Color,
    pub brand: Brand,
    pub space: Spacing,
    pub radius: Length,
    /// A completed row's text opacity.
    pub done_opacity: f32,
    /// Not a token: travels with the theme, absent from the vocabulary.
    #[tokens(skip)]
    pub name: &'static str,
}

fn light() -> TodoTheme {
    TodoTheme {
        background: "#f3f4f6".into(),
        surface: "#ffffff".into(),
        text: "#111827".into(),
        muted: "#6b7280".into(),
        accent: Swatch { solid_bg: "#4f46e5".into(), solid_text: "#ffffff".into(), soft_bg: "#eef2ff".into() },
        danger: Swatch { solid_bg: "#dc2626".into(), solid_text: "#ffffff".into(), soft_bg: "#fef2f2".into() },
        done_row: "#f0fdf4".into(),
        brand: Brand("#0ea5e9".into()),
        space: Spacing { sm: 4.0.into(), md: 8.0.into(), lg: 16.0.into() },
        radius: 8.0.into(),
        done_opacity: 0.6,
        name: "light",
    }
}

fn dark() -> TodoTheme {
    light().with(|t| {
        t.background = "#030712".into();
        t.surface = "#111827".into();
        t.text = "#e5e7eb".into();
        t.muted = "#9ca3af".into();
        t.accent.soft_bg = "#1e1b4b".into();
        t.danger.soft_bg = "#450a0a".into();
        t.done_row = "#052e16".into();
        t.done_opacity = 0.5;
        t.name = "dark";
    })
}

stylesheet! {
    /// One todo row.
    pub TodoRow<TodoTheme> {
        base(t) {
            background: t.surface(),
            color: t.text(),
            border_color: t.accent.soft_bg(),
            padding: t.space.md(),
            border_radius: t.radius(),
        }
    }
}

stylesheet! {
    /// The destructive "clear completed" button.
    pub ClearButton<TodoTheme> {
        base(t) {
            background: t.danger.solid_bg(),
            color: t.danger.solid_text(),
            padding: t.space.sm(),
            opacity: t.done_opacity(),
        }
    }
}

const ALL_NAMES: &[&str] = &[
    "background",
    "surface",
    "text",
    "muted",
    "accent-solid-bg",
    "accent-solid-text",
    "accent-soft-bg",
    "danger-solid-bg",
    "danger-solid-text",
    "danger-soft-bg",
    "done-row",
    "brand",
    "space-sm",
    "space-md",
    "space-lg",
    "radius",
    "done-opacity",
];

fn color(v: &str) -> TokenValue {
    TokenValue::Color(Color(v.into()))
}

/// The world's installed value for `name`.
fn installed(world: &World, name: &str) -> Option<TokenValue> {
    world.enter(|| runtime_vocabulary::glue::token_value(name))
}

// ===========================================================================
// Names
// ===========================================================================

#[test]
fn token_names_are_kebab_field_paths_and_a_shape_takes_each_prefix() {
    assert_eq!(TodoTheme::NAMES, ALL_NAMES);
    assert_eq!(TodoTheme::PATHS[6], "accent.soft_bg");
    assert_eq!(TodoTheme::PATHS[16], "done_opacity");

    let t = TodoTheme::tokens();
    assert_eq!(t.surface().name(), Some("surface"));
    assert_eq!(t.accent.soft_bg().name(), Some("accent-soft-bg"));
    assert_eq!(t.danger.soft_bg().name(), Some("danger-soft-bg"), "the same shape under a second prefix");
    assert_eq!(t.space.lg().name(), Some("space-lg"));
    assert_eq!(t.done_opacity().name(), Some("done-opacity"));
    // A namespace is reachable as a method too.
    assert_eq!(t.accent().solid_bg().name(), Some("accent-solid-bg"));
}

/// A theme's entries are its names zipped with its values, in order, and
/// a skipped field contributes nothing.
#[test]
fn token_entries_pair_every_name_with_its_value() {
    let entries = dark().token_entries();
    let names: Vec<&str> = entries.iter().map(|e| e.name).collect();
    assert_eq!(names, ALL_NAMES);
    assert_eq!(entries[1].value, color("#111827"));
    assert_eq!(entries[6].value, color("#1e1b4b"));
    assert_eq!(entries[11].value, color("#0ea5e9"), "a TokenLeaf newtype installs as its Value kind");
    assert_eq!(entries[13].value, TokenValue::Length(Length::Px(8.0)));
    assert_eq!(entries[16].value, TokenValue::Number(0.5));
}

#[test]
fn a_custom_leaf_reads_as_its_registry_kind() {
    let brand: runtime_shared::Tokenized<Color> = TodoTheme::tokens().brand();
    assert_eq!(brand.name(), Some("brand"));
    assert_eq!(brand.value(), &Color("#0ea5e9".into()));
}

/// A theme prefix namespaces every token name, leaving accessor paths
/// alone.
#[test]
fn a_prefix_namespaces_every_name() {
    #[derive(Tokens, Clone)]
    #[tokens(default = brand_default, prefix = "brand")]
    struct BrandTheme {
        primary: Color,
        accent: Swatch,
    }
    fn brand_default() -> BrandTheme {
        BrandTheme {
            primary: "#000".into(),
            accent: Swatch { solid_bg: "#111".into(), solid_text: "#222".into(), soft_bg: "#333".into() },
        }
    }
    assert_eq!(
        BrandTheme::NAMES,
        ["brand-primary", "brand-accent-solid-bg", "brand-accent-solid-text", "brand-accent-soft-bg"]
    );
    assert_eq!(BrandTheme::PATHS[1], "accent.solid_bg");
    assert_eq!(BrandTheme::tokens().accent.soft_bg().name(), Some("brand-accent-soft-bg"));
}

// ===========================================================================
// .with
// ===========================================================================

#[test]
fn with_returns_a_modified_copy_and_leaves_the_base_alone() {
    let base = light();
    let variant = base.with(|t| t.accent.solid_bg = "#000000".into());
    assert_eq!(base, light(), "the base theme is untouched");
    assert_eq!(variant.accent.solid_bg, Color("#000000".into()));
    assert_eq!(variant.accent.soft_bg, base.accent.soft_bg, "everything else carries over");
}

// ===========================================================================
// Fallbacks
// ===========================================================================

/// The fallback contract: every accessor's fallback is the DEFAULT
/// theme's value (`#[tokens(default = light)]`), whatever is installed —
/// it is only observed before an install, in a web `var()`'s second
/// argument, and in the catalog. The installed value always wins at
/// resolve time.
#[test]
fn fallbacks_come_from_the_default_theme_and_installed_values_win() {
    let world = World::new();
    let t = TodoTheme::tokens();

    // Before any install: resolve yields the default theme's value.
    world.enter(|| assert_eq!(t.surface().resolve(), Color("#ffffff".into())));

    world.enter(|| TodoTheme::install(dark()));
    world.flush();
    world.enter(|| {
        assert_eq!(t.surface().value(), &Color("#ffffff".into()), "fallback is the default, not the installed theme");
        assert_eq!(t.surface().resolve(), Color("#111827".into()), "the installed value wins");
        assert_eq!(t.space.md().resolve(), Length::Px(8.0));
        assert_eq!(t.done_opacity().resolve(), 0.5);
    });
}

// ===========================================================================
// Install / swap
// ===========================================================================

#[test]
fn install_then_set_writes_new_values_under_the_same_names() {
    let world = World::new();
    world.enter(|| TodoTheme::install(light()));
    world.flush();
    assert_eq!(installed(&world, "surface"), Some(color("#ffffff")));
    assert_eq!(installed(&world, "accent-soft-bg"), Some(color("#eef2ff")));
    assert_eq!(installed(&world, "danger-soft-bg"), Some(color("#fef2f2")));
    assert_eq!(installed(&world, "space-lg"), Some(TokenValue::Length(Length::Px(16.0))));
    assert_eq!(installed(&world, "name"), None, "a skipped field is not a token");

    world.enter(|| TodoTheme::set(dark()));
    world.flush();
    assert_eq!(installed(&world, "surface"), Some(color("#111827")));
    assert_eq!(installed(&world, "accent-soft-bg"), Some(color("#1e1b4b")));
    assert_eq!(installed(&world, "accent-solid-bg"), Some(color("#4f46e5")), "unchanged by .with");
    let mut names = world.enter(runtime_vocabulary::glue::token_names);
    names.sort_unstable();
    let mut expected = ALL_NAMES.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected, "a swap adds no names and drops none");
}

/// `set` from a platform event handler — outside `World::enter` — must
/// not panic and commits on the owning world's next flush.
#[test]
fn set_from_an_event_handler_commits_on_flush() {
    let world = World::new();
    world.enter(|| TodoTheme::install(light()));
    world.flush();
    TodoTheme::set(dark());
    world.flush();
    assert_eq!(installed(&world, "surface"), Some(color("#111827")));
}

/// The `app_background` token themes the host surface on install AND on
/// every swap (native backends apply the resolved value, so a swap must
/// re-publish it).
#[test]
fn app_background_follows_install_and_swap() {
    let h = Harness::new();
    h.record_all();
    let world = h.world.clone();
    let _realized = world.enter(|| {
        TodoTheme::install(light());
        realize(&h.backend, &h.registry, view().style(TodoRow()).build())
    });
    let log = h.take_log().join("\n");
    assert!(
        log.contains(r##"set_app_background Token { name: "background", fallback: Color("#f3f4f6") }"##),
        "{log}"
    );

    world.enter(|| TodoTheme::set(dark()));
    world.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains(r##"set_app_background Token { name: "background", fallback: Color("#030712") }"##),
        "{log}"
    );
}

// ===========================================================================
// follow_system
// ===========================================================================

fn follow_system_under(scheme: ColorScheme) -> (Harness, String) {
    runtime_shared::install_current_color_scheme(scheme);
    let h = Harness::new();
    h.record_all();
    let world = h.world.clone();
    let realized = world.enter(|| {
        TodoTheme::follow_system(light(), dark());
        realize(&h.backend, &h.registry, view().style(TodoRow()).build())
    });
    std::mem::forget(realized);
    let log = h.take_log().join("\n");
    runtime_shared::install_current_color_scheme(ColorScheme::Auto);
    (h, log)
}

#[test]
fn follow_system_installs_the_preferred_palette_and_declares_both() {
    let (h, log) = follow_system_under(ColorScheme::Dark);
    assert_eq!(installed(&h.world, "surface"), Some(color("#111827")), "dark preference → dark active");
    assert!(
        log.contains(r#"install_theme_palettes ["light(Some(Light))=17", "dark(Some(Dark))=17"]"#),
        "both palettes reach the backend, complete:\n{log}"
    );
    let tokens_at = log.find("install_tokens").expect("tokens delivered");
    let palettes_at = log.find("install_theme_palettes").expect("palettes delivered");
    assert!(tokens_at < palettes_at, "palettes are serialized relative to the active set, so come after it");

    let (h, _) = follow_system_under(ColorScheme::Light);
    assert_eq!(installed(&h.world, "surface"), Some(color("#ffffff")));
}

// ===========================================================================
// install_reactive
// ===========================================================================

#[test]
fn install_reactive_swaps_when_the_selector_signal_changes() {
    let world = World::new();
    let dark_mode = world.enter(|| {
        let dark_mode = signal(false);
        TodoTheme::install_reactive(move || if dark_mode.get() { dark() } else { light() });
        dark_mode
    });
    world.flush();
    assert_eq!(installed(&world, "surface"), Some(color("#ffffff")));

    world.enter(|| dark_mode.set(true));
    world.flush();
    assert_eq!(installed(&world, "surface"), Some(color("#111827")));
    assert_eq!(installed(&world, "accent-soft-bg"), Some(color("#1e1b4b")));

    world.enter(|| dark_mode.set(false));
    world.flush();
    assert_eq!(installed(&world, "surface"), Some(color("#ffffff")));
}

/// A second `install_reactive` replaces the first: the first selector's
/// signal no longer drives the theme.
#[test]
fn install_reactive_again_disposes_the_previous_selector() {
    let world = World::new();
    let (first, second) = world.enter(|| {
        let first = signal(false);
        TodoTheme::install_reactive(move || if first.get() { dark() } else { light() });
        let second = signal(false);
        TodoTheme::install_reactive(move || if second.get() { dark() } else { light() });
        (first, second)
    });
    world.flush();

    world.enter(|| first.set(true));
    world.flush();
    assert_eq!(installed(&world, "surface"), Some(color("#ffffff")), "the replaced selector is gone");

    world.enter(|| second.set(true));
    world.flush();
    assert_eq!(installed(&world, "surface"), Some(color("#111827")));
}

// ===========================================================================
// stylesheet! over the derived vocabulary
// ===========================================================================

/// The sheet's rules carry token REFERENCES (names + default fallbacks),
/// and resolving them reads the installed theme — before and after a swap.
#[test]
fn a_stylesheet_over_the_theme_resolves_installed_values_and_re_resolves_after_set() {
    let world = World::new();
    world.enter(|| TodoTheme::install(light()));
    world.flush();

    let rules = world.enter(|| runtime_shared::resolve_style(&StyleApplication::new(todo_row_style())));
    let bg = rules.background.clone().expect("background set");
    assert_eq!(bg.name(), Some("surface"));
    let border = rules.border_top_color.clone().expect("border color set");
    assert_eq!(border.name(), Some("accent-soft-bg"));
    let pad = rules.padding_left.expect("padding set");
    assert_eq!(pad.name(), Some("space-md"));
    world.enter(|| {
        assert_eq!(bg.resolve(), Color("#ffffff".into()));
        assert_eq!(border.resolve(), Color("#eef2ff".into()));
        assert_eq!(pad.resolve(), Length::Px(8.0));
    });

    world.enter(|| TodoTheme::set(dark()));
    world.flush();
    let rules = world.enter(|| runtime_shared::resolve_style(&StyleApplication::new(todo_row_style())));
    world.enter(|| {
        assert_eq!(rules.background.as_ref().unwrap().resolve(), Color("#111827".into()));
        assert_eq!(rules.border_top_color.as_ref().unwrap().resolve(), Color("#1e1b4b".into()));
    });

    let clear = world.enter(|| runtime_shared::resolve_style(&StyleApplication::new(clear_button_style())));
    world.enter(|| {
        assert_eq!(clear.background.as_ref().unwrap().resolve(), Color("#dc2626".into()));
        assert_eq!(clear.opacity.as_ref().unwrap().resolve(), 0.5);
    });
}

/// End to end through the backend seam: a mounted node styled by the
/// sheet is applied with the installed colour, and a `set` re-applies it
/// with the new one (the mock's apply line resolves the background the
/// way a non-cascading native backend does at apply time).
#[test]
fn a_mounted_node_re_tints_when_the_theme_is_set() {
    let h = Harness::new();
    h.set_style_line(|node, rules| {
        let bg = rules.background.as_ref().map(|b| b.resolve().0).unwrap_or_default();
        format!("apply_style n{node} background={bg}")
    });
    let world = h.world.clone();
    let realized = world.enter(|| {
        TodoTheme::install(light());
        realize(&h.backend, &h.registry, view().style(TodoRow()).build())
    });
    let log = h.take_log();
    let names = format!("install_tokens {:?}", ALL_NAMES);
    assert!(log.contains(&names), "every token installs under its derived name: {log:?}");
    assert!(log.contains(&"apply_style n0 background=#ffffff".to_string()), "{log:?}");

    world.enter(|| TodoTheme::set(dark()));
    world.flush();
    let log = h.take_log();
    assert!(log.contains(&format!("update_tokens {:?}", ALL_NAMES)), "{log:?}");
    assert!(log.contains(&"apply_style n0 background=#111827".to_string()), "{log:?}");
    drop(realized);
}

// ===========================================================================
// Catalog
// ===========================================================================

/// What the catalog lists for the theme: name, accessor path, namespace
/// (first path segment), kind, and the DEFAULT theme's value.
#[test]
fn catalog_rows_describe_every_token() {
    let rows = runtime_vocabulary::tokens::__catalog_rows::<TodoTheme>();
    assert_eq!(rows.len(), ALL_NAMES.len());
    let soft = rows.iter().find(|r| r.name == "accent-soft-bg").unwrap();
    assert_eq!(
        (soft.path, soft.namespace, soft.kind, soft.default_value.as_str(), soft.vocabulary),
        ("accent.soft_bg", "accent", "Color", "#eef2ff", "TodoTheme")
    );
    let lg = rows.iter().find(|r| r.name == "space-lg").unwrap();
    assert_eq!((lg.kind, lg.default_value.as_str()), ("Length", "16px"));
    let op = rows.iter().find(|r| r.name == "done-opacity").unwrap();
    assert_eq!((op.path, op.namespace, op.kind, op.default_value.as_str()), ("done_opacity", "done_opacity", "Number", "0.6"));
}

/// With the catalog on, the derive's set registration surfaces every
/// token through the same `style_tokens()` iterator `list_tokens` reads.
#[cfg(feature = "catalog")]
#[test]
fn catalog_lists_the_theme_tokens() {
    let entries: Vec<_> =
        runtime_vocabulary::glue::__mcp::style_tokens().filter(|e| e.vocabulary == "TodoTheme").collect();
    let mut names: Vec<&str> = entries.iter().map(|e| e.name).collect();
    names.sort_unstable();
    let mut expected = ALL_NAMES.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected);
    let surface = entries.iter().find(|e| e.name == "surface").unwrap();
    assert_eq!((surface.path, surface.value_type), ("surface", "Color"));
    assert_eq!(surface.default_value.get(), "#ffffff");
}
