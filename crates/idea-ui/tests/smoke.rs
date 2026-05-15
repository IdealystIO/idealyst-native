//! Integration test that exercises every idea-ui component through
//! the `ui!` DSL — using the macros from a separate crate context,
//! which is what real consumers do. If the macros aren't properly
//! exported, this test fails at compile time.

use std::rc::Rc;

use framework_core::{component, signal, ui, Color, Primitive, Signal};
use idea_ui::{
    install_idea_theme, light_theme, AvatarSize, BodyTone, CardTone, Danger, FieldSize, Ghost,
    HeadingKind, IconButtonSize, IdeaTheme, Intent, IntentPalette, IntoRcIntent, Neutral, Primary,
    Secondary, SkeletonWidth, StackAlign, StackGap, StackJustify, Success, Tab, Warning,
};
// Bring the export macros into scope so `ui!`'s lowered `vstack!(...)`
// etc. resolves.
use idea_ui::{
    alert, avatar, badge, body, caption, card, center, divider, field, heading, hstack,
    iconbutton, pressable, skeleton, spacer, spinner, switch, tabs, tag, vstack,
};

#[component]
fn demo() -> Primitive {
    install_idea_theme(light_theme());

    let name: Signal<String> = signal!(String::new());
    let dark: Signal<bool> = signal!(false);

    let on_name_change: Rc<dyn Fn(String)> = Rc::new(move |v| name.set(v));
    let on_dark_change: Rc<dyn Fn(bool)> = Rc::new(move |v| dark.set(v));
    let on_save: Rc<dyn Fn()> = Rc::new(|| {});

    ui! {
        VStack(gap = StackGap::Lg, align = StackAlign::Stretch) {
            Heading(content = "Heading H1".to_string(), kind = HeadingKind::H1)
            Heading(content = "Heading H2".to_string(), kind = HeadingKind::H2)
            Body(content = "Body text".to_string(), tone = BodyTone::Default)
            Body(content = "Muted body".to_string(), tone = BodyTone::Muted)
            Caption(content = "Caption / helper text".to_string())

            HStack(gap = StackGap::Sm, justify = StackJustify::Between) {
                Pressable(
                    label = "Primary".to_string(),
                    on_click = on_save.clone(),
                    intent = Primary.into_rc()
                )
                Pressable(
                    label = "Secondary".to_string(),
                    on_click = on_save.clone(),
                    intent = Secondary.into_rc()
                )
                Pressable(
                    label = "Neutral".to_string(),
                    on_click = on_save.clone(),
                    intent = Neutral.into_rc()
                )
                Pressable(
                    label = "Ghost".to_string(),
                    on_click = on_save.clone(),
                    intent = Ghost.into_rc()
                )
                Pressable(
                    label = "Danger".to_string(),
                    on_click = on_save.clone(),
                    intent = Danger.into_rc()
                )
            }

            Card(tone = CardTone::Elevated) {
                Heading(content = "Card title".to_string(), kind = HeadingKind::H3)
                Body(content = "Card body content goes here.".to_string())
                HStack(gap = StackGap::Xs) {
                    Badge(label = "New".to_string(), intent = Primary.into_rc())
                    Badge(label = "OK".to_string(), intent = Success.into_rc())
                }
            }

            Field(
                label = Some("Name".to_string()),
                value = name,
                on_change = on_name_change.clone(),
                placeholder = Some("Enter your name".to_string()),
                help = Some("Used only to greet you.".to_string()),
                size = FieldSize::Md
            )

            Switch(
                label = Some("Dark mode".to_string()),
                value = dark,
                on_change = on_dark_change.clone()
            )

            Divider()
            Spinner()

            // ---- new components ----

            // Layout helpers.
            HStack {
                Body(content = "Left".to_string())
                Spacer()
                Body(content = "Right".to_string())
            }
            Center {
                Spinner()
            }

            // IconButton — square Pressable variant.
            HStack(gap = StackGap::Sm) {
                IconButton(
                    glyph = "+".to_string(),
                    on_click = on_save.clone(),
                    intent = Primary.into_rc(),
                    size = IconButtonSize::Sm
                )
                IconButton(
                    glyph = "×".to_string(),
                    on_click = on_save.clone(),
                    intent = Ghost.into_rc()
                )
            }

            // Avatar — image + fallback.
            HStack(gap = StackGap::Sm) {
                Avatar(
                    initials = "AB".to_string(),
                    intent = Primary.into_rc(),
                    size = AvatarSize::Md
                )
                Avatar(
                    src = Some("https://example.com/me.png".to_string()),
                    initials = "ME".to_string()
                )
            }

            // Tag — pill with optional close.
            HStack(gap = StackGap::Xs) {
                Tag(label = "Rust".to_string(), intent = Primary.into_rc())
                Tag(
                    label = "Removable".to_string(),
                    intent = Neutral.into_rc(),
                    on_remove = Some(on_save.clone())
                )
            }

            // Alert — banner.
            Alert(
                title = "Heads up".to_string(),
                body = Some("Your trial expires in 3 days.".to_string()),
                intent = Warning.into_rc()
            )
            Alert(
                title = "Save failed".to_string(),
                intent = Danger.into_rc(),
                on_dismiss = Some(on_save.clone())
            )

            // Skeleton — loading placeholders.
            VStack(gap = StackGap::Sm) {
                Skeleton(height = 24.0, width = SkeletonWidth::Full)
                Skeleton(height = 16.0, width = SkeletonWidth::ThreeQuarter)
                Skeleton(height = 16.0, width = SkeletonWidth::Half)
            }
        }
    }
}

