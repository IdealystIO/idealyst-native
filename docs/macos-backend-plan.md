# macOS backend — plan

A first-class, **desktop-native** AppKit backend. Same `Backend` trait
the rest of the framework already drives; same Taffy-backed flex layout
the iOS and Android backends use; but every chrome decision —
navigators, drawers, tabs, toolbars, focus, cursor — is redesigned for
desktop idioms instead of leaning on mobile metaphors.

Concretely: the user shouldn't see a "bottom tab bar that scaled up,"
"swipe-from-edge drawer," or "full-screen modal stack." They should
see what a Mac user expects: a window with a unified toolbar, a
collapsible sidebar in an NSSplitView, NSToolbar-hosted segmented
controls or proper tab view controllers, NSPopovers for anchored
overlays, traffic-light window controls, keyboard navigation, hover
states, and a real menu bar.

---

## Scope (initial)

### In

- New crate `backend-macos` implementing `framework_core::Backend`.
- New shared substrate `backend-apple-core` (renamed/promoted from
  the current `backend-ios-core`) holding the cross-Apple pieces both
  iOS and macOS need: CoreText font registry, color parsing helpers,
  any other UIKit-or-AppKit-agnostic Foundation/CoreGraphics work.
- Taffy-backed flex layout via `native-layout` (same as iOS/Android).
- Cursor input: hover, click, drag, scroll-wheel/trackpad. No touch.
- Multi-window support deferred — start with a single host NSWindow
  set up by the app's `main`. Multi-window is a follow-up after the
  core trait surface is solid.
- AppKit chrome mappings for the three navigator shapes
  (Stack/Tab/Drawer) — see [§Navigator mapping](#navigator-mapping).
- A `host-appkit` crate (or equivalent in `crates/host/`) that owns
  the NSApplication boot, top-level NSWindow, NSAppDelegate, run loop.
  Mirrors `host-winit`'s role for the sim runtime.
- Welcome example running natively on macOS via this backend.

### Out (deferred, explicit)

- **AAS thin-client mode** — `backend-macos` initial version is
  local-render only. The `aas-shell` feature flag can be wired up
  later mirroring `backend-ios-mobile/aas-shell`.
- **Multi-window**, **NSDocument**, **tabbed windows**, **status bar
  items**, **Touch Bar**. All listed under "potential follow-ups" so
  they're not lost, but not v1.
- **Drag and drop** beyond what AppKit gives us for free (e.g.,
  promised file URLs into NSTextView). First-class
  `NSDraggingSource`/`Destination` integration is a separate effort.
- **`backend-macos-stack`** (a UIStackView-equivalent NSStackView
  variant kept as a fallback). The iOS-stack crate exists for
  historical reasons; macOS starts Taffy-only from day one.

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
| Stack-nav chrome         | `UINavigationController` push/pop     | NSToolbar (back chevron, title) + content view-swap            |
| Drawer chrome            | `UIScrollView` + scrim overlay        | `NSSplitView` w/ collapsible sidebar (NSSourceList style)      |
| Tab chrome               | Bottom `UITabBar`                     | Top NSToolbar segmented control OR `NSTabViewController`       |
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

## Crate layout

```
crates/backend/
  apple/                       <-- new shared substrate
    core/
      Cargo.toml
      src/
        lib.rs                 // re-exports + cfg gating
        font.rs                // CoreText font registry  (moved from ios/core)
        color.rs               // parse_color → CGColor / NSColor / UIColor adapters
        scheduler.rs           // NSTimer + main DispatchQueue (no UIKit deps)
        log.rs                 // NSLog shim (no UI deps)
        asset.rs               // image cache shared shape (NSImage-or-UIImage adapter)
  ios/
    core/
      // becomes a thin UIKit-flavored crate that re-exports apple-core
      // and adds the UIKit-specific style application + render loop.
    mobile/
    tv/
  macos/                       <-- new
    Cargo.toml
    src/
      lib.rs
      stub.rs                  // non-darwin no-op shim
      imp/
        mod.rs                 // MacosBackend struct + Backend impl
        node.rs                // MacosNode enum
        view.rs                // FlippedView (NSView w/ isFlipped = true)
        callbacks.rs           // Target/action helpers; closures-as-NSObjects
        cursor.rs              // NSTrackingArea + cursor swap helpers
        scroll.rs              // create_scroll_view (NSScrollView)
        text.rs                // create_text + measure_fn for NSTextField label
        text_input.rs          // create_text_input (NSTextField)
        text_area.rs           // create_text_area (NSTextView in NSScrollView)
        button.rs              // create_button (NSButton + bezel style)
        toggle.rs              // create_toggle (NSSwitch)
        slider.rs              // create_slider (NSSlider)
        activity.rs            // create_activity_indicator (NSProgressIndicator)
        image.rs               // create_image / NSImage cache
        icon.rs                // CAShapeLayer-on-NSView icon impl (mirrors ios/icon.rs)
        link.rs                // pressable + cursor swap
        graphics.rs            // CAMetalLayer-backed NSView + wgpu surface
        animated.rs            // AnimProp → CALayer / NSView setter
        layout.rs              // layout pass + frame application + scroll contentSize sync
        portal.rs              // NSPopover (anchored) + overlay NSView (viewport)
        navigator/
          mod.rs               // shared NavigatorEntry / toolbar plumbing
          stack.rs             // NSToolbar + content swap
          tab.rs               // NSSegmentedControl in toolbar OR NSTabViewController
          drawer.rs            // NSSplitView w/ NSSourceList sidebar
        external.rs            // Primitive::External handler registry
crates/host/
  appkit/                      <-- new
    Cargo.toml
    src/
      lib.rs                   // run() entry point
      app_delegate.rs          // NSApplicationDelegate (open file, terminate, dock)
      window.rs                // NSWindow setup, traffic lights, content view
      menu.rs                  // application menu bar setup (File/Edit/View/Window/Help)
      render_loop.rs           // CVDisplayLink / NSScreen.displayLink driver
```

`backend-apple-core` becomes the cross-Apple substrate. Both
`backend-ios-core` and `backend-macos` depend on it. Things move in
two passes:

1. **Lift** (no behavior change): CoreText font registry + `Color →
   CGColor` parsing + NSLog + scheduler → `apple-core`. `ios-core`
   re-exports from `apple-core` so existing callers don't break.
2. **Split**: macOS-specific NSColor adapter and UIKit-specific
   UIColor adapter live in their respective leaf crates; both
   consume the shared `CGColor` intermediate.

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
`sizeThatFits:` where appropriate). Reason logged at
[[project_ios_intrinsic_size_measurer]]: without a measurer these
widgets collapse to 0×0 in Taffy and hit-test against an empty rect.

