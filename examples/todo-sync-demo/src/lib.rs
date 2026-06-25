//! `todo-sync-demo` — a **full-stack** offline-first TODO list on the
//! [`sync`] SDK.
//!
//! - The **server** (`--bin server --features server`) holds the
//!   authoritative tasks behind two `#[server]` fns — [`pull_todos`] and
//!   [`push_todos`] — and serves the wasm client from the same origin.
//! - The **client** downloads tasks into a local cache, lets you edit them
//!   offline (each task's badge turns **Pending**), and on reconnect
//!   replays the queued edits and reconciles. The per-task status comes
//!   straight from [`Partition::entries`].
//!
//! The `#[server]` macro keys off the `server` cargo feature: the wasm
//! build gets RPC stubs that POST to `/_srv/<fn>`, the server build gets
//! the real bodies (which delegate to a [`sync::reference::Authority`]).
//!
//! ## Run it
//!
//! ```text
//! idealyst dev --web examples/todo-sync-demo
//! ```
//!
//! That builds the wasm client, starts the server (serving both the bundle
//! and the `/_srv/*` API at `http://127.0.0.1:3000`), and rebuilds on save.

use std::cell::RefCell;
use std::rc::Rc;
// `Arc` is only used by the server fns' `Arc<AppState>` extractor.
#[cfg(feature = "server")]
use std::sync::Arc;

use idea_ui::{
    install_idea_theme, light_theme, tone, typography_kind, variant, Badge, Button, Card,
    CardPadding, Field, Stack, StackAlign, StackAxis, StackGap, StackPadding, Typography,
};
use runtime_core::driver::spawn_async;
use runtime_core::{component, rx, signal, ui, Element};
use serde::{Deserialize, Serialize};
use server::{server, ServerError};
use sync::{
    sync_transport, EntryStatus, Merge, MergeCtx, PollingTrigger, Resolution, SharedPartition,
    SyncEngine,
};

// ============================================================================
// Wire type — shared between server and client — plus its merge policy.
// ============================================================================

/// A single task. `updated_at` is the client wall-clock at edit time; the
/// merge policy uses it for last-write-wins. Its sync `Id` is passed
/// separately to `upsert`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub title: String,
    pub done: bool,
    // `#[serde(default)]` so a cache written before this field existed still
    // deserializes (as updated_at: 0) instead of erroring on load.
    #[serde(default)]
    pub updated_at: u64,
}

impl Merge for Todo {
    fn merge(ctx: MergeCtx<'_, Self>) -> Resolution<Self> {
        // Last edit wins, ordered by the client-stamped `updated_at`. Swap
        // for `sync::policy::server_wins` / `manual` / a custom body to
        // change the strategy — it's per entity.
        sync::policy::last_write_wins(ctx, |t| t.updated_at)
    }
}

/// Client wall-clock in millis, used to stamp edits for last-write-wins.
/// (LWW by wall-clock is only as good as the device clock — fine for a
/// single user across their own devices; a real multi-user app would use a
/// server time or a logical clock.)
fn now_millis() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A stable **per-browser** client id (persisted in `localStorage`). This
/// is safe to share across tabs now because leader election ensures only
/// ONE tab (the leader) ever runs the engine + touches storage — so there's
/// one logical client per browser. It must be stable so a promoted follower
/// resumes the same client identity (the id is half of every idempotency
/// key).
fn device_id() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        const KEY: &str = "todo-sync-device-id";
        if let Some(ls) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            if let Ok(Some(existing)) = ls.get_item(KEY) {
                return existing;
            }
            let fresh = format!(
                "device-{}-{}",
                js_sys::Date::now() as u64,
                (js_sys::Math::random() * 1.0e9) as u64
            );
            let _ = ls.set_item(KEY, &fresh);
            return fresh;
        }
        "device-fallback".to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "device-native".to_string()
    }
}

/// Poll interval for the auto-sync `PollingTrigger` (keeps tabs fresh even
/// without manual Sync). Only the leader tab actually does work.
const POLL_INTERVAL_MS: u32 = 4000;

// ============================================================================
// Server-only state — the authoritative store behind the two endpoints.
// In a real app this is a DB; here it's an in-memory `Authority`.
// ============================================================================

#[cfg(feature = "server")]
pub mod state {
    use super::Todo;
    use std::sync::Mutex;
    use sync::reference::Authority;

