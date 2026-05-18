//! Reactivity — Signals, Effects, and scoped lifecycles.

use framework_core::{ui, Primitive};
use idea_ui::{body, card, heading, stack, BodyTone, HeadingKind, StackGap};

use crate::shell::{
    codeblock, pageheader, section, CodeBlockProps, PageHeaderProps, SectionProps,
};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Reactivity".to_string(),
                description = "Fine-grained state. Updates surgically rerun only what depends on them.".to_string(),
            )

            Card {
                Heading(content = "Signals".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "A `Signal<T>` is the framework's reactive cell. It is `Copy`, \
                               cheap to clone, and arena-backed — passing it through closures \
                               and child components has no allocation cost. Use \
                               `signal!(value)` as shorthand for `Signal::new(value)`.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "use framework_core::{component, signal, ui, Primitive};\n\
                            \n\
                            #[component]\n\
                            pub fn counter() -> Primitive {\n    \
                                let count = signal!(0);\n    \
                                ui! {\n        \
                                    Text { format!(\"Count: {}\", count.get()) }\n        \
                                    Button(\n            \
                                        label = \"+1\",\n            \
                                        on_click = move || count.update(|n| *n += 1),\n        \
                                    )\n    \
                                }\n\
                            }".to_string(),
                )
            }

            Section(
                title = "Read tracking".to_string(),
                body = "Inside a reactive context — a component body, an effect closure, a style \
                        builder — calling `.get()` on a signal registers a dependency. When the \
                        signal changes later, every dependent recomputes. You don't subscribe \
                        manually; the runtime threads the tracking through automatically.".to_string(),
            )

            Card {
                Heading(content = "Effects".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "An `Effect` is a side-effecting subscription. The closure runs \
                               once on creation and re-runs whenever any signal it read \
                               changes. Use effects for logging, networking, persistence — \
                               anywhere you need to react to state outside the UI tree.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "use framework_core::{Effect, signal};\n\
                            \n\
                            let count = signal!(0);\n\
                            let _e = Effect::new(move || {\n    \
                                println!(\"count is now {}\", count.get());\n\
                            });".to_string(),
                )
            }

            Section(
                title = "Scopes".to_string(),
                body = "Every component runs inside a reactive scope. When a component unmounts \
                        (e.g. a navigator pops back, an `if` branch collapses), the scope drops — \
                        and every effect, signal, and reactive binding owned by that scope drops \
                        with it. No leaks, no manual unsubscribe.".to_string(),
            )

            Section(
                title = "Update vs set".to_string(),
                body = "`signal.set(value)` replaces the inner value; `signal.update(|v| ...)` \
                        mutates in place. Both notify subscribers. `untrack(|| ...)` reads a \
                        signal without recording a dependency — useful inside effects that read \
                        one signal but only want to react to another.".to_string(),
            )

            Section(
                title = "Derived values".to_string(),
                body = "When a value is a pure function of other signals, declare it as a \
                        `Derived` instead of recomputing inline. `Derived` caches its result and \
                        invalidates only when its dependencies change; downstream reads stay \
                        fine-grained.".to_string(),
            )
        }
    }
}
