//! End-to-end SSR demo for the Stack navigator. Renders the app at each
//! URL with the SSR handler registered (so header chrome appears) and
//! prints the full HTML document a server would send.
//!
//!   cargo run -p stack-navigator --example ssr_demo

#![cfg(not(target_arch = "wasm32"))]

use std::rc::Rc;

use backend_ssr::{render_document, render_path_with};
use runtime_core::primitives::link::link;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{
    set_page_metadata, text, view, Color, Element, Length, PageMetadata, Route, StyleRules,
    StyleSheet, Tokenized, VariantSet,
};
use stack_navigator::{Navigator, StackBuilder, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");

fn page_style() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| {
        let mut r = StyleRules::default();
        r.background = Some(Tokenized::Literal(Color("#0b1020".into())));
        r.color = Some(Tokenized::Literal(Color("#e5e7eb".into())));
        r.padding_top = Some(Tokenized::Literal(Length::Px(24.0)));
        r.gap = Some(Tokenized::Literal(Length::Px(8.0)));
        r
    }))
}

fn app() -> Element {
    Navigator::new(&HOME)
        .screen(HOME, |_| {
            set_page_metadata(PageMetadata {
                title: Some("Idealyst — Home".into()),
                description: Some("One Rust codebase, every platform.".into()),
                ..Default::default()
            });
            Screen::new(
                view(vec![
                    text("Welcome to Idealyst").into(),
                    text("This first paint was server-rendered.").into(),
                    link(&ABOUT, (), vec![text("About →").into()]).into(),
                ])
                .with_style(page_style()),
            )
            .title("Home")
        })
        .screen(ABOUT, |_| {
            set_page_metadata(PageMetadata {
                title: Some("About — Idealyst".into()),
                description: Some("Why Idealyst exists.".into()),
                ..Default::default()
            });
            Screen::new(
                view(vec![
                    text("About").into(),
                    text("Escaping check: a < b && c > d").into(),
                    link(&HOME, (), vec![text("← Home").into()]).into(),
                ])
                .with_style(page_style()),
            )
            .title("About")
        })
        .into()
}

fn main() {
    for path in ["/", "/about"] {
        let page = render_path_with(path, |b| stack_navigator::chrome::register(b), app);
        println!("\n================  GET {path}  ================");
        println!("{}", render_document(&page, None));
    }
}
