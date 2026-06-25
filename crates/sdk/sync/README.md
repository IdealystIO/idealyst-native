# `sync` — offline-first cache + server synchronization

A client-side cache layer that synchronizes server artifacts with a device.
An app **downloads** a named slice of server data (a *partition*), **reads
and mutates** it offline, and on **reconnect** replays queued mutations and
reconciles server changes against local edits — without silently losing
either side.

It plugs into the framework's existing pieces: server functions for the
wire, the reactive `Signal` for the UI binding, context `provide`/`inject`
for app-wide access, and the [`storage`](../storage) SDK for persistence.

```text
   app entity T  +  Merge policy  +  two #[server] fns (pull/push)
                              │
                   ┌──────────┴───────────┐
                   │      SyncEngine       │   provided at app root
                   │  ┌─────────────────┐  │
                   │  │  Partition<T>   │  │   one per "project:123"
                   │  │  Signal<Vec<T>> │──┼──▶ UI binds here
                   │  │  outbox · cache │  │
                   │  └─────────────────┘  │
                   └──────────┬───────────┘
                       storage (durable)
```

## The protocol (two messages)

Everything is client-side bookkeeping over two RPCs, generic over the app's
entity `T`:

- **`pull(partition, cursor) -> { mode, changes, next_cursor, has_more }`**
  — "what changed since this cursor". With no cursor (or one the server has
  pruned past) it answers with a full `Snapshot`; otherwise an incremental
  `Delta`. Deletes are **explicit tombstones**, never silent omission.

- **`push(partition, ops) -> { results }`** — apply queued mutations. Each
  `Op` carries an **idempotency key** (safe retries) and a **base revision**
  (concurrency detection). Results are positional: `Applied`, `Duplicate`,
  `Conflict { server_value }`, `Gone`, or `Rejected`.

The app authors the *server bodies* of these (their own `#[server]` fns over
their DB) and the *merge policy*; the SDK owns persistence, the outbox, the
engine, and the protocol. The crate has **no dependency on `server`** — the
protocol *types* are the only contract, bridged by the [`Transport`] trait.

## Correctness model (v1)

*Correct* = **no silent data loss, crash-safe across restarts, conflicts
surfaced to the app**, under a single-writer-per-device assumption. It is
**not** automatic multi-device convergence, real-time push, or CRDTs — those
layer on top of the same cursor + outbox + merge primitives.

Crash-safety rests on one fact about [`storage`]: a single `set` is atomic,
nothing spanning two keys is. So each of a partition's three pieces of state
(cache, outbox, cursor) is one blob at one key, and the engine orders the
writes so that:

> **the outbox commits before the UI acknowledges; record-state commits
> before the outbox pops; record-data commits before the cursor advances.**

Recovery needs no special path — reloading the persisted state and replaying
is exactly the steady-state startup, made safe by the idempotency key. These
orderings are verified under simulated mid-operation crashes in
[`tests/crash_recovery.rs`](tests/crash_recovery.rs).

## Using it

### 1. Define an entity + its merge policy

```rust
use sync::{Merge, MergeCtx, Resolution};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Project { id: String, name: String, archived: bool }

impl Merge for Project {
    fn merge(ctx: MergeCtx<'_, Self>) -> Resolution<Self> {
        // 3-way: `ctx.base` is the frozen ancestor the local edit was made
        // on top of; `ctx.local` / `ctx.incoming` are the two sides.
        match (ctx.local, ctx.incoming) {
            (Some(local), Some(server)) => {
                // ... field-level merge, or Resolution::Unresolved to ask a UI
                Resolution::Merged(/* ... */ local.clone())
            }
            (Some(_), None) => Resolution::TakeLocal,     // server deleted, keep edit
            (None, Some(_)) => Resolution::TakeIncoming,  // local deleted, keep server
            _ => Resolution::TakeIncoming,
        }
    }
}
```

### 2. Author the two server fns and wire the transport

```rust
#[server]
pub async fn pull_projects(req: sync::PullRequest)
    -> Result<sync::PullResponse<Project>, ServerError> {
    // Read your DB / change-log. See `sync::reference::Authority` for a
    // complete in-memory reference of exactly what this must do.
}

#[server]
pub async fn push_projects(req: sync::PushRequest<Project>)
    -> Result<sync::PushResponse<Project>, ServerError> {
    // Apply ops with idempotency-key dedup + base-rev concurrency checks.
}

// One line bridges them to the engine:
sync::sync_transport!(ProjectTransport, Project,
    pull = pull_projects, push = push_projects);
```

