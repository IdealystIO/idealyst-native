//! `markdown-demo` — exercises the `markdown` SDK end to end.
//!
//! Renders one rich CommonMark/GFM document through the [`Markdown`]
//! component. On native backends the whole document is ONE styled-text
//! node (`NSAttributedString` / `SpannableString`); on web it's semantic
//! DOM. A light/dark toggle swaps the `MdTheme` — proving the styles
//! re-apply on theme change.
//!
//! The page is wrapped in a top-level reactive `switch` keyed on the
//! `dark` signal: toggling rebuilds the themed container (so the page
//! background follows the theme too) and the `Markdown` inside it with
//! the matching `MdTheme`. This is the same reactive-rebuild mechanism
//! the `Markdown` component uses internally for a live `theme` prop —
//! here it also paints the surrounding background, which `MdTheme`
//! (a per-element text style) intentionally does not own.

use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding};
use markdown::{Markdown, MdTheme};
use runtime_core::{
    button, signal, switch, ui, view, Color, Element, IntoElement, Length, StyleRules, StyleSheet,
    Tokenized,
};

/// No per-platform registration needed: the `markdown` external
/// self-registers via `inventory::submit!` at backend construction (see
/// [[project_inventory_self_registration]]). The crate stays linked through
/// the `Markdown`/`MdTheme` references in this module.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

/// The document we render — covers every block + inline feature the SDK
/// supports.
const SAMPLE: &str = r#"# Markdown SDK

A whole document rendered as **one native styled-text node** per backend
— `NSAttributedString` on iOS, `SpannableString` on Android, semantic DOM
on web. Inline styles are *attribute ranges*, not per-token widgets.

## Inline formatting

Text can be **bold**, *italic*, ***both***, `inline code`, ~~struck
through~~, and [a styled link](https://example.com). These all live in a
single attributed string.

## Lists

- First bullet
- Second bullet with **bold** inside
  - A nested item
  - Another nested item
- Back to the top level

1. Ordered one
2. Ordered two
3. Ordered three

## Code

A fenced block keeps its monospace font and a tinted background:

```
fn main() {
    println!("hello, markdown");
}
```

## Quote

> The best way to predict the future is to invent it.
> — paraphrasing Alan Kay

---

Toggle the theme above to watch every color update live.
"#;

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Single source of truth for the demo's light/dark state.
    let dark = signal!(false);

    // Reactive region: re-evaluated whenever `dark` flips. Rebuilds the
    // themed page container + the Markdown with the matching MdTheme.
    switch(move || dark.get(), move |&is_dark| page(is_dark, dark))
}

/// Build the themed page for a given light/dark state.
fn page(is_dark: bool, dark: runtime_core::Signal<bool>) -> Element {
    let theme = if is_dark { MdTheme::dark() } else { MdTheme::light() };
    let page_bg = if is_dark { "#0d1117" } else { "#ffffff" };

    let toggle = button(
        if is_dark { "Switch to light" } else { "Switch to dark" },
        move || dark.set(!dark.get()),
    )
    .into_element();

    let body: Vec<Element> = vec![
        toggle,
        ui! { Markdown(source = SAMPLE.to_string(), theme = theme) },
    ];

    let content = ui! {
        scroll_view {
            Stack(gap = StackGap::Lg, padding = StackPadding::Lg) { body }
        }
    };

    // Full-size container painting the page background — `MdTheme` styles
    // text, not the surrounding surface, so the demo owns the backdrop.
    let bg_style = Rc::new(StyleSheet::r#static(StyleRules {
        background: Some(Tokenized::Literal(Color(page_bg.to_string()))),
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    }));

    view(vec![content]).with_style(bg_style).into_element()
}
