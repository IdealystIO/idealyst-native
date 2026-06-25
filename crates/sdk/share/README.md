# `share`

Hand content to the system **share sheet** so the user can send it to another
app — the outbound counterpart to [`file-picker`](../file-picker). Where
`file-picker` brings content *in* from another app, `share` sends it *out*:
build a `ShareContent` (some text, a URL, and/or file references), call
`share(content).await`, and the OS shows its own share UI — the iOS/macOS share
sheet, the Android chooser, or the browser's Web Share dialog.

`share` is async because the share sheet is modal: the future resolves when the
user either picks a target (`ShareOutcome::Completed`) or dismisses it
(`ShareOutcome::Dismissed`).

```rust
use share::{share, ShareContent, ShareOutcome};

# async fn demo() -> Result<(), share::ShareError> {
let outcome = share(
    ShareContent::text("Look at this!").with_url("https://idealyst.dev"),
)
.await?;

match outcome {
    ShareOutcome::Completed => { /* the user sent it somewhere */ }
    ShareOutcome::Dismissed => { /* the user cancelled */ }
}
# Ok(())
# }
```

## What you get

One async free function over a small builder:

- `ShareContent` — a builder with optional `text`, `url`, `files`, and a
  `title`/subject. Ergonomic constructors `ShareContent::text(..)` /
  `::url(..)` / `::files([..])`, chainable `with_text` / `with_url` /
  `with_file` / `with_title` setters.
- `share(content) -> Result<ShareOutcome, ShareError>` — present the sheet and
  await the result.
- `ShareOutcome::{Completed, Dismissed}` — best-effort (see below).
- `ShareError::{Backend, NotSupported, NothingToShare}`.

Every backend delivers the **same shape** — the platforms diverge in mechanism,
not in the function you call. Sharing empty content (no text, url, or files) is
`ShareError::NothingToShare`; a title alone isn't shareable content.

### Best-effort outcome

Not every platform reports `Completed` vs `Dismissed` reliably. iOS's
`UIActivityViewController` and the Web Share API report both; Android's plain
`createChooser` and macOS's `NSSharingServicePicker` don't surface a result we
subscribe to, so they report `Completed` once the share UI has run. Treat
`Completed` as "the share UI ran", not a hard guarantee the user committed.

## Per-platform mechanism

| Target | Mechanism | Notes |
| --- | --- | --- |
| iOS | `UIActivityViewController`, presented from the top view controller (objc2) | ⚠️ compile-checked only |
| macOS | `NSSharingServicePicker`, shown relative to the key window (objc2) | ⚠️ compile-checked only |
| Android | `Intent.ACTION_SEND` wrapped in `Intent.createChooser`, `startActivity` (JNI) | ⚠️ compile-checked only; text/url only — see below |
| web (wasm32) | `navigator.share({ title, text, url })` (Web Share API) | needs a user gesture + secure context; `files` ignored |
| Windows / Linux / other native | `ShareError::NotSupported` | no uniform native outbound-share surface |

The iOS, macOS, and Android backends are **compile-checked only** — the share
UI resolves at runtime on a real device/desktop session (the same posture as
`file-export`). The web backend runs wherever `navigator.share` is available.

**Android file sharing is a seam.** Attaching files needs a `content://` URI
from a `FileProvider` declared in the app manifest (a raw `file://` URI throws
`FileUriExposedException` on modern Android). That provider wiring is app-level
config this SDK can't inject generically, so `share(...)` with *only* files (no
text/url) returns `NotSupported` on Android. Text/URL sharing works fully. A
future layer can add a FileProvider shim (like `file-export`'s Kotlin helper).

**Web `files` are ignored.** `navigator.share({ files })` needs `File` objects;
this crate's `files` are `PathBuf` references, which have no meaning in the web
sandbox. The web backend shares `title`/`text`/`url`. Where `navigator.share`
is missing (unsupported browser, insecure context) it returns `NotSupported`
rather than faking a fallback.

## Permissions

None. The share sheet is user-initiated UI — bringing it up and picking a
target is what does the sending — so this SDK needs no OS permission on any
platform, and the CLI injects nothing.

## Scope

Text / URL / file references handed to the OS share sheet — the unopinionated
raw capability. Custom activity types, share extensions, rich link previews,
and materializing in-memory bytes into shareable web `File`s are later layers,
deliberately left out of this crate.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p share` — builder + `is_empty` + `NothingToShare` guard
- [ ] `cargo build -p share --features catalog` — recipes/docs compile
- [ ] `cargo build -p share --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — from a button (user gesture, secure context), `share(text + url)` opens the Web Share dialog with the right targets; picking one sends the content; cancel → `Dismissed`. `files` are ignored; no `navigator.share` → `NotSupported`.
- [ ] **iOS** — `share(text + url)` presents `UIActivityViewController`; pick a target (Messages/Mail) and confirm it received the text/url/subject; cancel → `Dismissed`.
- [ ] **macOS** — `NSSharingServicePicker` appears by the key window; a picked service receives the content (outcome is best-effort `Completed`).
- [ ] **Android** — `share(text/url)` shows the `createChooser` sheet and the picked app receives the content. Files-only share returns `NotSupported` (FileProvider seam); verify text/url works fully.
- [ ] Empty (or title-only) content returns `NothingToShare` before any sheet appears, on every target.
