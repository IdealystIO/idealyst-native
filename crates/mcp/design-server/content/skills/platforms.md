+++
title = "Platform conventions and quirks"
summary = "What differs between iOS, Android, web and desktop that a design must account for — back navigation, hover, touch targets, safe areas, the keyboard, text inheritance, portals — and the framework seams to handle each without per-platform hacks."
when = "Designing navigation, overlays, inputs or anything touch-vs-pointer; or a screen looks/behaves differently on one platform."
order = 70
tags = ["platforms", "ios", "android", "web", "desktop", "quirks", "touch", "safe-area", "keyboard"]
+++
# Platform conventions and quirks

One author tree runs everywhere, and the framework makes the output match.
But platforms have conventions users expect, and a few real differences a
design has to plan for. This skill is the list, with the sanctioned way to
handle each.

**The rule:** adapt through the framework's seams — breakpoints,
`primary_pointer()`, `safe_area`, `keyboard_avoiding_view`, `platform()` —
never with a per-platform constant ("+4px on Android"). If something only
looks wrong on one backend and no seam covers it, it's a framework bug:
report it, don't patch around it in the app.

## The seams

| Need | API (`runtime_core::…`) | Notes |
| --- | --- | --- |
| Layout by size | `current_breakpoint()`, `breakpoint` blocks | The default. See read_design_skill("adaptive"). |
| Touch vs mouse | `primary_pointer() -> ReadSignal<PointerKind>` (`Fine` / `Coarse` / `None`) | The CSS `(pointer:)` analogue. Use it to densify for a mouse, never to remove touch affordances. |
| Notch, home indicator | `view(safe_area = SafeAreaSides::TOP)` (also `BOTTOM`, `HORIZONTAL`, `ALL`) | Re-applies as insets change. Zero on web/desktop, so it's safe to always declare. |
| On-screen keyboard | `keyboard_avoiding_view(…)` around the root; `keyboard_inset()` | Off by default — nothing avoids the keyboard unless you opt in. |
| Platform identity | `platform() -> Platform` (`is_mobile()`, `is_apple()`, `is_web()`, …) | For copy and shortcuts ("⌘K" vs "Ctrl K"), not layout. Linux/Windows/terminal report `Platform::Custom(..)`; `is_desktop()` is true only on macOS — branch on pointer/breakpoint for "desktop-like". |
| Light/dark | `install_idea_theme_schemes(light, dark)`; `set_idea_color_scheme` | The OS preference is read once at startup; offer an in-app toggle. Linux/Windows report `Auto` (light). |
| Reduced motion | `reduce_motion()` | Shorten or drop decorative motion when set. |

## Navigation

- **The stack header is drawn on every platform.** iOS and Android don't
  show a native navigation bar — `StackHeader` is the bar everywhere. It
  applies no safe-area inset: put it in a slot with
  `safe_area = SafeAreaSides::TOP`, and a bottom `TabBar` in one with
  `SafeAreaSides::BOTTOM`.
- **Back differs.** iOS: a real navigation controller with interactive
  swipe-back. Android: system Back pops while more than one screen is
  stacked; at a stack's root (and in a swap navigator) it leaves the app.
  Web: browser history. macOS/Linux/Windows/terminal: no gesture at all —
  the drawn back button is the only way back, so **never hide the back
  affordance on desktop**.
- `Screen::back_enabled(false)` blocks the gesture/system Back on iOS and
  Android (for an unsaved form) — pair it with a visible confirm-to-leave.
- **Covered screens**: on web a push disposes the covered screen and Back
  rebuilds it from its URL; elsewhere it stays mounted. Keep state that
  must survive a round trip (scroll position aside) in the URL
  (`screen_state`) or above the navigator — not in the covered screen's
  locals.

## Touch vs pointer

- **No hover on touch.** iOS and Android never set the `hovered` state
  (not even iPad with a trackpad). Nothing may be reachable *only* on
  hover: row actions that appear on hover also need a visible affordance
  (an overflow `IconButton`) on touch. `Tooltip` falls back to long-press;
  never put essential information only in a tooltip.
- **Touch targets ≥ 44pt.** `IconButton` is 24/32/48 (`size` Sm/Md/Lg); in
  toolbars and lists that a finger hits, use `IconButtonSize::Lg` on
  `Coarse` pointers or pad the slot. `Field` (48) and `Tabs` (40, in a
  padded bar) are fine.
- **Cursor**: idea-ui's interactive components set a pointer cursor. A
  raw `pressable` you style yourself must declare
  `cursor: Cursor::Pointer` in its sheet, or desktop users get an arrow.

## Keyboard

- Wrap the app root in `keyboard_avoiding_view` when any screen has inputs
  near the bottom (chat composers, sign-in on a phone). `Modal` already
  avoids the keyboard for its own card.
- On iOS/Android, app-level key handlers see only hardware keyboards, and
  nothing while a text field has focus — keyboard shortcuts are a
  desktop/web enhancement, never the only path.

## Rendering differences to design around

- **Text color does not inherit on most native backends** (iOS, Android,
  macOS, Windows). `view(color = white) { text(…) }` is white on web and
  Linux and theme-colored elsewhere. Put color on the text itself —
  `Typography(tone = …)` or a text style — never on a container.
- **Portals escape inheritance.** `Select`, `Menu`, `Popover`, `Modal`
  content mounts outside your tree on web; an app font/color set on a
  container doesn't reach it. Brand those through the theme
  (`brand.rs`), which does.
- **Overlap needs `z_index`.** Web paints positioned elements above their
  unpositioned siblings; native paints in child order. Anything that
  overlaps (a floating action, a badge on an avatar) sets `z_index`.
- **Scroll content shrinks by default.** Inside a `scroll_view`, give the
  content `flex_shrink: 0`; horizontal strips also hide the scrollbar
  (`scrollbar: ScrollbarVisibility::Hidden`) and set
  `overscroll_behavior: Contain` so a swipe doesn't trigger browser Back.
- **Children outside their parent's bounds are visible but untappable on
  iOS/Android.** Don't let interactive content overflow its container.
- **Fonts**: ship every weight and italic the brand uses. A missing bold
  is synthesized on web, falls back to Regular on iOS and is faked on
  Android — three different looks.
- **Images**: use absolute URLs, `asset!` files, or root-relative `/paths`
  (joined to the `server::configure` base; without one they load only in a
  browser). Document-relative paths (`logo.png`) are rejected everywhere. GIFs show their first frame. SVG `<text>` doesn't
  draw natively — outline it.
- **Shadows**: a shadow on a box with `overflow: hidden` and a translucent
  background doesn't render on iOS/macOS — keep shadowed cards opaque
  (idea-ui's are).
- **Pill shapes**: `radius-pill` is exact on web and Linux; elsewhere it's
  a large fixed radius, fine for anything under ~2000px tall.

## Current framework limitations (work around, and report)

- `pointer_events: None` is not implemented on Android or the GPU backend —
  an invisible overlay can still catch taps there. Unmount overlays when
  inactive rather than hiding them with `pointer_events`.
- `StackHeader` draws a `HeaderButton::icon(…)` as its *name* in text. Use
  `HeaderButton::text("Edit")` for header actions.
- `maps` renders only on web and iOS; elsewhere a placeholder. Design a
  fallback (address text + "Open in Maps" link).
- `focus()` / `autofocus` do nothing on the GPU and terminal backends —
  never rely on autofocus for a flow to be usable.
