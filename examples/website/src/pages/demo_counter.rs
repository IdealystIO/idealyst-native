//! Counter — live reactive counter sitting beside its source.

use std::rc::Rc;

use runtime_core::{bind, signal, text_fmt, ui, Primitive, Ref, ViewHandle};
use idea_ui::{btn, card, stack, typography, ButtonKind, IntentTag, StackGap, TypographyKind, TypographyTone};

use crate::pages::common::{code_panel, page_header, page_section};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Primitive {
    let live_ref: Ref<ViewHandle> = Ref::new();
    let source_ref: Ref<ViewHandle> = Ref::new();
    let explain_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: live_ref, label: "Live counter" },
        TocEntry { handle: source_ref, label: "The whole source" },
        TocEntry { handle: explain_ref, label: "What just happened" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Counter",
                "The canonical reactive counter \u{2014} fifteen lines of code, no \
                 virtual DOM, no re-render passes. Click the buttons; the framework \
                 mutates exactly the text node bound to the count signal."
            ) }
            { page_section(live_ref, vec![live_demo()]) }
            { page_section(source_ref, vec![source()]) }
            { page_section(explain_ref, vec![explanation()]) }
        }
    };
    layout_with_toc(content, toc)
}

fn live_demo() -> Primitive {
    let count = signal!(0);
    // `Signal<T>` is `Copy`, so each closure owns its own handle to
    // the same reactive cell \u{2014} no shadowing dance, no `.clone()`.
    let increment: Rc<dyn Fn()> = Rc::new(move || count.update(|n| *n += 1));
    let decrement: Rc<dyn Fn()> = Rc::new(move || count.update(|n| *n -= 1));
    let reset: Rc<dyn Fn()> = Rc::new(move || count.set(0));

    let buttons: Vec<Primitive> = vec![
        ui! { Btn(label = "\u{2212}".to_string(), on_click = decrement, intent = IntentTag::Neutral, kind = ButtonKind::Soft) },
        ui! { Btn(label = "Reset".to_string(), on_click = reset, intent = IntentTag::Neutral, kind = ButtonKind::Ghost) },
        ui! { Btn(label = "+".to_string(), on_click = increment, intent = IntentTag::Primary, kind = ButtonKind::Solid) },
    ];
    let card_children: Vec<Primitive> = vec![
        ui! { Typography(content = "Live counter".to_string(), kind = TypographyKind::H3) },
        // `text_fmt!` builds a reactive `TextSource` that drops directly
        // into a `Text { ... }` child slot. `bind!(signal)` marks each
        // arg the framework should subscribe to \u{2014} the resulting
        // text node re-resolves on every signal write, with no
        // surrounding tree rebuild.
        ui! { Text { text_fmt!("Count: {}", bind!(count)) } },
        ui! { Stack(gap = StackGap::Md, axis = idea_ui::StackAxis::Row) { buttons } },
    ];
    ui! { Card { card_children } }
}

fn source() -> Primitive {
    let snippet = "use runtime_core::{bind, component, signal, text_fmt, ui, Primitive};\n\
                   \n\
                   #[component]\n\
                   pub fn counter() -> Primitive {\n    \
                       let count = signal!(0);\n    \
                       ui! {\n        \
                           Stack(gap = StackGap::Md) {\n            \
                               Text { text_fmt!(\"Count: {}\", bind!(count)) }\n            \
                               Button(\n                \
                                   label = \"+\",\n                \
                                   on_click = move || count.update(|n| *n += 1),\n            \
                               )\n            \
                               Button(\n                \
                                   label = \"\u{2212}\",\n                \
                                   on_click = move || count.update(|n| *n -= 1),\n            \
                               )\n        \
                           }\n    \
                       }\n\
                   }";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "The whole source".to_string(), kind = TypographyKind::H2) },
        code_panel(snippet),
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn explanation() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "What just happened".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Three things to notice.".to_string())
        },
        ui! { Typography(content = "1. The signal".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "`signal!(0)` allocates a reactive `i32` cell. Reading \
                `count.get()` inside a reactive scope subscribes that scope to the cell's \
                change set. Writing via `count.update(...)` or `count.set(...)` fires \
                every subscriber.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Typography(content = "2. Reactive text via `text_fmt!`".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "`text_fmt!(\"template\", bind!(signal))` builds a \
                reactive `TextSource` that drops directly into a `Text { ... }` child \
                slot. `bind!(signal)` marks each argument the framework should \
                subscribe to; the resulting text node re-resolves on every signal write, \
                with no surrounding tree rebuild.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Typography(content = "3. Closures own the signal".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "`move || count.update(...)` is the framework's standard \
                event-handler shape. `Signal<T>` is Copy, so the closures own their own \
                handle to the signal \u{2014} no shared mutable state, no stale-closure \
                bugs.".to_string(),
                tone = TypographyTone::Muted)
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
