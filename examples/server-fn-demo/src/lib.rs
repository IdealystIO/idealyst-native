//! Full-stack server-functions demo.
//!
//! Three responsibilities in one crate:
//!
//! - **Wire types** (`Todo`, `CreateTodo`) — visible to both
//!   server and client.
//! - **`#[server]` functions** (`list_todos`, `create_todo`,
//!   `toggle_todo`, `delete_todo`, `whoami`, `slow_op`) — bodies
//!   compile only with `--features server`; client builds get
//!   RPC stubs.
//! - **`#[server::sse]` endpoint** (`ticks`) — a live Server-Sent
//!   Events stream consumed by `use_sse` in `app()`; on iOS/Android
//!   it exercises the device's native `net::EventSource` arm.
//! - **`app()`** — the idealyst UI the client renders in the
//!   browser, talking to those server fns over `_srv/_batch`.
//!
//! Building blocks:
//!   - `cargo run -p server-fn-demo --bin server --features server`
//!     hosts the API + serves the wasm bundle from `./pkg/`.
//!   - `idealyst build --web examples/server-fn-demo` produces
//!     the wasm bundle (and copies it into `./pkg/`).
//!
//! Open `http://127.0.0.1:3000` to see the UI; every interaction
//! is a server function call.

use idea_ui::{
    install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography,
};
use std::rc::Rc;

use runtime_core::{
    async_reducer, component, fixed_size, flat_list, signal, ui, AsyncReducer, AsyncStatus,
    Effect, Element, FlexDirection, IntoElement, Length, Signal, StyleRules, StyleSheet,
};
use serde::{Deserialize, Serialize};
use server::{server, ServerError};

// ============================================================================
// Wire types — shared between server and client.
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Todo {
    pub id: u64,
    pub title: String,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTodo {
    pub title: String,
}

/// One Server-Sent Event from the `ticks` stream. The server emits one
/// every `TICK_INTERVAL_MS`; the client renders the latest via `use_sse`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tick {
    pub seq: u64,
}

/// Cadence of the live SSE demo stream (server emits, client observes).
pub const TICK_INTERVAL_MS: u64 = 500;

// ============================================================================
// Server-only state.
//
// Gated behind `feature = "server"` so the wasm build doesn't
// compile it. In a real app this is where the Diesel pool / Redis
// client / etc. live; here it's just a `Mutex<Vec<Todo>>`.
// ============================================================================

#[cfg(feature = "server")]
pub mod state {
    use super::Todo;
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;

    pub struct AppState {
        pub todos: Mutex<Vec<Todo>>,
        pub next_id: AtomicU64,
    }

    impl AppState {
        pub fn new() -> Self {
            Self {
                todos: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
            }
        }
    }

    impl Default for AppState {
        fn default() -> Self {
            Self::new()
        }
    }
}

// ============================================================================
// Server functions.
// ============================================================================

#[cfg(feature = "server")]
use std::sync::atomic::Ordering;
#[cfg(feature = "server")]
use std::sync::Arc;

#[cfg(feature = "server")]
use crate::state::AppState;

#[server]
pub async fn list_todos() -> Result<Vec<Todo>, ServerError> {
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    let todos = state.todos.lock().unwrap().clone();
    Ok(todos)
}

#[server]
pub async fn create_todo(input: CreateTodo) -> Result<Todo, ServerError> {
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let todo = Todo {
        id,
        title: input.title,
        done: false,
    };
    state.todos.lock().unwrap().push(todo.clone());
    Ok(todo)
}

#[server]
pub async fn toggle_todo(id: u64) -> Result<Todo, ServerError> {
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    let mut todos = state.todos.lock().unwrap();
    let todo = todos
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| ServerError::failed(format!("no todo with id {id}")))?;
    todo.done = !todo.done;
    Ok(todo.clone())
}

