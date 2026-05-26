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
    component, fixed_size, flat_list, signal, ui, Effect, IntoPrimitive, Primitive, Signal,
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

#[server]
pub async fn delete_todo(id: u64) -> Result<(), ServerError> {
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    let mut todos = state.todos.lock().unwrap();
    let before = todos.len();
    todos.retain(|t| t.id != id);
    if todos.len() == before {
        return Err(ServerError::failed(format!("no todo with id {id}")));
    }
    Ok(())
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

    // Reactive state.
    let todos: Signal<Vec<Todo>> = signal!(Vec::new());
    let status: Signal<String> = signal!("loading...".to_string());

    // Fire one fetch on mount. `Effect::new` runs once at creation;
    // because we don't read any signals reactively inside it, it
    // doesn't re-run. We `mem::forget` the handle so the effect
    // outlives this function's stack frame.
    {
        let todos = todos;
        let status = status;
        let e = Effect::new(move || {
            refresh(todos, status);
        });
        std::mem::forget(e);
    }

    let on_refresh = move || refresh(todos, status);

    let on_add_milk = make_adder(todos, status, "Buy milk");
    let on_add_dog = make_adder(todos, status, "Walk the dog");
    let on_add_demo = make_adder(todos, status, "Demo idealyst");

    // Reactive status line.
    let status_line = runtime_core::text(move || status.get()).into_primitive();

    let header: Vec<Primitive> = vec![
        ui! { Typography(content = "Server-fn todos".to_string(), kind = TypographyKind::H1) },
        ui! {
            Typography(
                content = "Every interaction is a #[server] call. Open the network tab to watch \
                the requests coalesce into one /_srv/_batch POST per microtask.".to_string(),
                tone = TypographyTone::Muted,
            )
        },
        status_line,
        ui! { Button(label = "Refresh".to_string(), on_click = on_refresh) },
        ui! { Button(label = "Add: Buy milk".to_string(), on_click = on_add_milk) },
        ui! { Button(label = "Add: Walk the dog".to_string(), on_click = on_add_dog) },
        ui! { Button(label = "Add: Demo idealyst".to_string(), on_click = on_add_demo) },
    ];

    // The list. `flat_list(data, key, item_size, render_item)`
    // re-renders when `todos` changes; only the rows in the
    // viewport are realized. The `S` type parameter is unused
    // (PhantomData inside flat_list); turbofish `()` to pin it.
    let list = ui! {
        flat_list::<Todo, _, (), _>(
            todos,
            |_idx, t: &Todo| t.id,
            fixed_size(64.0),
            move |_idx, t: &Todo| todo_row(t.clone(), todos, status),
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

/// One row in the list. Tapping the row's button toggles its
/// `done` flag; the second button deletes it.
fn todo_row(t: Todo, todos: Signal<Vec<Todo>>, status: Signal<String>) -> Primitive {
    let id = t.id;
    let label = if t.done {
        format!("[x] {}", t.title)
    } else {
        format!("[ ] {}", t.title)
    };
    let on_toggle = move || {
        runtime_core::driver::spawn_async(async move {
            match toggle_todo(id).await {
                Ok(_) => refresh(todos, status),
                Err(e) => status.set(format!("toggle failed: {e}")),
            }
        });
    };
    let on_delete = move || {
        runtime_core::driver::spawn_async(async move {
            match delete_todo(id).await {
                Ok(()) => refresh(todos, status),
                Err(e) => status.set(format!("delete failed: {e}")),
            }
        });
    };
    let children: Vec<Primitive> = vec![
        ui! { Button(label = label, on_click = on_toggle) },
        ui! { Button(label = "delete".to_string(), on_click = on_delete) },
    ];
    ui! { Stack(gap = StackGap::Sm, padding = StackPadding::Md) { children } }
}

/// Re-fetch the list and update the status signal. Idempotent;
/// each call replaces the prior data + status.
fn refresh(todos: Signal<Vec<Todo>>, status: Signal<String>) {
    status.set("loading...".to_string());
    runtime_core::driver::spawn_async(async move {
        match list_todos().await {
            Ok(list) => {
                todos.set(list);
                status.set("ready".to_string());
            }
            Err(e) => status.set(format!("error: {e}")),
        }
    });
}

/// Build a click handler that posts `create_todo` with the given
/// canned title, then re-fetches. Returned as a `Fn()` closure
/// ready to drop into a `Button(on_click = ...)`.
fn make_adder(
    todos: Signal<Vec<Todo>>,
    status: Signal<String>,
    title: &'static str,
) -> impl Fn() + 'static {
    move || {
        let input = CreateTodo {
            title: title.to_string(),
        };
        runtime_core::driver::spawn_async(async move {
            match create_todo(input).await {
                Ok(_t) => refresh(todos, status),
                Err(e) => status.set(format!("add failed: {e}")),
            }
        });
    }
}