    pub struct AppState {
        pub authority: Mutex<Authority<Todo>>,
    }

    impl AppState {
        pub fn new() -> Self {
            let mut authority = Authority::new();
            // Seeds use updated_at: 0 so any real client edit is "newer".
            authority.seed(
                "welcome",
                Todo {
                    title: "Welcome — toggle me done".into(),
                    done: false,
                    updated_at: 0,
                },
            );
            authority.seed(
                "groceries",
                Todo {
                    title: "Buy groceries".into(),
                    done: false,
                    updated_at: 0,
                },
            );
            Self {
                authority: Mutex::new(authority),
            }
        }
    }

    impl Default for AppState {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(feature = "server")]
use crate::state::AppState;

// ============================================================================
// The two sync endpoints. On the wasm client these are RPC stubs; on the
// server they delegate to the authoritative `Authority`.
// ============================================================================

/// Pull changes for a partition (cursor delta, or a snapshot).
#[server]
pub async fn pull_todos(
    req: sync::PullRequest,
) -> Result<sync::PullResponse<Todo>, ServerError> {
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    let authority = state.authority.lock().unwrap();
    Ok(authority.pull(req.cursor.as_ref(), req.limit))
}

/// Apply a batch of queued mutations.
#[server]
pub async fn push_todos(
    req: sync::PushRequest<Todo>,
) -> Result<sync::PushResponse<Todo>, ServerError> {
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    let mut authority = state.authority.lock().unwrap();
    Ok(authority.push(req.ops))
}

// Generate the `TodoTransport` the engine calls — it bridges to the two
// `#[server]` fns above (their client RPC stubs on the wasm build).
sync_transport!(TodoTransport, Todo, pull = pull_todos, push = push_todos);

/// Point the `server` SDK at the API host. Web uses the page origin (the
/// server bin serves bundle + API from one port); native dev points at the
/// host running the server.
fn configure_server() {
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        server::configure(server::ClientConfig::new(origin));
    }
    #[cfg(all(not(feature = "server"), not(target_arch = "wasm32")))]
    {
        let base = if cfg!(target_os = "android") {
            "http://10.0.2.2:3000" // Android emulator's host-loopback alias
        } else {
            "http://127.0.0.1:3000"
        };
        server::configure(server::ClientConfig::new(base));
    }
}

// ============================================================================
// Client UI.
// ============================================================================

/// One task row with its live sync badge.
#[derive(Default)]
struct TodoRowProps {
    id: String,
    title: String,
    done: bool,
    status: EntryStatus,
    partition: Option<SharedPartition<Todo>>,
}

#[component]
fn TodoRow(props: &TodoRowProps) -> Element {
    let id = props.id.clone();
    let title = props.title.clone();
    let done = props.done;
    let partition = props.partition.clone();

    // Toggle done. `upsert` queues + flushes on the leader (or proxies to it
    // from a follower); it's async so it runs on the event loop, never
    // blocking the UI. Offline, the flush is a no-op and the edit stays
    // Pending until reconnect.
    let toggle: Rc<dyn Fn()> = {
        let partition = partition.clone();
        let id = id.clone();
        let title = title.clone();
        Rc::new(move || {
            let Some(p) = partition.clone() else { return };
            let id = id.clone();
            let todo = Todo {
                title: title.clone(),
                done: !done,
                updated_at: now_millis(),
            };
            spawn_async(async move {
                let _ = p.upsert(id, todo).await;
            });
        })
    };

    let delete: Rc<dyn Fn()> = {
        let partition = partition.clone();
        let id = id.clone();
        Rc::new(move || {
            let Some(p) = partition.clone() else { return };
            let id = id.clone();
            spawn_async(async move {
                let _ = p.delete(id).await;
            });
        })
    };

    let check = if done { "☑".to_string() } else { "☐".to_string() };

    let badge = match props.status {
        EntryStatus::Synced => {
            ui! { Badge(label = "Synced".to_string(), tone = tone::Success, variant = variant::Soft) }
        }
        EntryStatus::Pending => {
            ui! { Badge(label = "Pending".to_string(), tone = tone::Warning, variant = variant::Soft) }
        }
        EntryStatus::Conflicted => {
            ui! { Badge(label = "Conflict".to_string(), tone = tone::Danger, variant = variant::Soft) }
        }
    };

    ui! {
        Card(padding = CardPadding::Md) {
            Stack(axis = StackAxis::Row, gap = StackGap::Md, align = StackAlign::Center) {
                Button(label = check, on_click = toggle, tone = tone::Neutral, variant = variant::Soft)
                Typography(content = title)
                badge
                Button(label = "Delete".to_string(), on_click = delete, tone = tone::Danger, variant = variant::Ghost)
            }
        }
    }
}

