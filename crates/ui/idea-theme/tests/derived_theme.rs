//! idea's theme as a `#[derive(Tokens)]` set: an app theme extends it with
//! `#[tokens(flatten)]`, an unrelated app set that reuses one of idea's
//! names is refused, and the installed theme struct is readable.

use idea_theme::{install_idea_theme, light_theme, tokens, IdeaThemeDefaults};
use runtime_core::{Color, Length, TokenTheme, TokenValue, Tokens};

/// An app theme built on idea's: idea's tokens keep their names and
/// accessors, the app adds its own.
#[derive(Tokens, Clone)]
#[tokens(default = app_light)]
pub struct AppTheme {
    pub brand: Color,
    #[tokens(flatten)]
    pub idea: IdeaThemeDefaults,
}

fn app_light() -> AppTheme {
    AppTheme { brand: "#0ea5e9".into(), idea: light_theme() }
}

#[derive(Tokens, Clone)]
pub struct AppSpacing {
    pub md: Length,
}

/// An app set that independently names `spacing-md`.
#[derive(Tokens, Clone)]
#[tokens(default = clashing)]
pub struct Clashing {
    pub spacing: AppSpacing,
}

fn clashing() -> Clashing {
    Clashing { spacing: AppSpacing { md: 10.0.into() } }
}

#[test]
fn an_app_theme_extends_idea_with_flatten() {
    assert_eq!(AppTheme::NAMES[0], "brand");
    assert_eq!(&AppTheme::NAMES[1..], IdeaThemeDefaults::NAMES);
    let t = AppTheme::tokens();
    assert_eq!(t.brand().name(), Some("brand"));
    assert_eq!(t.color.surface().name(), Some("color-surface"));
    assert_eq!(t.intent.primary.solid_bg().name(), tokens().intent.primary.solid_bg().name());
    assert_eq!(t.radius.pill().value(), &Length::Full);

    // The app theme and idea-ui's own install coexist: the app's set
    // embeds idea's, so sharing its names is by design.
    let world = runtime_core::__World::new();
    world.enter(|| {
        install_idea_theme(light_theme());
        AppTheme::install(app_light().with(|t| t.idea.colors.surface = "#fafafa".into()));
    });
    world.flush();
    let surface = world.enter(|| runtime_core::token_value("color-surface"));
    assert_eq!(surface, Some(TokenValue::Color(Color("#fafafa".into()))));
}

/// The collision the shared registry used to let through: an app token
/// named `spacing-md` silently re-spacing every idea-ui component.
#[test]
#[should_panic(expected = "token `spacing-md` is already owned by token set `idea_theme::theme::IdeaThemeDefaults`")]
fn regression_an_app_set_reusing_an_idea_name_is_refused() {
    let world = runtime_core::__World::new();
    world.enter(|| {
        install_idea_theme(light_theme());
        Clashing::install(clashing());
    });
}

/// `install_idea_theme` installs through the derived set, so the theme
/// struct is readable as `IdeaThemeDefaults::current()`.
#[test]
fn the_installed_idea_theme_is_readable_as_a_struct() {
    let world = runtime_core::__World::new();
    let surface = world.enter(|| {
        install_idea_theme(idea_theme::dark_theme());
        IdeaThemeDefaults::current().colors.surface.value().clone()
    });
    assert_eq!(&surface, idea_theme::dark_theme().colors.surface.value());
}
