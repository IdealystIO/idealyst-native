//! Reactivity & animation — signals, derived values, effects, async
//! resources, and the `AnimatedValue` motion system. Companion to the
//! bundled `reactivity.md` guide and the live `/demo` page.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::{CONCEPTS_ROUTE, DEMO_ROUTE, SERVER_FUNCTIONS_ROUTE};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let signals_ref: Ref<ViewHandle> = Ref::new();
    let derived_ref: Ref<ViewHandle> = Ref::new();
    let effects_ref: Ref<ViewHandle> = Ref::new();
    let no_vdom_ref: Ref<ViewHandle> = Ref::new();
    let async_ref: Ref<ViewHandle> = Ref::new();
    let animation_ref: Ref<ViewHandle> = Ref::new();
    let pitfalls_ref: Ref<ViewHandle> = Ref::new();
    let next_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: signals_ref, label: "Signals" },
        TocEntry { handle: derived_ref, label: "Derived values" },
        TocEntry { handle: effects_ref, label: "Effects" },
        TocEntry { handle: no_vdom_ref, label: "No virtual DOM" },
        TocEntry { handle: async_ref, label: "Async: resources & reducers" },
        TocEntry { handle: animation_ref, label: "Animation" },
        TocEntry { handle: pitfalls_ref, label: "Two pitfalls" },
        TocEntry { handle: next_ref, label: "Where to go from here" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Reactivity & animation",
                blurb = "The reactive layer is signal-based — close in shape to SolidJS or \
                 Leptos, adapted for native rendering. Reads inside a reactive scope \
                 subscribe to a signal; a write fires exactly the subscribers that read it \
                 and nothing else. There is no virtual DOM, no diffing, and no re-render \
                 pass.",
            )
            PageSection(handle = signals_ref) { signals() }
            PageSection(handle = derived_ref) { derived() }
            PageSection(handle = effects_ref) { effects() }
            PageSection(handle = no_vdom_ref) { no_vdom() }
            PageSection(handle = async_ref) { async_data() }
            PageSection(handle = animation_ref) { animation() }
            PageSection(handle = pitfalls_ref) { pitfalls() }
            PageSection(handle = next_ref) { where_next() }
        }
    };
    layout_with_toc(content, toc)
}

// ============================================================================
// Sections
// ============================================================================

fn signals() -> Element {
    let snippet = "let count = signal!(0_i32);\n\
                   \n\
                   count.set(1);                 // overwrite\n\
                   count.update(|n| *n += 1);    // mutate in place\n\
                   let value = count.get();      // read (subscribes inside a scope)\n\
                   \n\
                   // Inside `ui!`, signals participate automatically:\n\
                   ui! {\n    \
                       text { text_fmt!(\"Count: {}\", bind!(count)) }\n    \
                       button(label = \"+1\", on_click = move || count.update(|n| *n += 1))\n\
                   }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Signals".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "`Signal<T>` is the reactive cell. Read with `.get()`, \
                write with `.set(v)` or `.update(|v| …)`. A read inside a reactive scope — a \
                component body, a style closure, an `effect!`, a `text_fmt!` arg marked with \
                `bind!` — registers the surrounding scope as a dependent. The next write \
                fires every dependent once.".to_string())
            CodePanel(src = snippet)
            Typography(content = "`Signal<T>` is `Copy`, so every closure that needs the \
                value just captures its own handle to the same cell — no `.clone()` dance, \
                no shared-ownership ceremony.".to_string())
        }
    }
}

fn derived() -> Element {
    let snippet = "let count = signal!(0_i32);\n\
                   \n\
                   // A derived value recomputes when its inputs change, and is\n\
                   // itself a reactive source other scopes can subscribe to.\n\
                   let doubled = derived(move || count.get() * 2);\n\
                   \n\
                   // Reads of `doubled` track `count` transitively:\n\
                   ui! { text { text_fmt!(\"Doubled: {}\", bind!(doubled)) } }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Derived values".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "`derived(|| …)` wraps a computed expression as a reactive \
                source. It re-runs only when one of the signals it reads changes, and \
                downstream scopes that read the derived value subscribe transitively. Use it \
                for any value that is a pure function of other signals — formatted strings, \
                filtered lists, an active-variant decision in a style closure.".to_string())
            CodePanel(src = snippet)
            Typography(content = "This is the same primitive the sidebar's active-link \
                highlight on this site rides on: a `derived(..)` closure reads the \
                navigator's active-route signal and flips the link's style variant, with no \
                manual subscription wiring.".to_string())
        }
    }
}

fn effects() -> Element {
    let snippet = "let theme_bg = signal!(Color(\"#ffffff\".into()));\n\
                   \n\
                   // Runs now, and re-runs whenever any signal it reads changes.\n\
                   effect!({\n    \
                       let bg = theme_bg.get();    // subscribes\n    \
                       sync_native_background(bg); // side effect\n\
                   });";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Effects".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "`effect!({ … })` runs its body once immediately, then \
                re-runs it whenever any signal read inside changes. It's the escape hatch for \
                side effects that aren't part of the rendered tree — syncing a value out to a \
                platform API, logging, imperative integration. Most authors rarely reach for \
                it directly: `ui!` already wraps the reactive parts of your tree in effects \
                under the hood.".to_string())
            CodePanel(src = snippet)
            Typography(content = "An effect's subscriptions are anchored to the reactive \
                scope that created it. When that scope is dropped — navigating away from a \
                screen, unmounting a `when(..)` branch — the effect unsubscribes and stops \
                firing. Lifetime is structural, not manual.".to_string())
        }
    }
}