/// The reactive task list, bound to the partition's status-aware entries.
#[derive(Default)]
struct TodoListProps {
    partition: Option<SharedPartition<Todo>>,
}

#[component]
fn TodoList(props: &TodoListProps) -> Element {
    let Some(partition) = props.partition.clone() else {
        return ui! { view {} };
    };
    let entries = partition.entries();

    ui! {
        Stack(gap = StackGap::Sm) {
            Typography(
                content = rx!(format!(
                    "{} pending",
                    entries.get().iter().filter(|e| e.status != EntryStatus::Synced).count()
                )),
                muted = true,
            )
            // Key by the row's full content, not just its id. A keyed
            // `for` builds each item once and does NOT re-render it when its
            // data changes (per-row dynamic state is meant to live in
            // signals, à la reactive-loops). Our row data lives in the sync
            // cache, so we make the key change whenever the entry changes —
            // the row rebuilds on a done-toggle or a Pending→Synced badge
            // flip. Cheap for a small list.
            for entry in entries,
                key = format!(
                    "{}|{}|{}|{:?}",
                    entry.id.0, entry.value.title, entry.value.done, entry.status
                )
            {
                TodoRow(
                    id = entry.id.0.clone(),
                    title = entry.value.title.clone(),
                    done = entry.value.done,
                    status = entry.status,
                    partition = Some(partition.clone()),
                )
            }
        }
    }
}

