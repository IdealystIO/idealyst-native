# TESTING.md — what to watch for when testing idea-ui

This is a checklist for an LLM agent (or human) verifying idea-ui components across
backends. idea-ui is **one author tree, every backend** — the whole point is that a
component looks and behaves the same on web, macOS, iOS, and Android even though the
toolkits underneath (DOM, AppKit, UIKit, Android Views) are completely different.

So the bar for "it works" is **not** "it renders on the platform I happened to run."
The bar is **parity**: same layout, same interaction, same animation, on every backend
it claims to support. Most of the bugs this project has shipped and then had to fix were
"works on web, silently broken on a native backend" — those are the ones this file exists
to catch.

> **Golden rule (CLAUDE.md §7):** the backend decides *how* something renders; it must not
> change *what* the user observes. If you find yourself reaching for `if platform == X` in
> framework/backend code to make a component look right, the backend is wrong at its root —
> fix it there, don't patch the call site. A per-platform fudge factor ("subtract 2px on iOS
> only") is a smell, not a fix.

---

## 0. Before you trust a "pass"

1. **Test on more than one backend.** A green `cargo test` covers logic, not rendering. If
   you changed anything visual or interactive, exercise it on at least web **and** one Apple
   backend; ideally Android too, since Android has the most divergent quirks (see §4).
2. **Run it the way the user runs it.** `idealyst dev --web` / `--ios` / `--macos`. For
   native, `--local` bypasses the wire (runtime-server) and is the fallback when the
   wire build is broken; the **default** mode is the only one that exercises the wire path.
   Know which one you're in — a bug that only reproduces over the wire won't show under
   `--local`, and vice versa.
3. **First paint counts.** Several bugs here were "renders correctly after a resize / a
   tap / a navigation, but the *first* frame is wrong." Don't resize the window to make it
   look right and then call it fixed — watch the very first paint.
4. **Every bug fix lands with a regression test (CLAUDE.md §8).** If you fixed something
   here, add the test that fails before and passes after, named after the bug.

---

## 1. Layout & sizing parity

- **Text wrapping / overflow.** The runtime seeds `max-width: 100%` on every node for
  web-parity wrapping. If a wide child (a long codeblock, a fixed-width image) blows out
  its flex ancestors and the screen overflows until you resize, suspect a min-content
  measurement reporting the content's intrinsic width instead of `0`. This bit external
  scrollers on macOS/iOS (not web). Test with content **wider than the viewport**.
- **Navigators fill their parent.** A navigator (Drawer/Stack) as a non-root flex child
  must still fill — it defaults to `flex_grow:1` + `100%`. A blank screen on iOS/Android
  where web shows the navigator is the classic symptom.
- **Conditional children don't shift siblings.** `if cond { X }` with an empty branch is
  `position: absolute`, so toggling it must **not** nudge siblings via flex-gap. Test
  Tooltip/Popover/Modal triggers: open and close, watch the trigger row — it must not jump.
- **`when` / dynamic insert.** Inserting a child after first paint, or `Element::When`
  toggling absolute content, must trigger a layout pass on **every** native backend.
  Android and iOS needed explicit child-splice opt-in here; web was always fine. Test:
  mount, then add/remove a child via a signal, on native.
- **Drawer sidebar is a plain full-height view**, not a scroll_view — its background must
  span the whole panel and the author opts into scrolling. Check the sidebar bg doesn't
  stop short of the panel bottom.

---

## 2. Interaction & events

- **Pressables inside scroll views (iOS/UIKit).** UIScrollView eats the first touch unless
  `delaysContentTouches = NO`. Symptom: buttons/`on_touch` content inside a modal or scroll
  view feel dead or need a second tap on iOS. Always test "tap a button that lives inside a
  scrollable area" on a real iOS surface.
- **Non-modal overlays must pass touches through (Android).** A `trap_focus=false` overlay
  (ToastHost, transient banners) that doesn't set `NOT_FOCUSABLE | NOT_TOUCHABLE` blocks
  **all** app touches behind it. This is the recurring "Android hamburger / whole screen is
  dead" bug. Test: show a toast, then try to tap anything underneath it on Android.
- **Optional callbacks: bind only when present (§9.6).** A component with an
  `Option<Rc<dyn Fn()>>` prop must conditionally attach the handler, never wire an
  unconditional no-op closure. A silent no-op handler blocks hit-test fall-through on some
  backends. Test: a component with `on_press = None` must let taps fall through to whatever
  is behind/around it.
- **Hover is desktop-only.** `view.on_hover` fires on web (pointerenter/leave) and macOS
  (NSTrackingArea); it's a **no-op on iOS/Android**. Anything that *only* reveals via hover
  (a bare Tooltip) must have a touch path (long-press) on mobile, or it's unreachable there.
- **Focus state after native back-navigation.** Native stack Pop doesn't update
  `active_route`, so `use_focus()` reads stale after a back gesture. Components that gate on
  "am I the focused/root screen" should derive from `use_can_go_back()` (depth-based),
  not `use_focus()`. Test: push a screen, go back, check focus-dependent UI is correct.

---

## 3. Styling, theming & state machines

- **Static-styled nodes still need interactive states.** A node styled with a *static*
  stylesheet that also declares `state hovered`/`pressed`/`focused` overlays must still get
  hover/press/focus on native — web gets it free via CSS, native does not unless the node is
  routed to reactive style attachment. Symptom: idea-ui MenuItem/ListItem with a static
  `active` style has no hover/press feedback on macOS/iOS while web does. Test every
  interactive component's pressed/hover/focus visuals **on native**, not just web.