fn no_vdom() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "No virtual DOM".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Most UI frameworks re-run a component on every state \
                change, build a fresh tree, diff it against the previous one, and patch the \
                difference. Idealyst does none of that. A signal write walks straight to the \
                primitives that read it and calls the backend's targeted update path — \
                `update_text`, a style re-apply, a single insert — on exactly those nodes.".to_string())
            Typography(content = "The practical consequence is that update cost scales with \
                what changed, not with the size of the surrounding tree. A counter deep \
                inside a thousand-node screen updates one text node. There is no \
                reconciler to budget for, and a heavy screen doesn't make a small update \
                slower.".to_string())
            Typography(content = "The Performance page has the head-to-head benchmark \
                numbers against React, Vue, and Svelte on identical screens.".to_string(),
                muted = true)
        }
    }
}

fn async_data() -> Element {
    let snippet = "let user_id = signal!(1_u64);\n\
                   \n\
                   // Re-fetches whenever `user_id` changes; prior fetch is cancelled.\n\
                   let user = resource(user_id, |id, cancel| async move {\n    \
                       server::with_cancel(cancel, get_user(id)).await\n\
                   });\n\
                   \n\
                   // Writes that fold their response back into local state:\n\
                   let create = async_reducer(\n    \
                       todos,\n    \
                       |input| async move { create_todo(input).await },\n    \
                       |list, new_todo| list.push(new_todo),\n\
                   );";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Async: resources & reducers".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Async work plugs into the same reactive system. \
                `resource(deps, fetcher)` is a dep-driven read: it runs the fetcher, exposes \
                loading / error / data state as signals, and re-fetches when its deps change \
                — cancelling the prior in-flight call so a stale response can't clobber a \
                fresh one. `mutation()` is fire-and-forget; `async_reducer()` folds a \
                response straight into a local `Signal<S>` — the workhorse for any write that \
                updates a list, map, or record.".to_string())
            CodePanel(src = snippet)
            Typography(content = "These compose directly with server functions: a \
                `#[server]` fn is just an async fn, so it drops into a `resource` or \
                `async_reducer` like any other future. The Server functions page covers the \
                full data-loading and cancellation story.".to_string())
        }
    }
}

fn animation() -> Element {
    let snippet = "let opacity = animated!(0.0_f32);\n\
                   \n\
                   // Tween to a value over a duration with an easing curve:\n\
                   opacity.animate(TweenTo::new(1.0, Duration::from_millis(400)).ease_out());\n\
                   \n\
                   // Physical spring motion:\n\
                   let x = animated!(0.0_f32);\n\
                   x.animate(SpringTo::new(120.0));\n\
                   \n\
                   // Bind the animated value into a style or transform —\n\
                   // it drives a per-frame native update, no React-style re-render.";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Animation".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "`AnimatedValue` is the per-frame motion handle, declared \
                with `animated!(...)`. Unlike a `transitions` block — which animates a \
                property when a style state flips — an `AnimatedValue` is imperative motion \
                you drive from app logic: tween to a target, fling on a spring, or \
                choreograph a `timeline!` of steps. Bind one into a style or transform and \
                the backend ticks it per frame on its native loop (Core Animation, the \
                Android choreographer, requestAnimationFrame, the wgpu host tick).".to_string())
            CodePanel(src = snippet)
            Typography(content = "Bindings are scope-anchored exactly like signal \
                subscriptions: when the owning component's scope is dropped, the animation \
                and its subscription tear down with it. The `/demo` page runs fade, \
                spring-vs-tween, multi-property entrance, and color-tween examples live on \
                whatever backend is rendering it right now.".to_string())
        }
    }
}

fn pitfalls() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Two pitfalls".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "1. Reading a signal with `.get()` OUTSIDE a reactive \
                scope gives you the current value once, with no subscription — fine when \
                that's what you want, surprising when you expected reactivity. Inside a \
                scope (a `ui!` closure, an `effect!`, a `text_fmt!` arg), `.get()` registers \
                a dependency.".to_string())
            Typography(content = "2. A bare `.set()` re-runs every dependent immediately. \
                When you're updating several signals at once and don't want N intermediate \
                re-runs, wrap the writes in `batch(|| …)` so dependents fire once after the \
                whole group settles — the same trick the theme-token swap uses to retint a \
                large tree in a single pass.".to_string())
        }
    }
}

fn where_next() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Where to go from here".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "See the reactive model running on real backends, then \
                read how the signal layer fits the rest of the architecture.".to_string())
            link(route = &DEMO_ROUTE, params = ()) {
                Typography(content = "Live demo \u{2192}".to_string())
            }
            link(route = &CONCEPTS_ROUTE, params = ()) {
                Typography(content = "Core concepts \u{2192}".to_string())
            }
            link(route = &SERVER_FUNCTIONS_ROUTE, params = ()) {
                Typography(content = "Server functions \u{2192}".to_string())
            }
        }
    }
}
