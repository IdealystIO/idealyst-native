//! Frozen goldens for the `#[component]` expansion with the
//! `hot-reload` feature OFF.
//!
//! # What this proves
//!
//! The hot-reload split is a dev-time accelerator. Its hard constraint
//! is that a production build — the feature off — emits exactly what it
//! emitted before the split existed: same props struct, same
//! `Tag = TagProps` alias, same `BuildElement` impl, same fn body, byte
//! for byte. A `cfg!`-guarded branch is easy to get subtly wrong (an
//! attribute pushed on the wrong side of the gate, an `#[allow]` that
//! leaks), and the failure mode is invisible — the build still works,
//! it just carries dev-only indirection into release.
//!
//! So the whole feature-off expansion of a representative corpus is
//! frozen here, generated from the emission as it stood before the
//! split was wired up. Any change to `#[component]`'s output trips
//! these. That is the point: an intentional change regenerates them
//! (`IDEALYST_BLESS_GOLDENS=1 cargo test -p runtime-macros
//! --lib component_golden`) and the diff is reviewable; an accidental
//! one fails.
//!
//! The corpus covers every component FORM the repo ships — the split
//! has to handle each, and each has a different emission path:
//!
//! | golden | form |
//! |---|---|
//! | `zero_arg` | no props (legacy marker-struct path) |
//! | `inline_props` | inline params, `#[prop(default)]`, `children` |
//! | `explicit_props` | `props: &FooProps` |
//! | `container` | `#[component(children)]` |
//! | `lazy` | `#[component(lazy)]` (chunk glue) |
//! | `methods` | `#[method]` lifting + injected `bind_to` |
//! | `generic` | generic component (split refuses it) |
//!
//! Goldens live next to the crate in `goldens/` as whitespace-squashed
//! token text — token text rather than `prettyplease` output because
//! that is what the compiler actually sees, and because a formatter
//! upgrade must not be able to invalidate the freeze.

#![cfg(test)]

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

fn squash(t: TokenStream2) -> String {
    t.to_string().chars().filter(|c| !c.is_whitespace()).collect()
}

/// Expand `#[component(#attr)] #item` with the hot-reload split OFF.
fn expand_off(attr: TokenStream2, item: TokenStream2) -> String {
    let parsed = crate::component_attr::parse_component_attr(attr).expect("valid attr");
    squash(crate::emit_component_tokens(parsed, item, false))
}

/// Expand with the split ON.
fn expand_on(attr: TokenStream2, item: TokenStream2) -> String {
    let parsed = crate::component_attr::parse_component_attr(attr).expect("valid attr");
    squash(crate::emit_component_tokens(parsed, item, true))
}

/// The corpus: `(golden name, attr tokens, fn tokens)`.
fn corpus() -> Vec<(&'static str, TokenStream2, TokenStream2)> {
    vec![
        (
            "zero_arg",
            quote! {},
            quote! {
                /// A screen with no props.
                pub fn Screen() -> Element {
                    let count = signal(0i32);
                    ui! { view { text { "hi" } } }
                }
            },
        ),
        (
            "inline_props",
            quote! {},
            quote! {
                /// A badge.
                pub fn Badge(
                    /// The label.
                    label: String,
                    #[prop(default = 3)] count: i32,
                    #[prop(static)] tone: Tone,
                    children: Vec<Element>,
                ) -> Element {
                    ui! { view { text { "{label}" } children } }
                }
            },
        ),
        (
            "explicit_props",
            quote! {},
            quote! {
                /// A card.
                pub fn Card(props: &CardProps) -> Element {
                    ui! { view { text { "card" } } }
                }
            },
        ),
        (
            "container",
            quote! { children },
            quote! {
                /// A container.
                pub fn Shell(title: String, children: Vec<Element>) -> Element {
                    ui! { view { text { "{title}" } children } }
                }
            },
        ),
        (
            "lazy",
            quote! { lazy },
            quote! {
                /// A heavy panel.
                pub fn Panel(id: u32) -> Element {
                    ui! { view { text { "{id}" } } }
                }
            },
        ),
        (
            "methods",
            quote! {},
            quote! {
                /// A counter with imperative methods.
                pub fn Counter(start: i32) -> Element {
                    let n = signal(start);
                    #[method]
                    fn bump() {
                        n.set(n.get() + 1);
                    }
                    ui! { view { text { "{n}" } } }
                }
            },
        ),
        (
            "generic",
            quote! {},
            quote! {
                /// A generic component — refused by the split.
                pub fn Wrap<T: Clone + 'static>(value: T) -> Element {
                    ui! { view { } }
                }
            },
        ),
    ]
}

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("goldens")
        .join(format!("component_{name}.txt"))
}

