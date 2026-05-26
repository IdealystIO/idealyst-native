//! Unit tests for the `docs!` macro.
//!
//! Lives inside the docs example crate because the macro's emission
//! references shell components (`PageHeader`, `Card`, etc.) whose
//! `macro_rules!` invocations are only in scope inside this crate.

#![cfg(test)]

use crate::meta::{
    BlockMeta, ComparisonFramework, DocConcept, NoteKind, PageCategory, Span,
};

// A demo function the page references. The macro emits a real call;
// if it didn't exist, the docs! invocation wouldn't compile.
#[allow(dead_code)]
fn counter_demo() -> runtime_core::Primitive {
    use runtime_core::IntoPrimitive;
    runtime_core::view(Vec::new()).into_primitive()
}

mod reactivity {
    use super::counter_demo;
    use docs_macro::docs;
    // The macro emits `ui! { Stack { Card { Heading; Body; ... } } }`
    // which the framework lowers to invocations of `idea_ui`'s
    // exported macros and the docs site's shell macros.
    //
    // - idea_ui macros (Stack/Card/Heading/Body) are `#[macro_export]`
    //   with `$crate::...` paths inside, so importing the function
    //   bridges the macro lowering.
    // - The shell's `#[component]`-generated macros expand to bare
    //   function calls + struct literals, so both the function and
    //   the props struct need to be in scope.
    #[allow(unused_imports)]
    use idea_ui::{typography, card, stack};
    #[allow(unused_imports)]
    use crate::shell::{code_block, page_header, CodeBlockProps, PageHeaderProps};

