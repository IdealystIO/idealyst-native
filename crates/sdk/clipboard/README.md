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
- `NotSupported` — no backend on this target (desktop Windows / Linux).

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
| Windows / Linux / other native | `NotSupported` (a desktop clipboard crate is out of scope) |

The **web** path is the genuinely-runnable backend. The Apple (iOS / tvOS
/ macOS) and Android backends are **compile-checked only** — not yet
verified on a device/emulator. Note the Apple backend's `NSPasteboard` /
`UIPasteboard` classes live in AppKit / UIKit, which a bare `cargo test`
binary doesn't link, so the round-trip can't run there; it runs in a real
app build where the framework links those.

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
alongside text", not a different shape. The desktop (Windows / Linux)
clipboard is also out of scope — those targets return `NotSupported`.

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
