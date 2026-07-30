# `toolbar`

A `Toolbar` primitive for desktop window chrome, built on the
framework's scene-registry extension mechanism. On **macOS** it attaches
an `NSToolbar` to the host window's title bar, on **Windows** a
Common-Controls toolbar, on **GTK4** a `GtkHeaderBar`. Everywhere else it
is a zero-size no-op, so it renders nothing wherever it's mounted.

This follows the project's mobile-first philosophy: toolbar / menu chrome
belongs in third-party SDKs, not the host capability set.

```rust
// App bootstrap: pass `register` to the boot entry's registry seam.
host_appkit::newcore::run_with(
    || app(),
    host_appkit::newcore::RunOptions::default(),
    |registry| toolbar::register(registry),
)?;

// Inside a `ui!` block — the toolbar's in-tree footprint is zero, so its
// position doesn't matter visually. Convention: mount near the root so
// the items closure is owned by a long-lived scope.
let count = signal(0_i32);
ui! {
    View {
        { toolbar::Toolbar(toolbar::ToolbarProps {
            items: Box::new(move || vec![
                toolbar::ToolbarItem::button("Save")
                    .icon("square.and.arrow.down")
                    .on_click({ let c = count.clone(); move || c.set(c.get() + 1) })
                    .into(),
                toolbar::ToolbarItem::flexible_space(),
                toolbar::ToolbarItem::button("Reload")
                    .on_click(|| log::info!("reload"))
                    .into(),
            ]),
            ..Default::default()
        }) }
        // ... rest of the app
    }
}
```

## Per-platform behavior

| Target | Mechanism |
| --- | --- |
| macOS | `NSToolbar` on the host window. Buttons become `NSToolbarItem`s (icon = SF Symbol), `Separator`/`Space`/`FlexibleSpace` map to the matching system item identifiers. |
| Windows | A `ToolbarWindow32` (Common Controls) child HWND parented under the host window and registered with the layout tree, so its in-tree placement DOES matter — mount it at the top of the root flex column with a fixed height. Buttons route through WM_COMMAND; the host must call `toolbar::flush_pending(&mut backend)` once per WndProc frame to make freshly-built buttons clickable. Label-only in v1 (no `TB_SETIMAGELIST` icons). |
| GTK4 (Linux) | A `GtkHeaderBar` installed as the window titlebar via `set_titlebar`; the in-tree node is a zero-size `gtk::Box`. Icons unwired in v1 (SF Symbol names don't map to Freedesktop icon names). `set_visible` is a no-op — the HeaderBar lives on the window, not in the tree. |
| iOS / Android / web / terminal / wgpu / ESP / CPU | `register` installs the External-placeholder handler; the in-tree primitive renders zero-size and `items` is never evaluated. |

## Reactive items

[`ToolbarProps::items`] is a `Box<dyn Fn() -> Vec<ToolbarItem>>`. Each
desktop handler wraps the call in a `runtime_world::effect`, so reading a
signal inside the closure makes the toolbar rebuild when that signal
changes — the same reactive shape as `webview::url`. Initial visibility is
set via [`ToolbarProps::visible`]; runtime visibility changes go through
[`ToolbarHandle::set_visible`].

## Items

Build items with the constructor helpers, not the enum directly — the
builder shape leaves room to grow optional fields (tooltip, badge, custom
view) without breaking call sites:

- [`ToolbarItem::button`] → [`ToolbarButton`] — chain
  [`.icon(...)`](ToolbarButton::icon), [`.tooltip(...)`](ToolbarButton::tooltip),
  [`.on_click(...)`](ToolbarButton::on_click). `Into<ToolbarItem>` lets you
  mix builders and raw variants in one `vec![]`.
- [`ToolbarItem::separator`], [`ToolbarItem::space`],
  [`ToolbarItem::flexible_space`] — divider, fixed gap, and a flex gap that
  pushes following items to the trailing edge.

## Imperative ops

Bind a [`ToolbarHandle`] via [`ToolbarBind::bind`] to drive ops after
mount:

```rust
let r: Ref<ToolbarHandle> = /* … */;
ui! { { toolbar::Toolbar(props).bind(r) } }
// later:
r.with(|h| h.set_visible(false));
```

## Registration + the click-flush discipline

`register` is generic over the host and type-dispatches ONCE at
registration time: it downcasts `&mut Registry<H>` to the platform's
concrete registry and installs the native handler on hit; every other `H`
gets the External-placeholder handler. A desktop build therefore serves
both its real backend and `Registry<HostMock>` (the test harness) from
one function. Mount-path cost is zero — the dispatch happens before any
element exists.

Every desktop leg wraps each button `on_click` so the backend's
`newcore::schedule_flush()` runs after the author code returns. Native
target-action / WM_COMMAND / `connect_clicked` all fire OUTSIDE the
runtime's wrapped dispatch sites, so an unwrapped click would leave a
signal write staged in the world forever. The macOS AppKit machinery
itself lives in `src/macos_shared.rs`, kept free of any runtime import so
the item translation is unit-testable on its own.

[`ToolbarProps::items`]: src/lib.rs
[`ToolbarProps::visible`]: src/lib.rs
[`ToolbarHandle`]: src/lib.rs
[`ToolbarHandle::set_visible`]: src/lib.rs
[`ToolbarBind::bind`]: src/lib.rs
[`ToolbarItem::button`]: src/lib.rs
[`ToolbarItem::separator`]: src/lib.rs
[`ToolbarItem::space`]: src/lib.rs
[`ToolbarItem::flexible_space`]: src/lib.rs
[`ToolbarButton`]: src/lib.rs
[`ToolbarButton::icon`]: src/lib.rs
[`ToolbarButton::tooltip`]: src/lib.rs
[`ToolbarButton::on_click`]: src/lib.rs

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. Tick each
item as you exercise it. This primitive is a real widget only on macOS; every
other target renders a zero-size no-op, so most checks verify the *absence* is
clean.

**Automated**
- [ ] `cargo test -p toolbar` — the placeholder-arm op-log suite + the
  item-translation / click-wrap unit tests
- [ ] `cargo check -p toolbar --target x86_64-pc-windows-gnu`
- [ ] `cargo check -p toolbar --target x86_64-unknown-linux-gnu` (needs
  GTK4 dev libs for the target; not reachable from a bare macOS host)

**Rendering / behavior**
- [ ] **macOS** — a real `NSToolbar` appears on the host window's title bar with the
  `items` (buttons = `NSToolbarItem`, SF Symbol icons; separator/space/flexible-space
  map to the system identifiers); clicking a button fires its `on_click`; the
  reactive `items` closure rebuilds the toolbar when a read signal changes;
  `ToolbarHandle::set_visible(...)` shows/hides it. A signal write inside a
  click must be visible immediately (the `schedule_flush` wrap).
- [ ] **Windows** — ⚠️ not yet run. A Common-Controls toolbar renders at the
  mounted frame; `flush_pending` drained per frame makes buttons clickable;
  the reactive `items` closure rebuilds the button list.
- [ ] **GTK4 (Linux)** — ⚠️ not yet run. A HeaderBar replaces the window
  titlebar with the `items`; clicking fires `on_click`; the reactive closure
  rebuilds the packed children.
- [ ] **iOS / Android / web / terminal / gpu** — the External placeholder renders
  **zero-size**; confirm nothing visible appears and there's no layout artifact
  wherever the `Toolbar(...)` is mounted.