NSTextField (label mode) wraps via
`cell.wraps = true` + `cell.usesSingleLineMode = false` + per-call
`cellSizeForBounds:` to compute wrapped height — equivalent to the
UILabel `sizeThatFits:` pattern.

---

## Navigator mapping

### Stack navigator → NSToolbar + content swap

`Navigator` becomes a single content area in the window. Push/pop
swaps the visible NSView for the new screen's root. There is **no**
animated slide (that's a mobile metaphor). Optional crossfade is a
later polish.

- The host NSWindow's NSToolbar carries:
  - A back chevron (NSToolbarItem with `NSImage(systemSymbolName:
    "chevron.backward")`) — enabled when `stack.len() > 1`.
  - The current screen's title (NSToolbarItem with a label).
  - `ScreenOptions.header_left` / `header_right` → NSToolbarItems.
- `ScreenOptions.header_background` → either toolbar's background
  via NSVisualEffectView material, or per-screen toolbar tint.
- `ScreenOptions.header_shown == false` → `window.toolbar = nil`
  for that screen (alternative: `window.toolbarStyle = .preference`
  with empty items — TBD which feels more native).

The content view sits below the toolbar in `window.contentView`.
On push/pop we swap subviews; the AppKit drawing pipeline handles
the redraw.

### Tab navigator → NSToolbar segmented OR NSTabViewController

Two viable approaches; pick one default and let the host choose:

1. **NSToolbar with NSSegmentedControl** (preferred default — looks
   native on Mail, Notes, App Store, modern Apple apps): the
   segmented control lives in the toolbar; tapping a segment swaps
   the content view. Subordinate stack navigation per tab works.

2. **NSTabViewController** (legacy-but-built-in): system tab UI
   above the content area. Looks more "System Preferences classic"
   and less modern.

Default: option 1. Option 2 available as a `placement: .system`
hint analogous to the existing `TabPlacement` enum.

### Drawer navigator → NSSplitView w/ NSSourceList sidebar

The single most "redesigned" piece. Instead of a swipeable scrim:

- The window's content view is an `NSSplitView` (horizontal split,
  `dividerStyle = .thin`).
