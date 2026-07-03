# `toolbar`

A `Toolbar` primitive for desktop window chrome, built on the
framework's `Element::External` mechanism. On **macOS** it attaches an
`NSToolbar` to the host window's title bar; on other platforms it's
currently a no-op (the in-tree footprint is a 0-size view, so it renders
nothing wherever it's mounted).

This follows the project's mobile-first philosophy: toolbar / menu chrome
belongs in third-party SDKs, not the core Backend trait.

```rust
// App bootstrap: pass a register-extensions closure to the host runner.
host_appkit::run_with(
    app,
    host_appkit::RunOptions::default(),
    |backend| {
        toolbar::register(backend);
    },
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
| macOS | `NSToolbar` on the host window. Buttons become `NSToolbarItem`s (icon = SF Symbol), `Separator`/`Space`/`FlexibleSpace` map to the matching system item identifiers. The handler installs an `Effect`, so the reactive `items` closure re-runs and rebuilds the toolbar whenever the signals it reads change. |
| Windows / Linux | `register` is wired but the backends don't yet expose `register_external`; treated as a no-op for now. |
| iOS / Android / web / terminal / wgpu / ESP / CPU | `register` is a no-op; the in-tree primitive renders zero-size. |

## Reactive items

[`ToolbarProps::items`] is a `Box<dyn Fn() -> Vec<ToolbarItem>>`. The
macOS handler wraps the call in an `Effect`, so reading a signal inside
the closure makes the toolbar rebuild when that signal changes — the same
reactive shape as `webview::url`. Initial visibility is set via
[`ToolbarProps::visible`]; runtime visibility changes go through
[`ToolbarHandle::set_visible`] from an `effect!`.

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

**Rendering / behavior**
- [ ] **macOS** — a real `NSToolbar` appears on the host window's title bar with the
  `items` (buttons = `NSToolbarItem`, SF Symbol icons; separator/space/flexible-space
  map to the system identifiers); clicking a button fires its `on_click`; the
  reactive `items` closure rebuilds the toolbar when a read signal changes;
  `ToolbarHandle::set_visible(...)` shows/hides it.
- [ ] **Windows / Linux** — `register` is wired but the backends don't yet expose
  `register_external`; verify it's a clean no-op (no toolbar, no crash).
- [ ] **iOS / Android / web / terminal / gpu** — `register` is a no-op and the
  in-tree primitive renders **zero-size**; confirm nothing visible appears and there's
  no layout artifact wherever the `Toolbar(...)` is mounted.