/// Deletes the todo with `id` and **echoes the id back on success**.
///
/// The echo lets the client's `async_reducer` apply closure remove
/// the matching entry from local state without remembering the
/// input. Convention for "remove-by-X" server fns: return the
/// X you removed.
#[server]
pub async fn delete_todo(id: u64) -> Result<u64, ServerError> {
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    let mut todos = state.todos.lock().unwrap();
    let before = todos.len();
    todos.retain(|t| t.id != id);
    if todos.len() == before {
        return Err(ServerError::failed(format!("no todo with id {id}")));
    }
    Ok(id)
}

#[server]
pub async fn whoami() -> Result<String, ServerError> {
    let auth = server::use_request_header("authorization")
        .ok_or_else(|| ServerError::failed("missing Authorization header"))?;
    Ok(format!("authenticated as: {auth}"))
}

#[server]
pub async fn slow_op(ms: u64) -> Result<String, ServerError> {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    Ok(format!("slept {ms}ms"))
}

// ============================================================================
// Live Server-Sent Events endpoint.
//
// `#[server::sse]` mounts `GET /_srv/_sse/ticks` on the server build (it
// serializes each yielded `Tick` as a `data:` event) and emits a
// `fn ticks() -> String` URL stub on the client build. The stream ticks an
// incrementing counter forever at `TICK_INTERVAL_MS`; axum drops it when the
// client disconnects (which `use_sse` does on scope teardown).
//
// On the client this is consumed by `use_sse::<Tick>(ticks())` in `app()`,
// which on iOS/Android drives the native `net::EventSource` streaming arm —
// the whole point of this demo.
// ============================================================================

#[server::sse]
pub async fn ticks() -> impl futures_util::Stream<Item = Tick> {
    futures_util::stream::unfold(0u64, |seq| async move {
        tokio::time::sleep(std::time::Duration::from_millis(TICK_INTERVAL_MS)).await;
        Some((Tick { seq }, seq + 1))
    })
}

// ============================================================================
// Point the `server` SDK at the API host. On wasm we use the
// browser's `window.location.origin`, which lines up with the
// server bin serving the bundle and the API from the same port.
// On terminal/native targets the demo doesn't have a meaningful
// default — caller would replace via `server::configure(...)`.
// ============================================================================

fn configure_server() {
    // The server bin compiles the lib too (for inventory registration etc.)
    // but never calls `app()`, and `server::configure` doesn't exist on the
    // server-feature build anyway (it's gated `#[cfg(not(feature = "server"))]`
    // in the SDK) — so every arm here is also implicitly client-only.

    // Web: same-origin as the page (the server bin serves the bundle + API
    // from one port).
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        server::configure(server::ClientConfig::new(origin));
    }

    // Native client (iOS / Android / desktop dev): point at the host machine
    // running `cargo run -p server-fn-demo --bin server --features server`.
    // The iOS simulator and desktop share the host's loopback; the Android
    // emulator reaches the host loopback via the special `10.0.2.2` alias.
    // For a *physical* device, replace this with the host's LAN IP
    // (e.g. `http://192.168.x.y:3000`).
    #[cfg(all(not(feature = "server"), not(target_arch = "wasm32")))]
    {
        let base = if cfg!(target_os = "android") {
            "http://10.0.2.2:3000"
        } else {
            "http://127.0.0.1:3000"
        };
        server::configure(server::ClientConfig::new(base));
    }
}

// ============================================================================
// SDK-handler registration hook the CLI-generated wrappers invoke before
// mount. No third-party SDKs in this demo, so it's an empty generic over
// `Backend` — backend-agnostic, no per-target `#[cfg]` and no `backend-*`
// dep. The wrappers pass the concrete backend per platform.
// ============================================================================

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// ============================================================================
// Recorder-side registration for the runtime-server sidecar. Gated by
// `sidecar` (set only by the generated sidecar wrapper) so device/web
// builds never pull `dev-server`.
// ============================================================================

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {
    // No SDK navigator/external needs recorder-side registration in this app.
}