### 3. Provide the engine and drive a partition

```rust
// At the app root:
let engine = sync::SyncEngine::new(storage::platform_storage("sync"), device_id);
runtime_core::provide(engine.clone());

// Anywhere below (ideally a long-lived provider component):
let engine = runtime_core::inject::<sync::SyncEngine>().unwrap();
let projects = engine
    .partition::<Project>("project:123", std::rc::Rc::new(ProjectTransport))
    .await?;

// Bind the UI reactively:
let items = projects.items();          // Signal<Vec<Project>>
// ... ui! { for p in items.get() { ProjectRow(project = p) } }

// Mutate (queues to the durable outbox; flushes when online):
projects.upsert("p1", Project { /* ... */ }).await?;
projects.delete("p2").await?;

// Connectivity:
engine.set_online(false);              // mutations accumulate offline
engine.set_online(true);
projects.sync_now().await?;            // pull, then flush — the reconnect action

// Conflicts the engine couldn't auto-resolve:
for id in projects.conflicts() {
    projects.resolve(id, Resolution::TakeIncoming).await?;
}
```

## Pluggable storage

Persistence goes through the [`SyncStore`] trait — load/save the three
per-partition concerns (records, outbox, cursor). Two impls ship and are
conformance-tested: `KvSyncStore` over any [`storage::Storage`] (the
production default — `localStorage` / `UserDefaults` / `SharedPreferences`
/ a file), and `MemorySyncStore` for tests. Implement `SyncStore` yourself
to back sync with SQLite, encrypted files, or anything else, and pass it to
`SyncEngine::new`. Crash-safety lives in the engine's *ordering* of the
`save_*` calls, so a backend only has to make each individual write
durable.

## One engine per store (important)

A `SyncEngine` **owns its storage namespace and client id**. Do not run two
engines over the *same* namespace concurrently — each writes its full
in-memory state back, so they blindly overwrite each other's cache and
outbox, and a stale instance's save can erase another's not-yet-synced
work. Two browser tabs sharing `localStorage` are exactly this trap.

On **native** there's one app instance, so a plain `Partition` over disk
storage is correct. On **web**, multiple tabs share one origin — use
[`SharedPartition`](crate::SharedPartition), which does **leader election**
(`navigator.locks`): exactly one tab owns the engine + storage and the
others proxy through it over `BroadcastChannel`, with automatic promotion
when the leader's tab closes. App code is identical to `Partition`
(`entries`/`items`/`upsert`/`delete`/`sync_now`), so you open a
`SharedPartition` on every platform and let it coordinate.

Don't try to keep tabs apart with browser storage instead:
`sessionStorage` is copied into a duplicated/linked tab and `localStorage`
is shared across all tabs, so two tabs collide either way — leader election
is the robust answer. (The client id is half of every idempotency key, so
it must be unique per logical client regardless; with leader election
there's one logical client per browser.)

## Per-item status in the UI

`Partition::entries()` is a reactive `Signal<Vec<Entry<T>>>` where each
`Entry` carries an [`EntryStatus`] (`Synced` / `Pending` / `Conflicted`) —
so a list can render a sync indicator per item without the app tracking any
state. See `examples/todo-sync-demo` for a **full-stack** TODO app that does
exactly this: a server holds the data behind `#[server]` pull/push fns (via
`sync_transport!`), and the client downloads tasks, edits them offline
(badges turn *Pending*), and reconnects to replay/reconcile (badges settle
to *Synced*). Run both with one command: `idealyst dev --web examples/todo-sync-demo`.

## Reference server

The `reference-server` feature exposes [`sync::reference::Authority`], a
complete, schema-agnostic, in-memory implementation of the server half:
monotonic revision, explicit-tombstone change-log, idempotency dedup, and
cursor-expiry → snapshot fallback. Use it directly for small/in-memory
state, to run an example fully in-process, or as the precise spec a
DB-backed server must reproduce.

## Explicit non-goals for v1

Real-time `#[subscription]` push · multi-device automatic convergence (we
*surface* conflicts, not auto-converge) · CRDTs (implement inside your
`Merge` policy if you want them) · query/filter-scoped partitions (a
partition syncs in full) · cross-partition transactions. Each of these
layers on top of the v1 primitives without reworking them.

[`Transport`]: https://docs.rs/sync
[`storage`]: ../storage
