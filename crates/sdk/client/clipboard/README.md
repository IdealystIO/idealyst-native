# `clipboard`

Cross-platform access to the **system clipboard** — copy and paste plain
text. Two small async free functions that map to each platform's native
clipboard API; the surface is identical everywhere, the platforms diverge
only in mechanism.

```rust
use clipboard::{set_text, text};

# async fn demo() -> Result<(), clipboard::ClipboardError> {
set_text("hello").await?;
assert_eq!(text().await?, Some("hello".to_string()));
# Ok(())
# }
```

## What you get

Two `async` free functions over plain text:

- `set_text(text) -> ()` — copy a string onto the clipboard, replacing its
  current contents.
- `text() -> Option<String>` — read the clipboard's text; `None` when it's
  empty or holds only a non-text representation (e.g. an image).

Failures surface as `ClipboardError`:

- `Backend(String)` — the platform clipboard API failed (a web
  `clipboard-read` permission denial, a missing window, an Obj-C / JNI
  error). The string carries the platform detail.
- `NotSupported` — no backend on this target (desktop Windows / other
  native). Linux has a real backend and reports live failures as `Backend`.

The functions are `async` for a uniform surface: the web backend
(`navigator.clipboard`) is genuinely Promise-based, while the native
backends do their work synchronously inside the returned future. Every
backend delivers the **same shape** — the platforms diverge in mechanism,
not in the functions you call.

## Per-platform mechanism

| Target | Mechanism |
| --- | --- |
| web (wasm32) | `navigator.clipboard.writeText` / `readText` (JsFuture) — runnable |
| iOS / tvOS | `UIPasteboard.generalPasteboard` `setString:` / `string` (objc2) — compile-checked only |
| macOS | `NSPasteboard.generalPasteboard` `clearContents` + `setString:forType:` / `stringForType:` with `public.utf8-plain-text` (objc2) — compile-checked only |
| Android | `ClipboardManager` (`Context.CLIPBOARD_SERVICE`) `setPrimaryClip(ClipData.newPlainText(...))` / `getPrimaryClip().getItemAt(0).coerceToText(context)` via JNI — compile-checked only |
| Linux (desktop) | `arboard` — plain-text `set_text` / `get_text`; covers X11 and Wayland (`wlr-data-control` / GTK) with one crate — runnable under a desktop session |
| Windows / other native | `NotSupported` (a desktop clipboard crate for these is out of scope) |

The **web** path is the genuinely-runnable backend. The Apple (iOS / tvOS
/ macOS) and Android backends are **compile-checked only** — not yet
verified on a device/emulator. Note the Apple backend's `NSPasteboard` /
`UIPasteboard` classes live in AppKit / UIKit, which a bare `cargo test`
binary doesn't link, so the round-trip can't run there; it runs in a real
app build where the framework links those.

The **Linux** backend (`arboard`) is runnable under a real desktop session
(X11 or a Wayland compositor). A headless process — no `DISPLAY` /
`WAYLAND_DISPLAY` — can't open a clipboard connection, so `Clipboard::new()`
fails there and surfaces as `Backend`.

On X11/Wayland the clipboard has no independent store — its contents are
served on demand by whichever client owns the selection. Since these are
stateless free functions, `set_text` releases ownership as soon as it
returns, so persistence relies on a running **clipboard manager**
(GNOME/KDE's built-in one, `klipper`, `wl-clip-persist`, …) taking
ownership of the offered text — the standard desktop mechanism. On a bare
session with no manager, the text is gone the instant `set_text` returns
and a later `text()` reads `None`. (We intentionally don't use arboard's
`SetExtLinux::wait()`, which would block the call until another client
claims the clipboard.)

On **macOS**, a write must `clearContents` first (this bumps the change
count and drops the prior owner's representations) or `setString:forType:`
is a no-op — that's the documented `NSPasteboard` contract. The empty
string read back from web / Android is normalized to `None` so the absent
case matches the native backends' nil.

## Permissions

None — no OS manifest permission on any platform, so this crate declares no
capability and the CLI injects nothing.

On **web**, reading the clipboard (`text()`) requires a user gesture (it
must run in the call stack of a click/keypress) and may prompt for the
`clipboard-read` permission at runtime; a denial — or a call made without a
gesture — surfaces as `ClipboardError::Backend`. That's a runtime browser
concern, not a build-time manifest entry.

## Scope

Plain text only. Images, rich text, and multiple simultaneous
representations are deliberately left to a later, higher-level SDK rather
than baked in here. The extension seam is clean: "more representations
alongside text", not a different shape.

**Linux desktop is in scope** (this reverses an earlier exclusion): copy /
paste is fundamental on desktop, so Linux gets a real `arboard` backend
covering X11 and Wayland. The remaining desktop targets (Windows / other
native) are still out of scope and return `NotSupported`.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p clipboard` — portable logic (error `Display`)
- [ ] `cargo build -p clipboard --features catalog` — recipes/docs compile
- [ ] `cargo build -p clipboard --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — `set_text("…")` via the demo, switch to another tab/app, paste (⌘V/Ctrl-V) — the copied text appears; `text()` called inside a user gesture returns it (a denial / no-gesture call surfaces as `Backend`).
- [ ] **iOS** — copy in-app, paste into Notes/another app — matches; `text()` reads back what another app copied.
- [ ] **Android** — copy in-app, paste into another app — matches; `text()` round-trips (empty clipboard → `None`).
- [ ] **macOS** — copy in-app, ⌘V into TextEdit — matches; a second `set_text` after `clearContents` overwrites (not a no-op).
- [ ] **Linux** — under a desktop session, `cargo test -p clipboard --test portable -- --ignored` (the `linux_round_trip` test); or copy in-app and Ctrl-V into another app — matches. Headless (no `DISPLAY`/`WAYLAND_DISPLAY`) surfaces as `Backend`.