    docs! {
        slug = "reactivity",
        title = "Reactivity",
        category = Foundation,
        description = "The mechanism behind every change in an Idealyst app.",
        related = ["signals", "components"],
        concepts = [Signal, Effect, Scope, Derived, Untrack],

        section(heading = "The model in one paragraph") {
            p("A signal holds a value. When a closure reads it inside a tracked context, the framework records the dependency."),
            p("When the signal changes, the framework re-runs every tracked context that read it — and only those."),
        },

        section(heading = "Signals") {
            p("Make a signal with ", code("signal!(initial)"), ". Read with ",
              code(".get()"), " and write with ", code(".set(v)"), "."),
            code(rust, r#"
                let count = signal!(0);
                count.set(5);
                count.update(|n| *n += 1);
            "#),
            p("Signals are the only kind of state the framework knows about."),
        },

        section(heading = "What gets tracked") {
            p("A signal read inside any of these contexts subscribes to it:"),
            list(
                ["Reactive ", code("Text"), " content"],
                ["Reactive ", code("if"), " inside ", code("ui!")],
                ["Reactive ", code("for"), " over a signal-backed list"],
                ["A manual ", code("Effect::new"), " closure"],
            ),
        },

        section(heading = "Comparisons") {
            compare(from = React) {
                p("A signal is not ", code("useState"), ". ", code("useState"),
                  " triggers a re-render of the component; a signal notifies only the reads that depended on it."),
            },
            compare(from = Solid) {
                p("Identical to ", code("createSignal"), ". Track-on-read, re-run on change, components run once."),
            },
        },

        section(heading = "Try it") {
            p("Press the button. The count survives every kind of edit the hot patcher can apply."),
            demo(counter_demo, description = "Counter with one signal."),
        },

        note(kind = Tip) {
            p("If an effect re-fires more often than expected, look for an accidental ",
              code(".get()"), " in a place that didn't need to subscribe."),
        },
    }
}

#[test]
fn page_meta_top_level_fields() {
    let meta = &reactivity::PAGE_META;
    assert_eq!(meta.slug, "reactivity");
    assert_eq!(meta.title, "Reactivity");
    assert_eq!(meta.category, PageCategory::Foundation);
    assert_eq!(
        meta.description,
        Some("The mechanism behind every change in an Idealyst app.")
    );
    assert_eq!(meta.related, &["signals", "components"]);
    assert_eq!(
        meta.concepts,
        &[
            DocConcept::Signal,
            DocConcept::Effect,
            DocConcept::Scope,
            DocConcept::Derived,
            DocConcept::Untrack,
        ]
    );
}

#[test]
fn sections_present_with_slugs() {
    let meta = &reactivity::PAGE_META;
    assert_eq!(meta.sections.len(), 6);

    assert_eq!(meta.sections[0].heading, "The model in one paragraph");
    assert_eq!(meta.sections[0].slug, "the-model-in-one-paragraph");

    assert_eq!(meta.sections[1].heading, "Signals");
    assert_eq!(meta.sections[1].slug, "signals");

    assert_eq!(meta.sections[2].heading, "What gets tracked");
    assert_eq!(meta.sections[2].slug, "what-gets-tracked");

    assert_eq!(meta.sections[3].heading, "Comparisons");
    assert_eq!(meta.sections[3].slug, "comparisons");

    assert_eq!(meta.sections[4].heading, "Try it");
    assert_eq!(meta.sections[4].slug, "try-it");

    // Note has no heading; it becomes an empty-heading "section".
    assert_eq!(meta.sections[5].heading, "");
}

#[test]
fn paragraph_with_mixed_spans() {
    let meta = &reactivity::PAGE_META;
    let signals = &meta.sections[1];
    let block = &signals.blocks[0];
    if let BlockMeta::Paragraph(spans) = block {
        assert_eq!(spans.len(), 7);
        assert!(matches!(spans[0], Span::Text(t) if t.contains("Make a signal")));
        assert!(matches!(spans[1], Span::Code("signal!(initial)")));
        assert!(matches!(spans[2], Span::Text(_)));
        assert!(matches!(spans[3], Span::Code(".get()")));
        assert!(matches!(spans[4], Span::Text(_)));
        assert!(matches!(spans[5], Span::Code(".set(v)")));
        assert!(matches!(spans[6], Span::Text(_)));
    } else {
        panic!("expected paragraph, got {:?}", block);
    }
}

#[test]
fn code_block() {
    let meta = &reactivity::PAGE_META;
    let signals = &meta.sections[1];
    let block = &signals.blocks[1];
    if let BlockMeta::Code { language, source } = block {
        assert_eq!(*language, "rust");
        assert!(source.contains("let count = signal!(0);"));
        // Indentation has been trimmed.
        assert!(!source.starts_with("                "));
    } else {
        panic!("expected code block, got {:?}", block);
    }
}

#[test]
fn bulleted_list() {
    let meta = &reactivity::PAGE_META;
    let what_gets_tracked = &meta.sections[2];
    let block = &what_gets_tracked.blocks[1];
    if let BlockMeta::List(items) = block {
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].len(), 3);
        assert!(matches!(items[0][1], Span::Code("Text")));
    } else {
        panic!("expected list, got {:?}", block);
    }
}

#[test]
fn comparison_blocks() {
    let meta = &reactivity::PAGE_META;
    let comparisons = &meta.sections[3];
    assert_eq!(comparisons.blocks.len(), 2);

    if let BlockMeta::Comparison { from, .. } = &comparisons.blocks[0] {
        assert_eq!(*from, ComparisonFramework::React);
    } else {
        panic!("expected comparison");
    }

    if let BlockMeta::Comparison { from, .. } = &comparisons.blocks[1] {
        assert_eq!(*from, ComparisonFramework::Solid);
    } else {
        panic!("expected comparison");
    }
}

#[test]
fn demo_block() {
    let meta = &reactivity::PAGE_META;
    let try_it = &meta.sections[4];
    let demo = &try_it.blocks[1];
    if let BlockMeta::Demo { name, description } = demo {
        assert_eq!(*name, "counter_demo");
        assert_eq!(*description, Some("Counter with one signal."));
    } else {
        panic!("expected demo, got {:?}", demo);
    }
}

#[test]
fn note_block() {
    let meta = &reactivity::PAGE_META;
    let note_section = &meta.sections[5];
    assert_eq!(note_section.blocks.len(), 1);
    if let BlockMeta::Note { kind, .. } = &note_section.blocks[0] {
        assert_eq!(*kind, NoteKind::Tip);
    } else {
        panic!("expected note");
    }
}

#[test]
fn page_function_renders() {
    let _prim: runtime_core::Primitive = reactivity::page();
}