- The leading pane (collapsible) is the drawer content. If the
  drawer content matches the source-list idiom (a vertical list of
  links), it's rendered inside an `NSOutlineView` styled as
  `NSTableView.Style.sourceList`. If the content is more freeform
  (custom views), it's a plain NSView inside an `NSScrollView`.
- The trailing pane is the navigator's stack content (same shape
  as the stack navigator).
- Toolbar gets a "toggle sidebar" item (`NSImage(systemSymbolName:
  "sidebar.left")`) that calls
  `splitViewController.toggleSidebar(_:)`.
- There is **no scrim** because the sidebar isn't modal. It's
  always a real second column. Mobile's "pinned vs modal" knob
  collapses to "shown vs collapsed".

This breaks the `DrawerType { Modal, Pinned, Responsive }` 3-way
choice for macOS, but in a desirable way: macOS users expect the
sidebar to live alongside content, not float over it. The framework's
`DrawerType` enum on macOS resolves to NSSplitView-with-collapse
regardless of which variant the author chose.

> Possible future enhancement: detect "small window" (width below
> some threshold) and fall back to a hidden-by-default sidebar that
> slides out as an overlay. Out of scope for v1.

---

## Portal & overlay mapping

`Primitive::Portal` exposes two shapes, both already lowered by
`framework-core`:

- **Anchored** (`AnchorTarget::Element { handle, side, align }`):
  pin a popup next to an element. **Map to NSPopover.** This is
  exactly what NSPopover does natively. We hand AppKit the anchor
  view + `preferredEdge` derived from `ElementSide` and
  `positioningRect` derived from `ElementAlign`. AppKit handles
  re-anchoring on window resize, scroll, etc.
- **Viewport** (`PortalTarget::Viewport(ViewportPlacement)`):
  full-screen overlay. **Map to a top-of-content NSView added to
  the window's content view at the highest z-order.** For true
  modality (block input below), we *could* use `NSWindow.beginSheet:`
  — but the user noted modals aren't a primitive anymore; the
  framework's overlay primitive doesn't ask for OS-level modality.
  An overlay NSView with `wantsLayer = true` and a click-eating
  background view is sufficient.

Overlay's `BackdropMode::DismissOnClick` is implemented by a
backdrop NSView underneath the content that consumes click events
and calls the overlay's dismiss handler.

---

## Cursor, hover, focus

- **Pressables / Links / Buttons**: install an `NSTrackingArea` on
  the view at construction time (or whenever the view's bounds
  change — implement `updateTrackingAreas`). `mouseEntered` →
  `NSCursor.pointingHand.push()`; `mouseExited` → `NSCursor.pop()`.
  Tooling for this lives in `imp/cursor.rs`.
- **Focus**: AppKit responder chain. NSView's `acceptsFirstResponder`
  + `becomeFirstResponder` + `resignFirstResponder` give us the
  hooks for `:focus` styling. Reactive `state` bits already have a
  Focused flag — wire it the same way iOS wires the equivalent.
  Tab / Shift-Tab navigation between focusable views works via
  AppKit's default `keyView` chain — we don't override
  `nextKeyView` / `previousKeyView` in v1; AppKit's heuristic
  (geometric order within the window) is good enough for the
  shapes the welcome example exercises. Revisit if author code
  hits a case where the order is wrong.
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
  a theme-changed callback into framework-core's appearance hook.
- Vibrancy: opt-in only. The sidebar in the NSSplitView gets an
  NSVisualEffectView background by default
  (`material = .sidebar`); regular content panes don't. Authors can
  opt into vibrancy on any container via a future style prop
  (`.material(.sidebar | .menu | .popover | .titlebar | ...)`) —
  noted as a follow-up.

---

## Animation, scheduler, render loop