- **Reactive style re-apply must invalidate on Android.** A reactive background
  (GradientDrawable) or text style that changes via signal froze on Android without an
  explicit `invalidate()` / mark-dirty. And a reactive style passed as `Rc<StyleSheet>` is
  treated as **static** (cached, goes stale) — use the **closure** form for reactive styles.
  Test: change a component's style via a signal on Android and confirm it actually repaints.
- **Color fades / transitions.** `transitions { background }` interpolation must use
  premultiplied alpha. A transparent→light fade that flashes dark gray mid-tween means the
  backend lerped straight RGBA (transparent stored as `[0,0,0,0]` drags toward black). Test
  any component that fades from transparent to a light color.
- **Corner radius in detached/portal layout (macOS).** Modals/Popovers go through a separate
  detached layout pass; if that pass skips the post-frame sync, you get square corners on
  buttons inside a modal while the same button is rounded inline. Test rounded components
  **inside** a modal/popover on macOS.
- **`color_scheme()` / dark mode.** Install the theme matching the platform's
  light/dark default at mount to avoid a flash of the wrong theme. Test first paint in
  both light and dark OS settings.

---

## 4. Android-specific traps (the highest-divergence backend)

Android has bitten this project the most. When a component "works everywhere but Android":

- **TLS key exhaustion.** Bionic caps pthread TLS keys at 128 and each `thread_local!`
  burns one. A per-stylesheet `thread_local!` crashed at mount (SIGABRT). Stylesheets must
  go through the one shared cached-stylesheet registry. If a component crashes *on mount*
  on Android with no obvious cause, count your thread_locals.
- **Stale generational signal sets are safe no-ops** — a deferred `set` on a recycled
  handle must not panic→SIGABRT. If Android crashes on a delayed signal update, this is why.
- **Net/JNI pending exceptions** must be cleared before worker detach or a failed server-fn
  crashes the bg thread (FATAL) instead of returning `Err`.
- **Keyboard/IME reflow.** Closing the soft keyboard must restore the viewport (root layout
  listener → layout pass). Test: focus a text input, type, dismiss the keyboard, confirm the
  layout returns to full height.
- **Some example apps are pre-broken on `--local` Android** (navigator apps blank under a
  non-FragmentActivity, etc.). Don't attribute a pre-existing example breakage to your
  change — verify against a known-good vehicle (canvas-demo) first.

---

## 5. iOS-specific traps

- **Simulator ≠ device for GPU.** The iOS Simulator's Metal lacks `INDIRECT_EXECUTION`, so
  vello-backed canvas falls back to canvas-native there; real devices run vello. A
  canvas/graphics component that looks right in the simulator may differ on device and vice
  versa. Note which one you tested.
- **`interactivePopGesture` is controller-global** — back-gesture locks must re-sync per top
  controller. Test back-swipe lock by pushing several screens.
- **Keyboard inset** is implemented via keyboard notifications but has been
  **device-unverified** in places — verify text-entry components on a real iPhone, not just
  the sim.
- **First-paint buffering.** Chrome built in a deferred microtask paints a frame late.
  Watch the very first frame of headers/sidebars.

---

## 6. macOS-specific traps

- **Anchored portals** (tooltips/popovers) must position window-relative with an anchor
  tracker, or they render at the window top-left. Open a tooltip/popover anchored to a
  button that is *not* at the origin and confirm it appears next to the trigger.
- **`text_area` autosize** must measure via the layout manager's used rect, not
  `intrinsicContentSize`, or it won't shrink when you delete lines. Test growing **and
  shrinking** a multi-line input.
- **`clear_children` must schedule a layout pass** or a popover won't refit after its
  content changes.
- **Icons** are painted at a fixed 24px and scaled to bounds in the layout pass — a boxy or
  oversized icon means that scale step was skipped.

---

## 7. Component-level smoke checklist

For any component you touch, walk this list across web + at least one native backend:

- [ ] Renders correctly on **first paint** (no resize/tap needed to fix it).
- [ ] Pressed / hover / focus visuals fire **on native**, not just web (§3).
- [ ] Disabled / `on_press = None` lets taps fall through (§2).
- [ ] Inside a **modal/popover**: corners, padding, and pressability survive (§3, §2).
- [ ] Inside a **scroll view**: pressables still tap on iOS (§2).
- [ ] Toggling it via a signal repaints on **Android** (§3).
- [ ] Long text **wraps** and doesn't overflow the viewport (§1).
- [ ] Both **light and dark** themes look right from first paint (§3).
- [ ] If it animates/fades: no dark flash, same timing on every backend (§3, §7-of-CLAUDE).
- [ ] A **regression test** exists for any bug you just fixed (§0).

---

## 8. Tooling for verification

- **Unit / logic:** `cargo test -p idea-ui` (and the framework suites for core changes).
- **Robot tests:** `#[robot_test]` fns drive a relay-connected `App` (locate→act→assert)
  and expand to real `#[test]`s; `idealyst test --<platform>` preps the env. Use these for
  cross-platform interaction assertions.
- **Screenshots:** the `screenshot` robot verb captures the real surface (native) or a DOM
  capture (web) — use it to diff parity between backends.
- **Native introspection / parity diffing:** `introspect_native` reads each primitive's
  *platform-resolved* geometry and props from the live native object (CALayer /
  getComputedStyle), not from author StyleRules. This is the strongest tool for catching
  "web and macOS disagree" — capture both and diff. Web + macOS are full; others are stubs.
- **Recipes:** `recipe!(Component, fn ...)` examples are compile-checked usage; they break
  the build if a prop changes, so they double as a guard that a component's public surface
  still works.

When in doubt: if you can't demonstrate parity with a screenshot, an introspection diff, or
a robot assertion, you haven't verified it — you've assumed it.