// ============================================================================
// `app()` — the idealyst UI.
//
// The whole thing fits in one function for the demo. State is two
// signals: the live todo list, and a status message that surfaces
// loading / error states. Three actions: refresh (re-fetch from
// server), add (canned titles, since the demo doesn't include a
// TextInput primitive yet), toggle (per-row button).
//
// Server-fn calls happen inside `spawn_async`-backed event
// handlers. `list_todos` / `create_todo` etc. are the macro-
// generated stubs from above — same names the server bin has
// inventory entries for.
//
// On the wasm build, the bodies of the `#[server]` fns are gone
// (replaced with stubs that POST to `/_srv/<fn>`). So this entire
// file compiles cleanly for the browser without dragging in any
// of the server-side state machinery.
// ============================================================================

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());
    configure_server();

    // Single source of truth: the live todo list.
    let todos: Signal<Vec<Todo>> = signal!(Vec::new());

    // Four async actions, each folding its response back into
    // `todos`. The reducer shape is the canonical "mutation
    // applies to local state" pattern — see runtime_core::async_reducer.
    //
    //   refresh : ()        → replace whole list
    //   create  : CreateTodo→ push
    //   toggle  : u64       → replace by id
    //   delete  : u64       → remove by id (server echoes the id)

    let refresh: AsyncReducer<(), ServerError> = async_reducer(
        todos,
        |_| async { list_todos().await },
        |list: &mut Vec<Todo>, new_list: Vec<Todo>| *list = new_list,
    );

    let create: AsyncReducer<CreateTodo, ServerError> = async_reducer(
        todos,
        |input| async { create_todo(input).await },
        |list: &mut Vec<Todo>, new_todo: Todo| list.push(new_todo),
    );

    let toggle: AsyncReducer<u64, ServerError> = async_reducer(
        todos,
        |id| async move { toggle_todo(id).await },
        |list: &mut Vec<Todo>, updated: Todo| {
            if let Some(slot) = list.iter_mut().find(|t| t.id == updated.id) {
                *slot = updated;
            }
        },
    );

    let delete: AsyncReducer<u64, ServerError> = async_reducer(
        todos,
        |id| async move { delete_todo(id).await },
        |list: &mut Vec<Todo>, deleted_id: u64| {
            list.retain(|t| t.id != deleted_id);
        },
    );

    // Fire the initial list fetch on mount.
    {
        let refresh = refresh.clone();
        let e = Effect::new(move || refresh.trigger(()));
        std::mem::forget(e);
    }

    // Reactive status line — projects refresh's lifecycle into text.
    let refresh_for_status = refresh.clone();
    let status_line = runtime_core::text(move || match refresh_for_status.status_now() {
        AsyncStatus::Idle => "ready".to_string(),
        AsyncStatus::Loading => "loading...".to_string(),
        AsyncStatus::Error(e) => format!("error: {e}"),
    })
    .into_element();

    let on_refresh = {
        let refresh = refresh.clone();
        move || refresh.trigger(())
    };

    let on_add_milk = make_adder(create.clone(), "Buy milk");
    let on_add_dog = make_adder(create.clone(), "Walk the dog");
    let on_add_demo = make_adder(create.clone(), "Demo idealyst");

    // Live Server-Sent Events line. `use_sse` opens `GET /_srv/_sse/ticks`
    // and re-renders this text on every event — on iOS/Android that runs the
    // device's native `net::EventSource` streaming arm. Client-only: the SDK
    // gates `use_sse` behind `#[cfg(not(feature = "server"))]`, and `app()`
    // is compiled (never called) on the server build, so it must still build
    // there — hence the placeholder arm.
    #[cfg(not(feature = "server"))]
    let sse_line: Element = {
        let live = server::use_sse::<Tick>(ticks());
        runtime_core::text(move || match live.latest() {
            Some(t) => format!("live SSE — tick #{} (status {:?})", t.seq, live.status()),
            None => format!("live SSE — connecting… (status {:?})", live.status()),
        })
        .into_element()
    };
    #[cfg(feature = "server")]
    let sse_line: Element =
        ui! { Typography(content = "live SSE (client builds only)".to_string(), muted = true) };

    let header: Vec<Element> = vec![
        ui! { Typography(content = "Server-fn todos".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Every interaction is a #[server] call. Open the network tab \
                — adds/toggles/deletes are single HTTP requests; their response folds \
                straight into local state via `async_reducer`, no second refetch.".to_string(),
                muted = true,
            )
        },
        status_line,
        sse_line,
        ui! { button(label = "Refresh".to_string(), on_click = on_refresh) },
        ui! { button(label = "Add: Buy milk".to_string(), on_click = on_add_milk) },
        ui! { button(label = "Add: Walk the dog".to_string(), on_click = on_add_dog) },
        ui! { button(label = "Add: Demo idealyst".to_string(), on_click = on_add_demo) },
    ];

    // Reactive list. Each row gets a clone of the toggle + delete
    // reducers so its buttons trigger the right action.
    let toggle_for_rows = toggle.clone();
    let delete_for_rows = delete.clone();
    let list = ui! {
        flat_list::<Todo, _, (), _>(
            todos,
            |_idx, t: &Todo| t.id,
            fixed_size(64.0),
            move |_idx, t: &Todo| {
                todo_row(t.clone(), todos, toggle_for_rows.clone(), delete_for_rows.clone())
            },
        )
    };

    let body: Vec<Element> = vec![
        ui! { Stack(gap = StackGap::Md, padding = StackPadding::Lg) { header } },
        list,
    ];

    // Wrap in a viewport-filling root. The iOS/Android backends size the
    // mounted root to the window only if the root node declares it — a bare
    // flex `Stack` shrinks to content and renders blank on native (web hides
    // this because the DOM body already fills the viewport). This mirrors the
    // `welcome` example's `page_sheet` and the navigator SDK's default fill.
    ui! {
        view(style = root_fill()) {
            Stack(gap = StackGap::Sm, padding = StackPadding::None) { body }
        }
    }
}

