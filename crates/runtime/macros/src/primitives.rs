//! Shared primitive-tag recognition for the `ui!` and `jsx!` macros.
//!
//! Primitives are the fixed set of framework leaf tags (`view`, `text`,
//! `button`, `text_input`, `scroll_view`, …) that lower to free functions
//! in `runtime_core` rather than going through `BuildElement` struct-literal
//! dispatch. The canonical form is **snake_case**, matching the underlying
//! `runtime_core::view(...)` / `runtime_core::text_input(...)` builder fn
//! names and React's `<div>` / `<input>` lowercase-intrinsic convention.
//!
//! `canonical_primitive` accepts either snake_case (`view`, `text_input`)
//! or PascalCase (`View`, `TextInput`) so call sites can migrate gradually;
//! once every site is on snake_case the PascalCase fallback can be deleted.

/// Convert a PascalCase identifier to snake_case. Idempotent on already
/// snake_case input: `View` → `view`, `TextInput` → `text_input`,
/// `text_input` → `text_input`.
fn to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_uppercase() && i > 0 {
            let prev = s.as_bytes()[i - 1] as char;
            if prev != '_' {
                out.push('_');
            }
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// If `name` is a recognized primitive tag (in either PascalCase or
/// snake_case form), return the canonical snake_case form. Otherwise
/// return `None`, signalling that the caller should dispatch to user-
/// component code (`emit_user`).
pub(crate) fn canonical_primitive(name: &str) -> Option<&'static str> {
    // `button` is special: idea-ui exposes a `Button` component that
    // should win when authors write `Button` in `ui!`. Only the
    // lowercase form routes to the primitive; PascalCase falls through
    // to user-component dispatch.
    if name == "Button" {
        return None;
    }
    let snake = to_snake(name);
    match snake.as_str() {
        "text" => Some("text"),
        "button" => Some("button"),
        "view" => Some("view"),
        "when" => Some("when"),
        "image" => Some("image"),
        "icon" => Some("icon"),
        "text_input" => Some("text_input"),
        "toggle" => Some("toggle"),
        "scroll_view" => Some("scroll_view"),
        "slider" => Some("slider"),
        "web_view" => Some("web_view"),
        "activity_indicator" => Some("activity_indicator"),
        "flat_list" => Some("flat_list"),
        "link" => Some("link"),
        "overlay" => Some("overlay"),
        "anchored_overlay" => Some("anchored_overlay"),
        "presence" => Some("presence"),
        "graphics" => Some("graphics"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_to_snake() {
        assert_eq!(to_snake("View"), "view");
        assert_eq!(to_snake("TextInput"), "text_input");
        assert_eq!(to_snake("ActivityIndicator"), "activity_indicator");
        assert_eq!(to_snake("AnchoredOverlay"), "anchored_overlay");
    }

    #[test]
    fn snake_passes_through() {
        assert_eq!(to_snake("view"), "view");
        assert_eq!(to_snake("text_input"), "text_input");
    }

    #[test]
    fn canonical_matches_both_cases() {
        assert_eq!(canonical_primitive("View"), Some("view"));
        assert_eq!(canonical_primitive("view"), Some("view"));
        assert_eq!(canonical_primitive("TextInput"), Some("text_input"));
        assert_eq!(canonical_primitive("text_input"), Some("text_input"));
    }

    #[test]
    fn canonical_rejects_unknown() {
        assert_eq!(canonical_primitive("MyComponent"), None);
        assert_eq!(canonical_primitive("Pressable"), None);
        assert_eq!(canonical_primitive("DrawerNavigator"), None);
    }

    #[test]
    fn pascal_button_routes_to_user_component() {
        // `Button` (PascalCase) is idea-ui's component; only lowercase
        // `button` is the primitive. See `canonical_primitive`.
        assert_eq!(canonical_primitive("Button"), None);
        assert_eq!(canonical_primitive("button"), Some("button"));
    }
}
