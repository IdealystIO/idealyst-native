//! Shared primitive-tag recognition for the `ui!` and `jsx!` macros.
//!
//! Primitives are the fixed set of framework leaf tags (`view`, `text`,
//! `button`, `text_input`, `scroll_view`, …) that lower to free functions
//! in `runtime_core` rather than going through `BuildElement` struct-literal
//! dispatch. The canonical (and only accepted) form is **snake_case**,
//! matching the underlying `runtime_core::view(...)` / `runtime_core::text_input(...)`
//! builder fn names and React's `<div>` / `<input>` lowercase-intrinsic
//! convention.
//!
//! PascalCase tags are **always** routed to user-component dispatch. This
//! is what lets an app or component library define a `#[component]` named
//! `Image`, `Link`, `Toggle`, `Slider`, etc. without the framework
//! primitive of the same name shadowing it. The lowercase form (`image`,
//! `link`, …) is the primitive; the PascalCase form (`Image`, `Link`, …)
//! is whatever component is in scope. (Historically the macro also
//! accepted PascalCase primitive tags for back-compat; that fallback was
//! removed once every call site migrated to snake_case.)

/// If `name` is a recognized primitive tag, return its canonical
/// snake_case form. Only the snake_case spelling is recognized —
/// PascalCase names return `None` so the caller dispatches to
/// user-component code (`emit_user`).
pub(crate) fn canonical_primitive(name: &str) -> Option<&'static str> {
    match name {
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
    fn canonical_matches_snake_case_only() {
        assert_eq!(canonical_primitive("view"), Some("view"));
        assert_eq!(canonical_primitive("text_input"), Some("text_input"));
        assert_eq!(canonical_primitive("anchored_overlay"), Some("anchored_overlay"));
    }

    #[test]
    fn pascal_case_primitives_route_to_user_component() {
        // PascalCase forms are NO LONGER recognized as primitives — they
        // fall through to component dispatch, so an app can define a
        // `#[component]` named `View`/`Image`/`Link`/`Toggle`/… and have
        // it win. The lowercase spelling is the primitive.
        assert_eq!(canonical_primitive("View"), None);
        assert_eq!(canonical_primitive("Text"), None);
        assert_eq!(canonical_primitive("Image"), None);
        assert_eq!(canonical_primitive("Link"), None);
        assert_eq!(canonical_primitive("Toggle"), None);
        assert_eq!(canonical_primitive("TextInput"), None);
        assert_eq!(canonical_primitive("Button"), None);
    }

    #[test]
    fn canonical_rejects_unknown() {
        assert_eq!(canonical_primitive("MyComponent"), None);
        assert_eq!(canonical_primitive("Pressable"), None);
        assert_eq!(canonical_primitive("DrawerNavigator"), None);
    }
}
