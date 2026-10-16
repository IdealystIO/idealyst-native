# macOS backend — design notes

> **Status: implemented.** `backend-macos` (`crates/backend/macos`) implements
> `runtime_scene::Host` plus every `runtime_vocabulary::caps::*Ops` trait over
> AppKit views in `src/newcore.rs`, delegating to the mechanism code in
> `src/imp/`; `host-appkit` (`crates/gpu-backend/host/appkit`) owns the
> NSApplication boot. Shared objc2 helpers live in `backend-apple-core`
> (`crates/backend/apple/core`). These notes record the AppKit mechanism
> decisions the backend is built on; for the backend seam itself see
> [`backend.md`](backend.md).

A first-class, **desktop-native** AppKit backend: the same `Host` +
capability-trait seam every backend implements, the same Taffy-backed flex
layout the iOS and Android backends use, with desktop idioms (cursor,
hover, keyboard focus, real AppKit controls) instead of mobile metaphors.

Concretely: a Mac user should see a real `NSWindow` with traffic-light
controls, AppKit controls (`NSButton`, `NSTextField`, `NSSwitch`,
`NSSlider`, …), pointer hover and cursors, keyboard focus through the
responder chain, and a real menu bar. Navigation chrome (sidebars, tab
bars, headers) is author layout around a navigator's outlet, the same as
on every other backend; the `toolbar` and `menu` SDKs supply the native
`NSToolbar` and menu bar.

---

## Why a separate backend instead of "iOS that runs on Mac"

The user explicitly asked for a desktop-native backend, not a Catalyst-
or iPad-on-Mac–style port. The trait surface stays the same — what
changes is everything below it:

| Concern                  | iOS (`backend-ios-mobile`)            | macOS (`backend-macos`)                                       |
| ------------------------ | ------------------------------------- | -------------------------------------------------------------- |
| UI toolkit               | UIKit                                 | AppKit                                                         |
| Base view                | `UIView`                              | `NSView` (overridden `isFlipped` to give top-left origin)      |
| Window                   | One full-screen `UIWindow`            | `NSWindow` + traffic lights + resize + (later) multi-window    |
| Input                    | Touch                                 | Cursor (mouse/trackpad), hover, scroll-wheel, keyboard         |
| Coordinate system        | Top-left origin, points               | Bottom-left by default — we override per-view via `isFlipped`  |
| Anchored portal          | Custom overlay window + CADisplayLink | `NSPopover` (native, AppKit positions + tracks anchor for us)  |
| Viewport portal          | UIWindow / top-of-host overlay        | Top-of-host overlay NSView (NSWindow sheet for true modality)  |
| Text input single-line   | `UITextField`                         | `NSTextField`                                                  |
| Text input multi-line    | `UITextView`                          | `NSTextView` inside `NSScrollView`                             |
| Toggle / Slider          | `UISwitch` / `UISlider`               | `NSSwitch` / `NSSlider`                                        |
| Button                   | `UIButton`                            | `NSButton` w/ `bezelStyle = .rounded` by default               |
| Image                    | `UIImageView` (+ intrinsic measure)   | `NSImageView` (+ intrinsic measure)                            |
| Activity indicator       | `UIActivityIndicatorView`             | `NSProgressIndicator` (`.spinning`)                            |
| Cursor                   | n/a                                   | `NSCursor.pointingHand` for pressables/links; `NSTrackingArea` |
| Dark mode                | `UITraitCollection.userInterfaceStyle`| `NSAppearance.currentAppearance`                               |
| Vibrancy                 | n/a                                   | `NSVisualEffectView` (sidebars, popovers — opt-in)             |
| Render loop              | NSTimer @ 60 Hz (`raf_loop`)          | `CVDisplayLink` (NSScreen.displayLink on macOS 14+); NSTimer interim |
| Scheduler                | NSTimer + main DispatchQueue          | Identical — `dispatch_get_main_queue` + NSTimer, lifted to apple-core |

The Taffy layout tree, asset registries, animation, reactivity, wire
protocol — all unchanged.

---

## Coordinate system: top-left via `isFlipped`

AppKit defaults to a bottom-left origin (Y increases upward). Taffy
emits frames in a top-left origin. We resolve this once, at the view
level, by overriding `isFlipped` to return `true` on every container
view we create. Inside a flipped view, AppKit applies frames using
top-left origin — matching iOS, Android, web, and Taffy.

```rust
// src/imp/view.rs
declare_class!(
    pub struct FlippedView;

    unsafe impl ClassType for FlippedView {
        type Super = NSView;
        // ...
    }

    unsafe impl FlippedView {
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool { true }
    }
);
```

Every `create_view` / `create_pressable` / `create_link` returns a
`FlippedView`. AppKit-supplied leaves (NSTextField, NSButton,
NSSlider, NSSwitch, NSImageView) keep their default orientation —
their internal layout is opaque to us anyway. The Taffy layout pass
sets their `frame` directly, computed in a flipped parent's
coordinate space.

---

## Layout & rendering pass

Identical to iOS:

1. The framework walks the primitive tree and calls `create_*` /
   `insert` / `apply_style`, building a parallel `LayoutTree`
   (Taffy) alongside the NSView tree. Each NSView pointer maps to a
   Taffy `LayoutNode` in `view_to_layout: HashMap<usize, (Retained<NSView>, LayoutNode)>`.
2. `apply_style` updates Taffy properties (size, margin, padding,
   flex direction, gap, etc.) on the corresponding node — same code
   path the iOS backend uses, since Taffy's `Style` shape is platform-
   agnostic.