#[test]
fn every_component_compiles() {
    let _tree: Primitive = demo();
}

// ============================================================================
// Custom intent: prove the extension story works end-to-end.
// ============================================================================
//
// `Hype` is defined here in test code (a separate crate from idea-ui).
// It implements `Intent` against the public trait and uses the
// framework's color values directly. The same marker type then works
// inside `ui! { Pressable(intent = Hype.into_rc(), …) }` and
// `Badge(intent = Hype.into_rc(), …)` without any modification to
// idea-ui itself.

#[derive(Copy, Clone)]
pub struct Hype;

impl Intent for Hype {
    fn palette(&self, _theme: &dyn IdeaTheme) -> IntentPalette {
        IntentPalette {
            background:         Color("#ff00aa".into()),
            background_hover:   Color("#ff44bb".into()),
            background_pressed: Color("#cc0088".into()),
            foreground:         Color("#ffffff".into()),
            border:             None,
        }
    }
    fn cache_key(&self) -> u64 {
        0xAA_AA_AA_AA
    }
}

#[component]
fn hype_demo() -> Primitive {
    install_idea_theme(light_theme());
    let click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        VStack {
            Pressable(label = "Buy now".to_string(), on_click = click.clone(), intent = Hype.into_rc())
            Badge(label = "Hot".to_string(), intent = Hype.into_rc())
        }
    }
}

#[test]
fn custom_intent_compiles_and_runs() {
    let _tree: Primitive = hype_demo();
}

// ============================================================================
// Custom theme: prove the trait-based extension story works.
// ============================================================================
//
// `MyTheme` wraps `IdeaThemeDefaults` and adds an extra field.
// Idea-ui's stylesheets only see the trait methods, so the
// extension lives on the concrete type for app-level use without
// disturbing the library.

use idea_ui::{Colors, IdeaThemeDefaults, Radius, Spacing, Typography};

struct MyTheme {
    base: IdeaThemeDefaults,
    /// App-level extension: an extra brand-specific accent color.
    /// Idea-ui never sees this; only app-level code reaches for it.
    #[allow(dead_code)]
    pub brand_accent: String,
}

impl IdeaTheme for MyTheme {
    fn colors(&self) -> &Colors {
        self.base.colors()
    }
    fn spacing(&self) -> &Spacing {
        self.base.spacing()
    }
    fn radius(&self) -> &Radius {
        self.base.radius()
    }
    fn typography(&self) -> &Typography {
        self.base.typography()
    }
}

// ============================================================================
// Tabs — controlled selection + panel switching.
// ============================================================================

#[component]
fn tabs_demo() -> Primitive {
    install_idea_theme(light_theme());
    let active: Signal<String> = signal!("overview".to_string());

    // Each panel is built lazily — the closure runs the first time
    // the tab becomes active. Switching tabs drops the old panel's
    // scope, so any signals inside it free deterministically.
    let overview_panel: Rc<dyn Fn() -> Primitive> = Rc::new(|| {
        ui! {
            VStack {
                Heading(content = "Overview".to_string(), kind = HeadingKind::H2)
                Body(content = "Big-picture summary goes here.".to_string())
            }
        }
    });
    let activity_panel: Rc<dyn Fn() -> Primitive> = Rc::new(|| {
        ui! {
            VStack {
                Heading(content = "Activity".to_string(), kind = HeadingKind::H2)
                Body(content = "Recent events stream here.".to_string())
            }
        }
    });
    let settings_panel: Rc<dyn Fn() -> Primitive> = Rc::new(|| {
        ui! {
            VStack {
                Heading(content = "Settings".to_string(), kind = HeadingKind::H2)
                Body(content = "Configuration knobs live here.".to_string())
            }
        }
    });

    ui! {
        Tabs(
            selected = active,
            tabs = vec![
                Tab::new("overview", "Overview", overview_panel),
                Tab::new("activity", "Activity", activity_panel),
                Tab::new("settings", "Settings", settings_panel),
            ]
        )
    }
}

#[test]
fn tabs_compiles_and_runs() {
    let _tree: Primitive = tabs_demo();
}

#[test]
fn custom_theme_installs() {
    install_idea_theme(MyTheme {
        base: light_theme(),
        brand_accent: "#ff00aa".into(),
    });
    // Build a styled tree to force the stylesheet closures to run
    // against the custom theme. If the trait-object downcast or any
    // trait method dispatch were broken, this would panic.
    let _tree: Primitive = body(&idea_ui::BodyProps {
        content: "Hello from MyTheme".into(),
        ..Default::default()
    });
}
