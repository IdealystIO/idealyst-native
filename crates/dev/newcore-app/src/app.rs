//! The authored app — CORE-AGNOSTIC source. Everything here is plain
//! `ui!` + `#[component]` authoring surface: snake_case primitives,
//! struct-literal components with named/defaulted props, children
//! blocks, f-string text, reactive closures, a reactive `if`, a keyed
//! `for` with row-local state, and handler-driven mutations — plus
//! `test_id = ...` identity anchors (the P5 seam: registered in the
//! vocabulary robot registry on the new core, `Bound::test_id` on the
//! old). The crate feature decides which core it lowers to (see
//! lib.rs); nothing in this file changes between the two.

use crate::prelude::*;
use runtime_macros::{component, stylesheet, ui};

// The app's stylesheet — P3c: theme tokens (the `color-surface`
// background tracks the installed theme), a `size` variant, and a
// `state hovered` interaction overlay. Core-agnostic like everything
// else in this file: the macro emits the old `IntoStyleSource` impl or
// the new `IntoStyleProp` impl depending on the build's core.
stylesheet! {
    pub Card<()> {
        base(_t) {
            padding: 8,
            background: Tokenized::token("color-surface", Color("#101010".into())),
        }
        variant size {
            #[default]
            medium(_t) {}
            large(_t) { padding: 16 }
        }
        state hovered(_t) {
            border_width: 4.0,
        }
    }
}

/// A themed container: static sheet application with a variant picked
/// from a static prop. The `state hovered` overlay makes this the
/// static-divert case on event-driven backends (state machine attached
/// even though the application itself is constant).
#[component]
fn StyledCard(
    /// Picks the `size` variant (static prop with a default).
    #[prop(static, default = false)]
    large: bool,
    children: Vec<Element>,
) -> Element {
    ui! {
        view(style = Card().size(if large { CardSize::Large } else { CardSize::Medium })) {
            children
        }
    }
}

/// One todo item. Rows are keyed by `id`.
#[derive(Clone, PartialEq)]
pub struct Todo {
    pub id: u32,
    pub label: String,
    pub done: bool,
}

/// The app's root signals, exposed so tests can drive mutations
/// directly as well as through recorded button handlers.
pub struct AppHandle {
    pub todos: Signal<Vec<Todo>>,
    pub draft: Signal<String>,
    pub next_id: Signal<u32>,
}

/// Build the app tree plus its handle. Must run with the owning
/// world/scope ambient (tests wrap this in `World::enter`).
pub fn build_app() -> (Element, AppHandle) {
    let todos = signal(vec![
        Todo { id: 1, label: "write tests".to_string(), done: false },
        Todo { id: 2, label: "ship it".to_string(), done: true },
    ]);
    let draft = signal(String::new());
    let next_id = signal(3u32);
    let root = ui! {
        TodoApp(todos = todos, draft = draft, next_id = next_id)
    };
    (root, AppHandle { todos, draft, next_id })
}

/// A titled container: reactive `title`, a static (unwrapped) flag with
/// a default, and a children block.
#[component]
fn Section(
    /// Section heading (reactive-by-default prop).
    title: String,
    /// Draws the `*` marker. Static prop with a default — call sites
    /// may omit it.
    #[prop(static, default = false)]
    highlighted: bool,
    children: Vec<Element>,
) -> Element {
    ui! {
        view {
            view {
                text { move || title.get() }
                if highlighted {
                    text { "*" }
                }
            }
            children
        }
    }
}