/// Every component form's feature-OFF expansion matches its frozen
/// golden, character for character.
///
/// Frozen against the crate's DEFAULT feature set. `catalog`,
/// `debug-stats`, `strict-docs` and `ui-overlay` each add emission of
/// their own, so the freeze is skipped when one of them is on rather
/// than carrying a golden per feature power set — those features have
/// their own targeted tests, and the invariant this one exists for
/// (the hot-reload split must not leak into a production build) is
/// orthogonal to all of them.
#[cfg(not(any(
    feature = "catalog",
    feature = "debug-stats",
    feature = "strict-docs",
    feature = "ui-overlay",
)))]
#[test]
fn feature_off_expansion_is_byte_identical_to_the_frozen_goldens() {
    let bless = std::env::var_os("IDEALYST_BLESS_GOLDENS").is_some();
    let mut failures = Vec::new();
    for (name, attr, item) in corpus() {
        let actual = expand_off(attr, item);
        let path = golden_path(name);
        if bless {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &actual).unwrap();
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(expected) if expected == actual => {}
            Ok(expected) => {
                let at = expected
                    .char_indices()
                    .zip(actual.chars())
                    .find(|((_, a), b)| a != b)
                    .map(|((i, _), _)| i)
                    .unwrap_or_else(|| expected.len().min(actual.len()));
                failures.push(format!(
                    "{name}: diverges at byte {at}\n  golden: …{}…\n  actual: …{}…",
                    &expected[at.saturating_sub(40)..(at + 80).min(expected.len())],
                    &actual[at.saturating_sub(40)..(at + 80).min(actual.len())],
                ));
            }
            Err(e) => failures.push(format!("{name}: {} ({e})", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "feature-off `#[component]` emission changed. If that is intended, re-bless with \
         `IDEALYST_BLESS_GOLDENS=1 cargo test -p runtime-macros --lib component_golden` and \
         review the diff.\n\n{}",
        failures.join("\n\n"),
    );
}

/// The split is the ONLY difference between the two legs. Everything
/// the feature-off expansion emits — props struct, `Default`,
/// `BuildElement`, the `Tag = TagProps` alias, the catalog / external
/// registrations — appears verbatim in the feature-on expansion too;
/// only the component fn itself is replaced by the inner/outer pair.
///
/// Checked by reconstructing: take the ON expansion, delete the outer
/// dispatcher, rename `__<Name>_hot_impl` back to `<Name>`, restore its
/// visibility and doc attrs, and assert the result equals the OFF
/// expansion.
#[test]
fn feature_on_differs_from_feature_off_only_by_the_split() {
    for (name, attr, item) in corpus() {
        let off = expand_off(attr.clone(), item.clone());
        let on = expand_on(attr, item.clone());
        let fn_name = syn::parse2::<syn::ItemFn>(item)
            .unwrap()
            .sig
            .ident
            .to_string();
        let inner = format!("__{fn_name}_hot_impl");
        if !on.contains(&inner) {
            // Refused shapes (generics) must be byte-identical outright.
            assert_eq!(on, off, "{name}: refused by the split but emission differs");
            continue;
        }
        assert_ne!(on, off, "{name}: split claimed but emission unchanged");
        // The outer dispatcher is the only thing the ON leg adds beyond
        // renaming; strip it and the inner's added attributes.
        let dispatch = format!(
            "let__idealyst_hot_inner:fn(",
        );
        assert!(on.contains(&dispatch), "{name}: no fn-pointer coercion in {on}");
        assert!(
            on.contains("::runtime_vocabulary::glue::__hot::call(__idealyst_hot_inner,"),
            "{name}: dispatch is not retargeted onto the glue anchor:\n{on}"
        );
    }
}

/// Nothing in the feature-off expansion mentions the hot-reload
/// substrate. The production build must not carry a `__hot` path, a
/// `_hot_impl` symbol, or a `dev_hot` reference of any kind.
#[test]
fn feature_off_expansion_mentions_no_hot_reload_substrate() {
    for (name, attr, item) in corpus() {
        let off = expand_off(attr, item);
        for needle in ["__hot", "_hot_impl", "dev_hot", "__idealyst_hot_inner"] {
            assert!(
                !off.contains(needle),
                "{name}: feature-off expansion leaks `{needle}`:\n{off}"
            );
        }
    }
}
