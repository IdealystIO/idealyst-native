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
    install_idea_theme, light_theme, stack, typography, StackGap, StackPadding,
    TypographyKind, TypographyTone,
};
use runtime_core::{
    async_reducer, component, fixed_size, flat_list, signal, ui, AsyncReducer, AsyncStatus,
    Effect, IntoPrimitive, Primitive, Signal,
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
// Point the `server` SDK at the API host. On wasm we use the
// browser's `window.location.origin`, which lines up with the
// server bin serving the bundle and the API from the same port.
// On terminal/native targets the demo doesn't have a meaningful
// default — caller would replace via `server::configure(...)`.
// ============================================================================

fn configure_server() {
    // Only the wasm client needs this — the server bin compiles
    // the lib too (for inventory registration etc.) but never calls
    // `app()`, and `server::configure` doesn't exist on the server
    // feature build anyway (it's gated under `#[cfg(not(feature = "server"))]`
    // in the SDK).
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        server::configure(server::ClientConfig { base_url: origin });
    }
}

// ============================================================================
// Per-target SDK-handler registration hook the CLI-generated
// wrappers invoke before mount. No third-party SDKs in this demo —
// the function exists per-target with an empty body so the
// wrapper-side `register_extensions(&mut Backend)` call compiles.
// ============================================================================

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios_mobile::IosBackend) {}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}

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
pub fn app() -> Primitive {
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
    .into_primitive();

    let on_refresh = {
        let refresh = refresh.clone();
        move || refresh.trigger(())
    };

    let on_add_milk = make_adder(create.clone(), "Buy milk");
    let on_add_dog = make_adder(create.clone(), "Walk the dog");
    let on_add_demo = make_adder(create.clone(), "Demo idealyst");

    let header: Vec<Primitive> = vec![
        ui! { Typography(content = "Server-fn todos".to_string(), kind = TypographyKind::H1) },
        ui! {
            Typography(
                content = "Every interaction is a #[server] call. Open the network tab \
                — adds/toggles/deletes are single HTTP requests; their response folds \
                straight into local state via `async_reducer`, no second refetch.".to_string(),
                tone = TypographyTone::Muted,
            )
        },
        status_line,
        ui! { Button(label = "Refresh".to_string(), on_click = on_refresh) },
        ui! { Button(label = "Add: Buy milk".to_string(), on_click = on_add_milk) },
        ui! { Button(label = "Add: Walk the dog".to_string(), on_click = on_add_dog) },
        ui! { Button(label = "Add: Demo idealyst".to_string(), on_click = on_add_demo) },
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

    let body: Vec<Primitive> = vec![
        ui! { Stack(gap = StackGap::Md, padding = StackPadding::Lg) { header } },
        list,
    ];

    ui! {
        Stack(gap = StackGap::Sm, padding = StackPadding::None) { body }
    }
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
) -> Primitive {
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
    let children: Vec<Primitive> = vec![
        ui! { Button(label = label, on_click = on_toggle) },
        ui! { Button(label = "delete".to_string(), on_click = on_delete) },
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