/// One todo row. Row-local `taps` state proves keyed reuse: it must
/// survive list edits elsewhere. `done` is derived from the list signal
/// (a kept keyed row is NOT re-rendered, so row content that can change
/// must read through a signal, not a snapshot prop).
#[component]
fn TodoRow(
    /// Row identity (static — it IS the key).
    #[prop(static)]
    id: u32,
    /// Display label (labels never change; arrives as a snapshot).
    label: String,
    /// The list signal, for derived row state.
    todos: Signal<Vec<Todo>>,
    /// Optional callbacks — bound only when present (repo rule 9.6).
    on_toggle: Option<Rc<dyn Fn()>>,
    on_remove: Option<Rc<dyn Fn()>>,
) -> Element {
    // Row-local state.
    let taps = signal(0u32);
    // Derived row state: one memo per row, cutting propagation when an
    // unrelated row toggles.
    let done = memo(move || todos.get().iter().any(|t| t.id == id && t.done));
    ui! {
        view {
            text { move || format!("{} {}", if done.get() { "[x]" } else { "[ ]" }, label.get()) }
            button(
                label = move || format!("bump{}:{}", id, taps.get()),
                on_click = move || taps.set(taps.get() + 1)
            )
            if let Some(cb) = on_toggle {
                button(label = move || format!("toggle-{}", id), on_click = move || (cb)())
            }
            if let Some(cb) = on_remove {
                // Shared across rows on purpose: sibling list rows
                // sharing one affordance test_id is the find_all /
                // toHaveCount pattern (duplicate policy: find is
                // last-wins, find_all returns every row's button).
                button(label = move || format!("remove-{}", id), test_id = "row-del", on_click = move || (cb)())
            }
        }
    }
}

// ===========================================================================
// P3-set primitive demo (overlay / anchored_overlay / presence /
// flat_list) — a second root, mounted by its own e2e tests so the
// TodoApp op-log assertions stay undisturbed. Core-agnostic like
// everything else in this file: the tags lower to the old primitives or
// to the vocabulary glue depending on the build's core.
// ===========================================================================

/// The demo's root signals, exposed so tests can drive mutations
/// directly as well as through recorded button handlers.
pub struct DemoHandle {
    pub modal_open: Signal<bool>,
    pub toast_on: Signal<bool>,
    pub rows: Signal<Vec<Todo>>,
}

/// Build the P3-set demo tree plus its handle. Must run with the owning
/// world/scope ambient (tests wrap this in `World::enter`).
pub fn build_demo() -> (Element, DemoHandle) {
    let modal_open = signal(false);
    let toast_on = signal(false);
    let rows = signal(vec![
        Todo { id: 1, label: "alpha".to_string(), done: false },
        Todo { id: 2, label: "beta".to_string(), done: false },
        Todo { id: 3, label: "gamma".to_string(), done: true },
    ]);
    let root = ui! {
        PrimsDemo(modal_open = modal_open, toast_on = toast_on, rows = rows)
    };
    (root, DemoHandle { modal_open, toast_on, rows })
}

/// Exercises the P3-set tags: a reactive-`if`-gated `overlay` +
/// `anchored_overlay` pair (modal open/close via authored handlers and
/// backdrop dismiss), a `presence` with enter/exit fades, and a
/// `flat_list` over a reactive row signal.
///
/// The anchor target is a DETACHED ref (`Ref::default()` — fills
/// nothing, resolves `None`): real anchor filling is the P5 identity
/// port; an unresolved anchor exercises the primitive's structure
/// without it, identically on both cores.
#[component]
fn PrimsDemo(
    modal_open: Signal<bool>,
    toast_on: Signal<bool>,
    rows: Signal<Vec<Todo>>,
) -> Element {
    let anchor = AnchorTarget::from(Ref::<ViewHandle>::default());
    ui! {
        view {
            button(label = "open-modal", on_click = move || modal_open.set(true))
            button(label = "toggle-toast", on_click = move || toast_on.set(!toast_on.get()))
            if modal_open.get() {
                overlay(
                    placement = ViewportPlacement::Center,
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = move || modal_open.set(false),
                    trap_focus = true
                ) {
                    text { "modal body" }
                    button(label = "close-modal", on_click = move || modal_open.set(false))
                }
                anchored_overlay(target = anchor.clone(), side = ElementSide::Below) {
                    text { "anchored tip" }
                }
            }
            presence(
                present = move || toast_on.get(),
                enter = PresenceAnim::fade(100, Easing::EaseOut),
                exit = PresenceAnim::fade(100, Easing::EaseIn)
            ) {
                text { "toast body" }
            }
            flat_list(
                data = rows,
                key = |_i, t: &Todo| t.id as u64,
                size = fixed_size(24.0),
                render = |_i, t: &Todo| ui! { text { t.label.clone() } }.into()
            )
        }
    }
}