3. After build, `finish` runs `layout.compute(root, host_bounds)`
   and walks `view_to_layout` to assign `frame` on every registered
   view. NSScrollView's `documentView.frame` and `contentSize` get
   synced from the bounding rect of its Taffy children — mirrors
   the iOS `scroll_views: HashSet<usize>` post-pass.
4. Re-layout: AppKit calls `layout` / `viewWillDraw` on size change.
   We hook this via an NSView subclass at the window's content view
   level (`LayoutObserverView`-equivalent) that calls back into the
   backend to dispatch a fresh layout pass. This mirrors the iOS
   observer pattern documented at
   [backend-ios-mobile/src/imp/mod.rs:374](../crates/backend/ios/mobile/src/imp/mod.rs#L374).

### Intrinsic-size measurers

Same pattern as iOS — every leaf widget that has a non-trivial
intrinsic size (`NSTextField` as label, `NSButton`, `NSSwitch`,
`NSSlider`, `NSImageView`, `NSTextView`) gets a Taffy `measure_fn`
that returns `view.intrinsicContentSize` (or `.fittingSize` /
`sizeThatFits:` where appropriate): without a measurer these
widgets collapse to 0×0 in Taffy and hit-test against an empty rect.

NSTextField (label mode) wraps via
`cell.wraps = true` + `cell.usesSingleLineMode = false` + per-call
`cellSizeForBounds:` to compute wrapped height — equivalent to the
UILabel `sizeThatFits:` pattern.

---

## Portal & overlay mapping

`create_portal` mounts a viewport-spanning container into the host
window's `contentView` and registers it as its own Taffy root.

- **Viewport** portals place their content child with the container's
  flex style — the region the author asked for.
- **Anchored** portals (tooltips, popovers, dropdowns) position the
  content child absolutely against the trigger's viewport rect, using the
  placement mapping every backend shares, and re-pin it every frame so it
  tracks the anchor through scrolls, relayouts, and window resizes. This
  is deliberately not `NSPopover`: one placement mapping on every backend
  means a popover lands in the same place everywhere. See
  `src/imp/portal.rs` and `src/portal_policy.rs`.

A dismissing backdrop is a click-consuming view underneath the content that
calls the overlay's dismiss handler.

---

## Cursor, hover, focus

- **Pressables / Links / Buttons**: install an `NSTrackingArea` on
  the view at construction time (or whenever the view's bounds
  change — implement `updateTrackingAreas`). `mouseEntered` →
  `NSCursor.pointingHand.push()`; `mouseExited` → `NSCursor.pop()`.
- **Focus**: AppKit responder chain. NSView's `acceptsFirstResponder`
  + `becomeFirstResponder` + `resignFirstResponder` give us the
  hooks for `:focus` styling. Reactive `state` bits already have a
  Focused flag — wire it the same way iOS wires the equivalent.
  Tab / Shift-Tab navigation between focusable views works via
  AppKit's default `keyView` chain — we don't override
  `nextKeyView` / `previousKeyView`; AppKit's heuristic (geometric
  order within the window) is used.
- **Hover style**: framework's `:hover` state bit, set from the
  NSTrackingArea callbacks. No extra primitive needed — same path
  as `:focus`.

---

## Color, dark mode, materials

- `Color` → `NSColor` adapter lives next to the iOS `Color → UIColor`
  adapter; both intermediate to a CGColor under the hood.
- Dark mode: `NSAppearance.currentAppearance` resolves to
  `aqua` / `darkAqua` / `vibrantLight` / `vibrantDark` /
  `accessibilityHighContrast*`. Backend's `color_scheme()` returns
  `Light` for aqua, `Dark` for darkAqua, `Auto` otherwise.
- Watching for appearance changes: KVO on
  `NSApplication.effectiveAppearance` OR override
  `viewDidChangeEffectiveAppearance` on `LayoutObserverView`. Fire
  a theme-changed callback into runtime-core's appearance hook.
- Vibrancy (`NSVisualEffectView` materials) is not exposed.

---

## Animation, scheduler, render loop

These three pieces map 1:1 to iOS — the scheduler in particular is
**the same code** (NSTimer + main DispatchQueue), shared through
`backend-apple-core`. The animation backend (CALayer +
CABasicAnimation / CAKeyframeAnimation) is also shared: NSView's
backing CALayer is the same Core Animation API as UIView's. The
existing `animated.rs` shape in `backend-ios-mobile` ports over
near-1:1 — the only differences are which property setters route
to CALayer transforms vs. AppKit view properties.

Render loop: an NSTimer-based driver at ~60 Hz on the main thread
(`src/render_loop.rs`), the same shape as iOS. `CVDisplayLink` (or
`NSScreen.displayLink` on macOS 14+) would pace more exactly but needs a
thread hop back to the main thread; the driver's consumer (an embedded GPU
preview) doesn't need frame-exact pacing. The animation clock
(`backend-apple-core::scheduler::raf_loop`) runs in common run-loop modes
so springs keep advancing during scroll tracking; the render driver runs
in the default mode so it yields to scrolling.

---

## Non-goals (explicit, so they don't sneak in)

- No UIKit-on-Mac (Catalyst). AppKit only.
- No SwiftUI bridge. AppKit only.
- No SwiftUI lookalike paint pipeline. Native AppKit widgets only.
- No reimplementation of NSToolbar / the menu bar in Taffy — the
  `toolbar` and `menu` SDKs use AppKit's own.
- No "phone simulator on Mac" (`variant-phone`/`variant-tablet`
  variants already do that via wgpu — different code path entirely).

---
