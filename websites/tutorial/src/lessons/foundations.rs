//! Foundations — the unifying mental model that sits above the
//! per-primitive tracks. Two steps: how signals, the UI, and the theme are
//! all one reactive engine; and an under-the-hood look at the flush, which
//! is the boundary every one of those pieces commits at.
//!
//! Like the rest of the tutorial, the lessons here teach the framework's
//! own surface. The chrome (`Typography`, `Callout`, ...) is idea-ui; the
//! substance is the core.

use idea_ui::{typography_kind, Typography};
use runtime_core::{ui, Element};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::demo::FlushDemo;
use crate::routes::{CORE_ENGINE_ROUTE, CORE_FLUSH_ROUTE};
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
                    system with reactivity so runtime restyling and theming need no third-party \
                    library. This page maps how the pieces connect; the tracks that follow fill \
                    in each one.".to_string()
            )

            Typography(content = "Signals drive reactivity".to_string(), kind = typography_kind::H2)
            Typography(
                content = "A signal read inside a running effect records a two-way link: the \
                    effect joins the signal's subscribers, and the signal joins the effect's \
                    dependencies. A write stages a value; the next flush commits it and re-runs \
                    exactly the effects that read it. Dependencies are recollected on every run, \
                    so a branch that stops reading a signal stops being notified by it.".to_string()
            )

            Typography(content = "Reactivity drives the UI".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Your UI nodes are effects. When you bind a signal into a node, the \
                    framework wraps that node in an effect whose body calls into the backend. A \
                    committed write re-runs that one effect, which repaints that one node \u{2014} \
                    the unit of update is the closure that read the signal. A bound node is an \
                    effect that owns a native view, so updates stay surgical: there's no virtual \
                    DOM, and the tree is never diffed or re-rendered wholesale.".to_string()
            )
            CodePanel(src = include_str!("../samples/fnd_engine.rs").to_string())

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
                    that read a changed token.".to_string()
            )
            CodePanel(src = include_str!("../samples/fnd_tokens.rs").to_string())

            Typography(content = "One boundary for all of it".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Because dynamic text, keyed lists, styles, and theme switching are all \
                    signals and effects, they also share one commit point. A turn of your code \
                    stages whatever it stages \u{2014} state, tokens, list contents \u{2014} and \
                    the flush commits the lot as a single logical update. The next step walks that \
                    boundary in detail.".to_string()
            )

            Callout(label = "Learn the core once".to_string()) {
                Typography(
                    content = "Dynamic text, keyed lists, styles, and theme switching all run on \
                        the same signal-and-effect core. Learn it once and each of those becomes \
                        the same idea applied somewhere new.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The full model \u{2014} per-world arenas, scopes, the subscription \
                    graph, and the token registry.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}

pub fn flush_boundary() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = CORE_FLUSH_ROUTE.name(),
            title = "Under the hood: the flush".to_string(),
            lead = "Why a theme switch that touches fifty tokens costs one update.".to_string(),
        ) {
            Typography(
                content = "Theming is the workload that shows why the commit point exists. A \
                    theme swap writes around fifty token signals at once, and a typical styled \
                    node reads two to five of them. If each write fanned out the moment it \
                    happened, every node's style effect would re-run once per token it read \
                    \u{2014} several full re-applies per node, each re-resolving every property \
                    and pushing it onto the live view (on native, a platform message per property \
                    plus animator scheduling).".to_string()
            )

            Typography(
                content = "Staging collapses the fan-out".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "A write stages a pending value and returns. Nothing observable moves \
                    until the driver flushes, and the flush drains all fifty staged tokens in one \
                    pass: it commits them, settles the derived values, dedupes the woken effects, \
                    and runs each one once. A node that read five changed tokens re-applies its \
                    style a single time. The old core exposed a batch(f) wrapper to buy this \
                    coalescing per call site; here it is the shape of every turn, so there is no \
                    wrapper to remember.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_flush.rs").to_string())

            Typography(content = "Watch it happen".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The reader below is a reactive text node \u{2014} an effect \u{2014} so \
                    its run count is an effect-run count. \"write both\" writes two of its \
                    dependencies in one handler and the count moves by one.".to_string()
            )
            FlushDemo()

            Typography(
                content = "On the web, a theme change is a variable write".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Coalesced re-applies onto live views are the native story. The web \
                    takes a shorter path. A token compiles to a CSS custom property, so a \
                    stylesheet rule that reads it emits var(--token, fallback) into the element's \
                    class. That class carries the reference, so it stays valid across every theme \
                    and never has to be recomputed.".to_string()
            )
            CodePanel(src = r##"/* a token reference compiles to a CSS variable, baked in once: */
.card-a1b2 { background: var(--color-surface, #ffffff); }

/* a theme switch rewrites only the :root variables — N writes, total: */
:root { --color-surface: #14161c; --color-text: #e8eaf0; }

/* every .card-* recolors through the cascade. No element is touched. */"##.to_string())
            Typography(
                content = "Switching the theme updates the :root variables with one setProperty \
                    per changed token, and the browser's cascade repaints every element that reads \
                    them. No class is recomputed, no element's style attribute is rewritten, no \
                    node is added or removed. The only DOM mutation is that handful of variable \
                    writes on a single rule, however many elements depend on them.".to_string()
            )
            Callout(label = "Cost scales with tokens".to_string()) {
                Typography(
                    content = "A web theme swap costs the number of changed tokens and stays flat \
                        as the tree grows. Ten elements or ten thousand, the work is the same few \
                        variable writes; the browser's cascade does the rest.".to_string(),
                    muted = true,
                )
            }

            Typography(
                content = "What the boundary asks of you".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Two habits. Read-modify-write goes through update, whose closure sees \
                    the staged value, so increments in one turn compose. And a handler computes \
                    against the snapshot it started with, so anything that must react to a value \
                    the handler just wrote belongs in an effect or a memo, which the flush runs \
                    after the commit.".to_string()
            )

            Callout(label = "Memos compound the win".to_string()) {
                Typography(
                    content = "An equality-guarded memo stops a cascade early: if a derived value \
                        recomputes to the same result, its subscribers aren't notified at all. The \
                        flush collapses duplicate runs; memos prevent the runs that wouldn't \
                        change anything from being queued in the first place.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The staged-commit contract, the flush algorithm, and where each \
                    backend's flush driver lives.".to_string(),
                link_label = "Runtime v2 migration guide".to_string(),
                doc_file = "migrating-to-runtime-v2.md".to_string(),
            )
        }
    })
}