/// Full-screen column root: `width:100% height:100%` so the mounted tree
/// fills the native window (see the wrap in `app()`).
fn root_fill() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        flex_direction: Some(FlexDirection::Column),
        ..Default::default()
    }))
}


/// One row in the list. Tapping the toggle button fires the
/// shared `toggle` reducer; the delete button fires `delete`.
///
/// The row's button label is a REACTIVE closure (`Fn() -> String`)
/// that re-reads the matching todo from the `todos` signal each
/// time the signal changes. This matters because `flat_list` keeps
/// in-place rows mounted across data changes — the per-row subtree
/// is built once (when the row enters the viewport) and isn't
/// rebuilt on every `todos.set(...)`. Anything that should change
/// with the data needs to be expressed reactively inside the row,
/// not computed once at build time.
fn todo_row(
    t: Todo,
    todos: Signal<Vec<Todo>>,
    toggle: AsyncReducer<u64, ServerError>,
    delete: AsyncReducer<u64, ServerError>,
) -> Element {
    let id = t.id;
    let initial_title = t.title.clone();
    let initial_done = t.done;
    let label = move || {
        let list = todos.get();
        match list.iter().find(|x| x.id == id) {
            Some(cur) => {
                if cur.done {
                    format!("[x] {}", cur.title)
                } else {
                    format!("[ ] {}", cur.title)
                }
            }
            None => {
                if initial_done {
                    format!("[x] {}", initial_title)
                } else {
                    format!("[ ] {}", initial_title)
                }
            }
        }
    };
    let on_toggle = move || toggle.trigger(id);
    let on_delete = move || delete.trigger(id);
    let children: Vec<Element> = vec![
        ui! { button(label = label, on_click = on_toggle) },
        ui! { button(label = "delete".to_string(), on_click = on_delete) },
    ];
    ui! { Stack(gap = StackGap::Sm, padding = StackPadding::Md) { children } }
}

/// Click handler that fires the create reducer with a canned title.
fn make_adder(
    create: AsyncReducer<CreateTodo, ServerError>,
    title: &'static str,
) -> impl Fn() + 'static {
    move || {
        create.trigger(CreateTodo {
            title: title.to_string(),
        });
    }
}
