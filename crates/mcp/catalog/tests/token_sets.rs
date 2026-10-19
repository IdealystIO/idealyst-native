//! A `#[derive(Tokens)]` theme registers its whole vocabulary as one
//! `StyleTokenSetEntry`, and `style_tokens()` — what the catalog JSON and
//! `list_tokens` read — expands it into ordinary token entries.

use runtime_core::{Color, Length, Tokens};

#[derive(Tokens, Clone)]
pub struct Swatch {
    pub solid_bg: Color,
    pub soft_bg: Color,
}

#[derive(Tokens, Clone)]
#[tokens(default = light)]
pub struct CatalogTheme {
    pub surface: Color,
    pub accent: Swatch,
    pub gap: Length,
}

fn light() -> CatalogTheme {
    CatalogTheme {
        surface: "#ffffff".into(),
        accent: Swatch { solid_bg: "#4f46e5".into(), soft_bg: "#eef2ff".into() },
        gap: 12.0.into(),
    }
}

#[test]
fn a_token_set_expands_into_style_token_entries() {
    let mine: Vec<_> = mcp_catalog::style_tokens().filter(|e| e.vocabulary == "CatalogTheme").collect();
    let rows: Vec<(&str, &str, &str, &str, String)> = mine
        .iter()
        .map(|e| (e.name, e.path, e.namespace, e.value_type, e.default_value.get()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("surface", "surface", "surface", "Color", "#ffffff".to_string()),
            ("accent-solid-bg", "accent.solid_bg", "accent", "Color", "#4f46e5".to_string()),
            ("accent-soft-bg", "accent.soft_bg", "accent", "Color", "#eef2ff".to_string()),
            ("gap", "gap", "gap", "Length", "12px".to_string()),
        ]
    );
    // Expanded once: a second iteration yields the same entries, not a
    // second copy.
    assert_eq!(mcp_catalog::style_tokens().filter(|e| e.vocabulary == "CatalogTheme").count(), 4);
}

#[test]
fn the_catalog_document_carries_the_expanded_tokens() {
    let doc = mcp_catalog::catalog_json();
    let tokens = doc["style_tokens"].as_array().expect("style_tokens slice");
    let soft = tokens
        .iter()
        .find(|t| t["name"] == "accent-soft-bg")
        .expect("the derived token is in the document");
    assert_eq!(soft["path"], "accent.soft_bg");
    assert_eq!(soft["vocabulary"], "CatalogTheme");
    assert_eq!(soft["default_value"], "#eef2ff");
}
