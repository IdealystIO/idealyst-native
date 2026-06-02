//! Foundations — the unifying mental model that sits above the
//! per-primitive tracks. Two steps: how signals, the UI, and the theme
//! are all one reactive engine; and an under-the-hood look at batching
//! that explains why that engine stays cheap at scale.
//!
//! Like the rest of the tutorial, the lessons here lean on `runtime_core`
//! concepts directly. The chrome (`Typography`, `Callout`, ...) is idea-ui;
//! the substance is the core.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Typography};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::routes::{CORE_ENGINE_ROUTE, CORE_PERF_ROUTE};
use crate::shell;

pub fn engine() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = CORE_ENGINE_ROUTE.name(),
            title = "One reactive engine".to_string(),
            lead = "Signals and effects power reactive UI \u{2014} and in Idealyst, the style \
                system too.".to_string(),
        ) {
            Typography(
                content = "Signals and effects are the reactive core, the same primitives many \
                    frameworks share. Idealyst takes this a little further, bridging the style \
                    system with reactivity for efficient, consistent style management with no \
                    third-party libraries. This page maps how the pieces connect; the tracks that \
                    follow fill in each one.".to_string()
            )

            Typography(content = "Signals drive reactivity".to_string(), kind = typography_kind::H2)
            Typography(
                content = "A signal read inside a running effect records a two-way link: the \
                    effect joins the signal's subscribers, and the signal joins the effect's \
                    dependencies. A write re-runs exactly the effects that read it. Dependencies \
                    are tracked automatically as each effect runs, so a branch that stops reading \
                    a signal stops being notified by it.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{signal, Effect};

let count = signal!(0);

let _e = Effect::new(move || {
    // reading count here subscribes this effect to it:
    log::info!("count = {}", count.get());
});

count.set(1); // re-runs the effect — and nothing else"##.to_string())

            Typography(content = "Reactivity affects the UI".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Your UI nodes are effects. When you bind a signal into a node, the \
                    framework wraps that node in an effect whose body calls into the backend. A \
                    write re-runs that one effect, which repaints that one node \u{2014} the unit \
                    of update is the closure that read the signal. A bound node is an effect that \
                    owns a native view, so updates stay surgical: there's no virtual DOM, and the \
                    tree is never diffed or re-rendered wholesale.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{signal, ui, rx};

let count = signal!(0);

// This text node IS an effect. Reading `count` inside `rx!` subscribes
// it; a write repaints just this node — no diff, no tree walk.
ui! { text { rx!(format!("Count: {}", count.get())) } }

count.update(|n| *n += 1); // re-runs only this text node"##.to_string())

            Typography(
                content = "Stylesheets are composed of tokens, tokens are reactive".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Here's where Idealyst extends the core: the style system runs on the \
                    same machinery. A design token is itself a signal \u{2014} one per name, \
                    living in the token registry. Stylesheets are composed from these tokens: a \
                    rule stores a token reference, and resolving it is a signal read, so a styled \
                    node subscribes to exactly the tokens it uses. Switching the theme rewrites \
                    those signals, and the same fan-out re-applies the styles of only the nodes \
                    that read a changed token \u{2014} no separate theming library, no manual \
                    wiring.".to_string()
            )
            CodePanel(src = r##"// A style references a token by NAME, not a concrete value:
stylesheet! {
    Card<ThemeRef> {
        base(t) { background: Tokenized::token("color-surface", Color("#ffffff")) }
    }
}

// Switching the theme writes the token signals…
set_theme(dark_theme());
// …and every node that resolved `color-surface` re-applies its style.
// Nodes that never read it stay asleep."##.to_string())

            Callout(label = "Learn the core once".to_string()) {
                Typography(
                    content = "Dynamic text, keyed lists, styles, and theme switching all run on \
                        the same signal-and-effect core. Learn it once and each of those becomes \
                        the same idea applied somewhere new.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The full model \u{2014} the arena, scopes, the subscription graph, and \
                    the token registry.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}

pub fn performance() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = CORE_PERF_ROUTE.name(),
            title = "Under the hood: batching".to_string(),
            lead = "Why a theme switch that touches fifty tokens still feels \
                instant.".to_string(),
        ) {
            Typography(
                content = "A naive observer model would thrash on exactly the workload theming \
                    creates. A theme swap writes around fifty token signals at once. A typical \
                    styled node reads two to five of them. Fan out every write the moment it \
                    happens and each node's style effect re-runs once per token it read \u{2014} \
                    two to five full re-applies per node, each redoing real work: re-minting a CSS \
                    class on web, msg_send-ing every property and scheduling animators on native. \
                    On a docs-sized tree \u{2014} ~490 views, hundreds of effects \u{2014} that's \
                    the difference between a snappy toggle and one that hangs the main thread for \
                    hundreds of milliseconds.".to_string()
            )

            Typography(
                content = "Batching turns N\u{00d7}M into N".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "batch coalesces the fan-out. Inside the batch, writes still land \
                    immediately \u{2014} a read after a set sees the new value \u{2014} but \
                    subscriber notifications are queued instead of run. At the end of the \
                    outermost batch the queue is de-duplicated, preserving first-seen order, and \
                    each effect runs exactly once. A node that read five changed tokens re-applies \
                    its style a single time, not five.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{signal, batch};

let first = signal!(0);
let second = signal!(0);

batch(|| {
    first.set(1);
    assert_eq!(first.get(), 1); // the WRITE is visible immediately…
    second.set(2);
}); // …but an effect reading both re-runs ONCE here, not twice"##.to_string())

            Typography(
                content = "You rarely call it yourself".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "The framework already wraps the expensive bulk paths. update_tokens \u{2014} \
                    every theme switch \u{2014} batches its per-token writes internally; that's the \
                    line between a theme toggle that scales and one that doesn't. You reach for \
                    batch directly on your own bulk writes: resetting a list, hydrating a form, \
                    or updating several related fields where one settled result beats a flurry of \
                    intermediate ones.".to_string()
            )

            Callout(label = "Immediate reads, deferred effects".to_string()) {
                Typography(
                    content = "Batching delays only the subscriber fan-out; your values update \
                        immediately. Inside a batch, set-then-get sees the new value, while the \
                        effects wait until the batch closes.".to_string(),
                    muted = true,
                )
            }
            Callout(label = "Memos compound the win".to_string()) {
                Typography(
                    content = "An equality-gated memo stops a cascade early: if a derived value \
                        recomputes to the same result, its subscribers aren't notified at all. \
                        Batching collapses duplicate runs; memos prevent the runs that wouldn't \
                        change anything from firing in the first place.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The batching internals, the dedup-and-flush order, and the reactive \
                    seams across the framework.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}
