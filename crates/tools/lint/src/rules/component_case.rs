//! `component-pascal-case` — a function annotated `#[component]` must be
//! named in PascalCase.
//!
//! The convention is load-bearing, not cosmetic: inside `ui!` / `jsx!`, a
//! PascalCase tag routes to `#[component]` (`BuildElement`) dispatch while
//! a lowercase tag is a framework primitive (`view`, `text`, …). A
//! `#[component] fn icon_button` would have to be called as `IconButton`
//! at the tag site anyway, so a snake_case definition is a guaranteed
//! mismatch waiting to confuse. This rule pins the convention at the
//! definition.

use crate::diagnostic::RawDiag;

pub(crate) const RULE: &str = "component-pascal-case";

pub(crate) fn check_fn(item: &syn::ItemFn, out: &mut Vec<RawDiag>) {
    if !has_component_attr(&item.attrs) {
        return;
    }
    let ident = &item.sig.ident;
    let name = ident.to_string();
    if is_pascal_case(&name) {
        return;
    }
    out.push(
        RawDiag::new(
            RULE,
            format!("component `{name}` should be PascalCase"),
            ident.span(),
        )
        .with_help(format!(
            "rename to `{}` — components are PascalCase, primitives are lowercase",
            to_pascal_case(&name)
        )),
    );
}

/// True when the function carries a `#[component]` / `#[component(…)]`
/// attribute, including a path-qualified `#[runtime_macros::component]`.
fn has_component_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path()
            .segments
            .last()
            .map(|s| s.ident == "component")
            .unwrap_or(false)
    })
}

/// PascalCase = starts with an ASCII uppercase letter and contains no
/// underscores. Acronym-heavy names (`URLBar`) pass; that's fine — the
/// rule's job is to reject snake_case / lowercase, not to enforce a
/// particular acronym style.
pub(crate) fn is_pascal_case(name: &str) -> bool {
    match name.chars().next() {
        Some(c) if c.is_ascii_uppercase() => !name.contains('_'),
        _ => false,
    }
}

/// Best-effort `snake_case` / `lowerCamel` → `PascalCase` for the fix hint.
fn to_pascal_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut capitalize_next = true;
    for ch in name.chars() {
        if ch == '_' {
            capitalize_next = true;
            continue;
        }
        if capitalize_next {
            out.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() {
        name.to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case_recognition() {
        assert!(is_pascal_case("Button"));
        assert!(is_pascal_case("IconButton"));
        assert!(is_pascal_case("URLBar"));
        assert!(!is_pascal_case("button"));
        assert!(!is_pascal_case("icon_button"));
        assert!(!is_pascal_case("Icon_Button"));
        assert!(!is_pascal_case(""));
    }

    #[test]
    fn pascal_suggestion() {
        assert_eq!(to_pascal_case("icon_button"), "IconButton");
        assert_eq!(to_pascal_case("button"), "Button");
        assert_eq!(to_pascal_case("my_cool_card"), "MyCoolCard");
    }
}
