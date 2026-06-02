# `storage`

Cross-platform **plaintext** key-value persistence for *non-secret* app
data — preferences, UI state, caches. One async `Storage` trait that maps to
each platform's native key-value store; `platform_storage(name)` hands you
the right backend for the current target as an `Arc<dyn Storage>`.

It's the `AsyncStorage` half of the storage split. Its sibling
[`credentials`](../credentials) is the `SecureStore` half — for secrets.

> **This is NOT secure storage.** Everything written here is in the clear
> and readable by anything with access to the device/browser profile (other
> code in the process, any script on the web origin, anyone with the device
> unlocked, a backup). There is deliberately no "secure" mode and no
> encryption — a store that *looked* secure but wasn't would be worse than
> an honestly-insecure one. **Never put tokens, keys, or any secret here**;
> use [`credentials`](../credentials).

```rust
use storage::platform_storage;

# async fn demo() -> Result<(), storage::StorageError> {
let store = platform_storage("my_app");      // Arc<dyn Storage>
store.set("theme", "dark").await?;
assert_eq!(store.get("theme").await?, Some("dark".to_string()));
store.remove("theme").await?;
# Ok(())
# }
```

## What you get

One async, object-safe `Storage` trait (held as `Arc<dyn Storage>`) with
string values — encode structured data (e.g. JSON) caller-side:

- `get(key) -> Option<String>` — value, or `None` if absent.
- `set(key, value)` — store, replacing any existing value.
- `remove(key)` — idempotent.
- `clear()` — remove every key owned by *this* store (its namespace only).

Three ways to get a `Storage`:

- `platform_storage(name)` — the platform's native plaintext store,
  namespaced by `name`. **Infallible** to construct; backend errors surface
  per-operation as `StorageError`.
- `MemoryStorage::new()` — in-process, all targets. For tests and ephemeral
  state.
- `FileStorage::new(path)` — a JSON file on disk (native targets only); the
  whole map is rewritten on each mutation, suited to small key sets.

Every backend delivers the **same shape** — the platforms diverge in
mechanism, not in the trait you call.

## Per-platform mechanism

| Target | Backend (`platform_storage`) |
| --- | --- |
| web (wasm32) | `localStorage`, keys prefixed with `name` |
| iOS / macOS / tvOS | `NSUserDefaults`, keys prefixed with `name` (objc2) |
| Android | `SharedPreferences` file named `name` (JNI) |
| Windows / Linux / other native | a JSON `FileStorage` under the user's data dir (`%APPDATA%` / `$XDG_DATA_HOME` / `~/.local/share`) |

`clear()` removes only this store's own keys (the `name` namespace), not the
whole platform store.

Operations are `async` for a uniform surface across the genuinely-async web
backend and the synchronous native ones. On native the work runs
synchronously inside the returned future (fine for the small payloads this
is meant for; a high-throughput caller should batch).

## Permissions

None. Plaintext key-value storage needs no OS permission on any platform, so
this crate declares no capability and the CLI injects nothing.

## Scope

A flat string→string namespace per store — the unopinionated raw capability.
Typed values, reactive bindings, and migration helpers are deliberately left
to a higher-level SDK rather than baked in here.