/// A `#[method]`-bearing component (P5 robot remainder, same-source):
/// the macro lifts the nested fns into a `MethodTallyHandle`, fills the
/// auto-injected `bind_to` prop, and registers the methods with the
/// robot component registry — invokable by name + JSON args through
/// `list_components` / `invoke_method` on EITHER core. Bodies use
/// `set(get() + n)` (not `update`) because the two cores' `update`
/// closure shapes differ — the one same-source-visible seam.
#[component]
fn MethodTally(
    /// Starting value (static — `reset()` returns to it).
    #[prop(static, default = 0)]
    initial: i32,
) -> Element {
    let value = signal(initial);
    /// Add `n` to the tally.
    #[method]
    fn bump_by(n: i32) {
        value.set(value.get() + n);
    }
    /// Return the tally to its mounted `initial`.
    #[method]
    fn reset() {
        value.set(initial);
    }
    ui! {
        view(test_id = "method-tally") {
            text(test_id = "method-tally-val") { "tally: {value}" }
        }
    }
}

/// The root component: a memo, an uncontrolled-input-free controlled
/// `text_input`, an add handler, a reactive empty-state `if`, and the
/// keyed list.
#[component]
fn TodoApp(
    todos: Signal<Vec<Todo>>,
    draft: Signal<String>,
    next_id: Signal<u32>,
) -> Element {
    let remaining = memo(move || todos.get().iter().filter(|t| !t.done).count());
    // Robot watch (P5 remainder): exposes the memo's live value to the
    // bridge's `read_signal`/`list_watched_signals` verbs (robot-test's
    // `assert_signal`). New-core prelude routes to the vocabulary
    // registry; the old-core check leg shims it to a no-op (lib.rs).
    watch_signal("remaining", remaining);
    ui! {
        Section(title = "Todos", highlighted = true) {
            text(test_id = "remaining") { "{remaining} left" }
            text_input(
                value = draft,
                on_change = move |s: String| draft.set(s),
                placeholder = "What next?",
                test_id = "draft-input"
            )
            button(label = "add", test_id = "add-btn", on_click = move || {
                let name = draft.get();
                if name.is_empty() {
                    return;
                }
                let id = next_id.get();
                next_id.set(id + 1);
                let mut list = todos.get();
                list.push(Todo { id, label: name, done: false });
                todos.set(list);
                draft.set(String::new());
            })
            if todos.get().is_empty() {
                text { "nothing to do" }
            } else {
                view {
                    for todo in todos, key = todo.id {
                        TodoRow(
                            id = todo.id,
                            label = todo.label.clone(),
                            todos = todos,
                            on_toggle = Some(Rc::new({
                                let id = todo.id;
                                move || {
                                    let mut list = todos.get();
                                    if let Some(t) = list.iter_mut().find(|t| t.id == id) {
                                        t.done = !t.done;
                                    }
                                    todos.set(list);
                                }
                            }) as Rc<dyn Fn()>),
                            on_remove = Some(Rc::new({
                                let id = todo.id;
                                move || {
                                    let mut list = todos.get();
                                    list.retain(|t| t.id != id);
                                    todos.set(list);
                                }
                            }) as Rc<dyn Fn()>),
                        )
                    }
                }
            }
            Section(title = "About") {
                text { "static footer" }
            }
            StyledCard(large = true) {
                text { "themed card" }
            }
            MethodTally(initial = 5)
        }
    }
}