/// Root component. Builds the engine, kicks off the async load + initial
/// download, and renders the status-aware list once ready.
///
/// `#[component]` is load-bearing: it gives the root an owning reactive
/// scope so the list's keyed `for` loop keeps its subscription alive and
/// re-renders when `entries` changes (toggles, deletes, new tasks). Without
/// it, fine-grained `rx!` text bindings still update but the `for` loop and
/// row contents go stale until a full page reload.
#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());
    configure_server();

    // Durable per-browser store. Safe across tabs because the SharedPartition
    // below elects ONE leader tab that exclusively owns this engine +
    // storage; followers proxy through it (no shared-store clobber). The
    // transport reaches the server via the generated `TodoTransport`.
    let engine = SyncEngine::with_kv(storage::platform_storage("todo-sync"), device_id());
    let transport = Rc::new(TodoTransport);

    let loaded = signal!(false);
    let online = signal!(true);
    let new_title = signal!(String::new());
    let status = signal!("starting…".to_string());
    let role = signal!("connecting…".to_string());
    let part_cell: Rc<RefCell<Option<SharedPartition<Todo>>>> = Rc::new(RefCell::new(None));

    // Async bootstrap: open the multi-tab-coordinated partition (becomes
    // leader or follower), reveal the UI, and start the auto-sync poller.
    {
        let cell = part_cell.clone();
        let engine = engine.clone();
        spawn_async(async move {
            match SharedPartition::open(engine.clone(), "todos", transport).await {
                Ok(sp) => {
                    // Leadership is acquired asynchronously (the lock callback
                    // fires later), so mirror the reactive flag into `role`
                    // rather than reading it once.
                    let ls = sp.leader_signal();
                    runtime_core::Effect::new(move || {
                        role.set(if ls.get() { "leader".into() } else { "follower".into() });
                    })
                    .persist();
                    *cell.borrow_mut() = Some(sp);
                    loaded.set(true);
                    status.set("ready".to_string());
                    // Periodic auto-sync (only the leader tab does work).
                    engine.start_auto_sync(Rc::new(PollingTrigger::new(POLL_INTERVAL_MS)));
                }
                Err(e) => status.set(format!("open failed: {e}")),
            }
        });
    }

    let add: Rc<dyn Fn()> = {
        let cell = part_cell.clone();
        Rc::new(move || {
            let Some(sp) = cell.borrow().clone() else {
                status.set("not ready".to_string());
                return;
            };
            let title = new_title.get();
            if title.trim().is_empty() {
                return;
            }
            // Collision-free id by scanning current entries.
            let id = {
                let existing = sp.entries().get();
                let mut n = 1u32;
                loop {
                    let candidate = format!("local-{n}");
                    if !existing.iter().any(|e| e.id.0 == candidate) {
                        break candidate;
                    }
                    n += 1;
                }
            };
            new_title.set(String::new());
            status.set(format!("adding {id}…"));
            spawn_async(async move {
                let todo = Todo { title, done: false, updated_at: now_millis() };
                match sp.upsert(id.clone(), todo).await {
                    Ok(()) => status.set(format!("added {id}")),
                    Err(e) => status.set(format!("{id}: error: {e}")),
                }
            });
        })
    };

    let toggle_online: Rc<dyn Fn()> = {
        let cell = part_cell.clone();
        Rc::new(move || {
            let now = !online.get();
            online.set(now);
            status.set(if now { "online".to_string() } else { "offline".to_string() });
            if let Some(sp) = cell.borrow().clone() {
                sp.set_online(now); // routed to the leader
            }
        })
    };

    let sync_now: Rc<dyn Fn()> = {
        let cell = part_cell.clone();
        Rc::new(move || {
            let Some(sp) = cell.borrow().clone() else {
                status.set("not ready".to_string());
                return;
            };
            status.set("syncing…".to_string());
            spawn_async(async move {
                match sp.sync_now().await {
                    Ok(()) => status.set("synced".to_string()),
                    Err(e) => status.set(format!("sync failed: {e}")),
                }
            });
        })
    };

    let on_title: Rc<dyn Fn(String)> = Rc::new(move |v: String| new_title.set(v));

    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) {
            Typography(content = "Offline TODO".to_string(), kind = typography_kind::H1)
            Typography(
                content = "Open in two tabs: one is the leader (owns sync), the other a follower. Edits in either show in both; go offline to queue edits, reconnect to sync.".to_string(),
                muted = true,
            )

            Stack(axis = StackAxis::Row, gap = StackGap::Md, align = StackAlign::Center) {
                Button(
                    label = rx!(if online.get() { "● Online".to_string() } else { "○ Offline".to_string() }),
                    on_click = toggle_online,
                    tone = tone::Neutral,
                    variant = variant::Soft,
                )
                Button(label = "Sync now".to_string(), on_click = sync_now, tone = tone::Primary, variant = variant::Soft)
            }

            Typography(content = rx!(format!("this tab: {} · status: {}", role.get(), status.get())), muted = true)

            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::End) {
                Field(
                    label = Some("New task".to_string()),
                    value = new_title,
                    on_change = on_title,
                    placeholder = Some("What needs doing?".to_string()),
                )
                Button(label = "Add".to_string(), on_click = add, tone = tone::Primary, variant = variant::Filled)
            }

            if loaded.get() {
                TodoList(partition = part_cell.borrow().clone())
            } else {
                Typography(content = "Loading…".to_string(), muted = true)
            }
        }
    }
}

// ============================================================================
// CLI-generated wrapper hooks.
// ============================================================================

/// SDK-registration hook the platform wrappers call before mount.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

/// Recorder-side registration for the native dev-server sidecar.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(done: bool, updated_at: u64) -> Todo {
        Todo {
            title: "t".into(),
            done,
            updated_at,
        }
    }

    /// The Task policy is last-write-wins by `updated_at`: the more recent
    /// edit wins regardless of which side it's on. (This is the "mark
    /// complete vs. incomplete offline" resolution.)
    #[test]
    fn lww_keeps_the_newer_edit() {
        // Local marked it done later than the server marked it not-done.
        let local = todo(true, 200);
        let incoming = todo(false, 100);
        assert!(matches!(
            Todo::merge(MergeCtx {
                base: None,
                local: Some(&local),
                incoming: Some(&incoming),
            }),
            Resolution::TakeLocal
        ));

        // Server's edit is newer → it wins.
        let local = todo(true, 100);
        let incoming = todo(false, 200);
        assert!(matches!(
            Todo::merge(MergeCtx {
                base: None,
                local: Some(&local),
                incoming: Some(&incoming),
            }),
            Resolution::TakeIncoming
        ));
    }
}