These three pieces map 1:1 to iOS — the scheduler in particular is
**the same code** (NSTimer + main DispatchQueue), which is why it
moves into `apple-core`. The animation backend (CALayer +
CABasicAnimation / CAKeyframeAnimation) is also shared: NSView's
backing CALayer is the same Core Animation API as UIView's. The
existing `animated.rs` shape in `backend-ios-mobile` ports over
near-1:1 — the only differences are which property setters route
to CALayer transforms vs. AppKit view properties.

Render loop: `CVDisplayLink` is the macOS analogue of
`CADisplayLink`. macOS 14+ adds `NSScreen.displayLink(target:selector:)`
which is simpler. For v1 we can ship the NSTimer-based driver
(matches iOS's current shape and is shared in `apple-core`); the
CVDisplayLink upgrade is a perf follow-up after the framework
shape is locked in.

---

## Implementation order (phased)

Each phase is a working artifact — no half-mounted plumbing.

### Phase 0 — `apple-core` lift

Move font registry, color → CGColor, NSLog, scheduler from
`backend-ios-core` into a new `backend-apple-core`. `ios-core`
becomes a thin UIKit-flavored re-export. Verifies the refactor
doesn't break iOS before any macOS code lands.

**Done when:** iOS welcome example still runs, no behavior change.

### Phase 1 — minimal macOS render path (no chrome)

- Crate skeleton with `MacosBackend`, `MacosNode`, `FlippedView`.
- `create_view`, `create_text`, `create_button`, `create_image`,
  `create_scroll_view`, `create_text_input`, `create_text_area`,
  `create_toggle`, `create_slider`, `create_activity_indicator`,
  `create_icon`, `create_link`, `create_pressable`, `create_graphics`.
- `insert`, `clear_children`, `apply_style`, `finish`.
- Taffy layout pass + frame application + NSScrollView contentSize
  sync.
- Intrinsic-size measurers for label / button / switch / slider /
  image / textfield / textview.
- Theme + asset + typeface registry hooks (delegating to apple-core).
- `host-appkit` crate booting NSApplication + a single NSWindow,
  installing the backend, running the welcome example **without**
  any navigator chrome (top-level content only).

**Done when:** the welcome example renders, all visible primitives
display correctly, hover + cursor work on pressables.

### Phase 2 — navigators

- Stack: NSToolbar back chevron, title item, header buttons,
  content swap. `apply_header_options` AppKit analogue.
- Drawer: NSSplitView with collapsible sidebar, toolbar toggle item.
- Tab: NSToolbar NSSegmentedControl variant.
- `attach_navigator_layout` + the per-screen scope keepalive
  pattern documented at [[feedback_navigator_scope_keepalive]].

**Done when:** an example with all three navigator shapes works.
Toolbar buttons (sidebar toggle, back chevron) respond to clicks;
no custom keyboard shortcuts wired in v1 (see resolved scope §3).

### Phase 3 — overlays + portals

- `create_portal` for both anchored (NSPopover) and viewport
  (overlay NSView) shapes.
- `apply_presence` for enter/exit animations on overlay subjects.

**Done when:** an example using `anchored_overlay()` next to a
toolbar item shows an NSPopover; `overlay()` with `DismissOnClick`
backdrop dismisses on click outside content.

### Phase 4 — animation + render loop polish

- CALayer-driven `set_animated_f32` / `set_animated_color` via
  the iOS `animated.rs` shape.
- CVDisplayLink-based render loop (replace NSTimer fallback).
- `install_render_loop()` analogue under `async-driver` feature.

**Done when:** the animation-test example runs at 60 fps on a
Retina display with `debug-stats` confirming the per-frame phase
budget is in line with the iOS / web baselines.

### Phase 5 — polish + system integration

- App menu bar setup in `host-appkit` with **AppKit defaults only**
  (File / Edit / View / Window / Help). System shortcuts that
  ship for free — Cmd-Q, Cmd-W, Cmd-H, Cmd-M, Cmd-X/C/V/A in text
  contexts — work via NSResponder's default routing. No
  framework-author-defined shortcuts (see resolved scope §3).
- NSAppearance KVO for live light/dark switching.
- NSVisualEffectView opt-in via a `.material(...)` style prop.

**Done when:** the app feels like a native macOS app — menu bar
present with system defaults, system text-editing shortcuts work
inside focused inputs, dark mode flips live.

---

## Resolved scope decisions

These were open questions at draft time. Recording the answer + the
reasoning so a future contributor doesn't relitigate them.

1. **NSWindow ownership — host owns, injects content view.**
   `host-appkit` creates the NSWindow and content view, then calls
   `backend.set_host_root(content_view)`. Mirrors iOS, where the
   Swift host owns UIWindow and hands the backend a UIView. Keeps
   the backend free of NSApplication boot concerns.

2. **Per-screen vs shared NSToolbar — defer.** Default to a single
   shared NSToolbar on the host window that the backend re-skins
   on each navigation (cleanest AppKit fit, no flicker on swap).
   Revisit if/when per-screen toolbar state becomes a real
   constraint.

3. **Keyboard shortcuts — defer entirely.** No `on_key_down`
   bubbling, no `useKeyboardShortcut()` primitive, no menu-bar
   shortcut binding in v1. The framework is mobile-first; phones
   don't have a keyboard shortcut model. macOS authors who need
   shortcuts can reach for AppKit directly via `Primitive::External`
   until there's a cross-platform shape worth designing. **A
   placeholder app menu bar (File / Edit / View / Window / Help
   with system items only) still ships in `host-appkit` Phase 5
   so the app feels native** — no author-defined shortcuts wired
   through it, just the AppKit defaults (Cmd-Q, Cmd-W, Cmd-H,
   Cmd-M, etc.).

4. **Multi-window — out of scope.** Single window in v1. The
   framework's design philosophy is mobile-first; two-screen
   workflows aren't a standard mobile pattern. If multi-window
   matters later, the most likely shape is a third-party
   extension (a `Primitive::External` that opens an auxiliary
   NSWindow) rather than a core-trait expansion — keeps
   mobile-first backends from having to ignore a method that
   doesn't apply to them.

5. **Catalyst interop — not pursued.** AppKit only. The
   `apple-core` lift incidentally leaves shared CoreText / color /
   scheduler bits in a reusable spot if a Catalyst experiment
   ever happens, but no work targets Catalyst.

## Genuinely open / TBD

These are decisions inside Phase 1 implementation work, not scope
boundaries — flagging so they don't get lost:

- **`window.toolbar` style** when `ScreenOptions.header_shown ==
  false`. Options: `window.toolbar = nil` (cleanest, but causes
  the title-bar height to recompute → tiny content jump) versus
  `window.toolbarStyle = .preference` with empty items (stable
  height, slightly more code). Pick during Phase 2.
- **Whether the `apple-core` lift renames `backend-ios-core` or
  keeps it as a re-export shim.** Renaming is the cleaner end
  state but requires touching every iOS dependent. A re-export
  shim lets Phase 0 land without iOS churn, with the rename
  happening as a follow-up.
- **NSPopover vs custom overlay for anchored portals where the
  anchor scrolls off-screen.** NSPopover's behavior on scroll-out
  is to close itself; framework callers might expect "follow
  anchor and clip." Decide after the welcome example exercises
  the path.

---

## Non-goals (explicit, so they don't sneak in)

- No UIKit-on-Mac (Catalyst). AppKit only.
- No SwiftUI bridge. AppKit only.
- No SwiftUI lookalike paint pipeline. Native AppKit widgets only.
- No reimplementation of NSSplitView / NSToolbar / NSPopover in
  Taffy. We use what AppKit gives us.
- No "phone simulator on Mac" (`native-phone`/`native-tablet`
  variants already do that via wgpu — different code path entirely).

---

## Related memory entries

- [[project_third_party_extension]] — `Primitive::External` shape
  the macOS backend will mirror.
- [[project_ios_intrinsic_size_measurer]] — same problem applies
  to AppKit widgets.
- [[project_ios_scrollview_bounds_origin]] — NSScrollView has its
  own scroll-position semantics (`documentVisibleRect`); apply-
  frames must preserve it.
- [[project_install_theme_required]] — same render-time contract
  on macOS.
- [[feedback_navigator_scope_keepalive]] — sidebar / toolbar
  reactive scopes need the same keepalive Effect pattern.
- [[feedback_no_idealyst_prefix]] — crate names are
  `backend-macos`, `backend-apple-core`, `host-appkit`. No prefix.
- [[wgpu_aspect_lock]] — `host-appkit`'s NSWindow setup will
  intersect this code in the multi-window phase; for v1 the
  AppKit-native window resize doesn't need aspect lock.
