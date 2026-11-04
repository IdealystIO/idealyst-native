# The robot bridge

The robot is the framework's built-in automation surface: an E2E test, the
MCP server or the Inspector finds elements in a running app, acts on them and
reads them back — with the same verbs on every backend. It is compiled into
dev builds only (`robot` features); release builds carry none of it.

The goal is Playwright-style tests that run unchanged on web, iOS, Android,
macOS, Linux and Windows: one suite, every platform, and any difference in
the results is a real difference between the backends. A behavior a test
cannot drive or observe through the robot is a gap in the robot's surface,
and is treated as one.

## Where it lives

- `crates/runtime/vocabulary/src/robot.rs` — the element registry (every
  builtin primitive registers on mount and deregisters on teardown), the
  [`Robot`] API, and `bridge::invoke_command`, the verb dispatcher.
- `robot_methods.rs` (components and `#[method]`s), `robot_watch.rs`
  (`watch_signal`), `robot_props.rs`, `robot_ext.rs` (SDK nodes),
  `robot_fn.rs` (functions a test calls by name, `#[robot_fn]`).
- `crates/runtime/shared/src/robot/` — the transport (TCP bridge, relay
  client) and the registry-independent verbs (`get_logs`, `screenshot`, …).
- Clients: `crates/dev/robot-test` (`#[robot_test]` host-side tests),
  `crates/dev/robot-e2e` (in-app suites, which drive `Robot` directly: their
  `click` is the robot's `click`, with no native tier), the MCP server.

## In the app

What an app does to be testable compiles in every build, so it needs no
`cfg`: `test_id = "…"` on an element, `robot::watch_signal("name", sig)`
(and `watch_signal_writable`), `robot_fn::register(f::robot())`, an SDK's
`register_robot_controls()`, and `robot::is_test_driven()`. Outside a robot
build each is inert — `is_test_driven()` is `false`. An app can't gate them
itself: `idealyst dev` and `idealyst test` turn on the framework's `robot`
features, never the app crate's own, so an app-side
`#[cfg(feature = "robot")]` is off in dev builds too.

The same holds for component libraries: idea-ui forwards every component's
`test_id` prop in every build and leaves registration to the runtime, so
its ids work in dev builds without its own `robot` feature (that feature
only turns on `runtime-core/robot`). A component puts the id on the primitive a
test acts on — the text input, the pressable, a picker's trigger — never on
a layout wrapper, and a component that rebuilds that primitive (a `Button`
with a live `loading`) re-registers the id on each rebuild; the registry's
remove only un-indexes an id it still owns, so one live element holds it.
Coverage list: the testing guide (`crates/mcp/catalog/guides/testing.md`,
§1).

## Elements

`find_element` / `find_all_elements` / `count_elements` take a query
object; every field given must match (AND), and with none every element
matches:

- `test_id` — exact; `kind` — the `ElementKind` name (an unknown kind is an
  error); `label` — exact; `label_contains` — a substring, or an array of
  substrings that must all appear.
- `within` — only descendants of a container, never the container itself:
  an element id, or a nested query object whose own optional `nth` picks
  one of the containers it matches (document order). `{"kind": "TextInput",
  "label": "Email", "within": {"test_id": "login"}}` is the field in the
  login form — not its "Email" caption, not a field elsewhere.

`count_elements` returns the number of matches; `find_all_elements` a list
of elements in document order, and `find_element` the first (a lone
`test_id` resolves through the last-wins index), shaped

```json
{"id": 7, "kind": "Toggle", "test_id": "wifi", "label": null,
 "visible": true, "in_viewport": true,
 "visible_frame": {"x": 16, "y": 120, "width": 51, "height": 31},
 "state": {"checked": true, "disabled": false}}
```

`visible` and `in_viewport` are on every element, `visible_frame` on every
element whose frame is known; `state` on elements that have any (below).
`get_snapshot` nodes carry the same fields.

### Visibility

`visible` means a user could see the element: the toolkit shows it, its
laid-out box is not empty, and no ancestor that clips without scrolling
(`overflow: hidden`) cuts that box away entirely — text inside a drawer
collapsed to 0 pt is not visible. "Shown" is the backend's own answer
(`IntrospectionOps::is_shown`): in a window, not hidden by itself or an
ancestor, not fully transparent (opacity 0 on it or any ancestor).
Content a scroller or the window cuts off (scrolled away, beyond the
window) is still visible, as in Playwright — scrolling reveals it.

`visible_frame` is the part of the element's box left on screen: its
window frame cut by the window and by every registered ancestor that clips
its content — `overflow: hidden`, a `scroll_view`, a list (a virtualized
row is cut by its list). Zero-sized when nothing is left or the element is
not visible. `in_viewport` is whether it is non-empty (absent when the
frame or the window size is unknown).

How an ancestor clips is the backend's read of the live object
(`IntrospectionOps::content_clip`), computed once in the vocabulary:

| Backend | Scrolls | Cuts (`overflow: hidden`) |
|---|---|---|
| Web | computed `overflow-x`/`-y` `auto` or `scroll` | `hidden` or `clip` |
| macOS | an `NSScrollView` | the layer's `masksToBounds` |
| iOS | a `UIScrollView` (`scroll_view`, list, `text_area`) | `clipsToBounds` |
| Android | `canScrollVertically`/`Horizontally` either way | a `ViewGroup` with `getClipChildren()` (Android's default) |
| Linux | a `GtkScrolledWindow` | an `IdealystView` whose snapshot clips (`overflow: hidden` or a radius) |
| Windows | a view with a scroll | `overflow: hidden` — the painters' clip |
| Terminal | not yet: nothing counts as clipping |  |

| Backend | "Shown" means |
|---|---|
| Web | `isConnected` and `checkVisibility({opacityProperty, visibilityProperty})` |
| macOS | in a window, not `isHiddenOrHasHiddenAncestor`, `alphaValue` × layer opacity up the chain above ~0 |
| iOS | in a window, no `isHidden` up the chain, `alpha` × layer opacity above ~0 |
| Android | `isShown` (attached, `VISIBLE` up the chain), `getAlpha` up the chain above ~0 |
| Linux | `is_drawable` (visible and mapped), `opacity` up the parent chain above ~0 |
| Windows | reachable from a root through no hidden node, opacity product above ~0 — the painters' walk |
| Terminal | not yet: the robot falls back on the box alone |

An element with no answer and no frame (a headless host) is visible:
mounted is all that's known.

### State

`get_state {element_id}` returns the element's own state — what a user
would see on screen — with no help from the app:

| Field | Primitives | Meaning |
|---|---|---|
| `value` | `text_input`, `text_area` (string), `slider` (number) | The text the field shows; the slider's value, snapped to `step` |
| `checked` | `toggle`, a `pressable` with `a11y_state` or an authored checkable role | What the switch shows; for a pressable, what a screen reader is told (mixed reads as `false`) |
| `disabled` | toggle, slider, text input/area, pressable, button | Whether it refuses input |
| `focused` | `text_input`, `text_area` | Whether it holds keyboard focus, read from the toolkit at query time: web `document.activeElement`, iOS `isFirstResponder`, Android `View.isFocused()`, macOS the window's first responder (the field editor for a `text_input`), Linux the window's focus widget, Windows `GetFocus()`. A background or headless window, which gets no focus events, still reports it. A backend that can't read it falls back on the field's last focus/blur notification |
| `scroll` | `scroll_view` | `{x, y}` content offset, read from the native scroller |
| `hovered`, `pressed` | any element whose style has `state hovered` / `state pressed` variants | Whether that style state is on, as the backend set it |
| `keyboard` | `text_input` | The soft-keyboard kind the app asked for (`keyboard = KeyboardKind::…`): `"default"`, `"email"`, `"phone"`, `"number"`, `"decimal"`, `"url"` |
| `native_keyboard` | `text_input` | The kind the native widget holds, read back from it: web `inputmode`, iOS `keyboardType`, Android `inputType` (a masked type several kinds share reads as the first, the keyboard actually shown). Absent on desktop backends, which have no soft keyboard |

It is the primitive's view, not the author's: an uncontrolled toggle reports
what it was last flipped to, a controlled one the author's value after each
write-back. Fields a primitive doesn't have are left out; a view, text or
image reports `{}`.

`value` is the robot's own record of the field: it follows both the user's
edits and the bound value, so it reads the new text even on a backend that
never pushed a value written from code into the widget. To check what the
widget itself shows, read `introspect_native` (robot-e2e:
`expect(…).to_show_native_text(…)`).

`hovered` / `pressed` come from the style's state machine, so they are
absent on web (CSS `:hover` / `:active` own those states there, and the
page can't read them) and on elements whose style has no state variants.

## Actions behave like a user

`click`, `type_text`, `set_toggle`, `set_slider`, `focus`, `blur`,
`set_scroll` and `drag` act on a control the way a person would:

- **A disabled control refuses.** The action fails with
  `action 'set_toggle' refused: the element is disabled`, and nothing
  reaches the author — so a test can assert that `disabled` blocks input.
- **`focus`, `blur` and `key Tab` run the focus handlers in a background
  page.** A browser page without OS window focus moves `activeElement` and
  fires no event, so a robot build delivers `blur` / `focus` itself there, and a
  field that commits on blur commits. A robot `key Enter` reaches `on_key_down`.
- **The native widget moves first, then its own change callback reports
  it.** `set_toggle` flips the switch; `set_slider` clamps to the track,
  snaps to `step` and moves the thumb; `type_text` puts the text in the
  field. The author's `on_change` sees what a real interaction would
  produce.
- **`click`** runs a pressable/button/link's press, or delivers a synthetic
  touch (Began → Ended at the element's center) to the nearest view with an
  `on_touch` handler, bubbling to ancestors.

These actions run inside the app, through the framework: they test the
framework's logic and the backend's widget updates, not the OS's event
delivery or hit testing. For that, use the native tier below.

## Native input: `tap_native`, `drag_native`, `hover_native`, `press_native`, `context_click`, `key`

`tap_native {element_id}` taps the element's center with real platform
input, through the toolkit's own event path — hit testing, overlays,
gesture recognizers, the native disabled state and the app pointer sink
all take part. `drag_native {element_id, points}` does the same for a
path (points relative to the element's top-left, as `drag` takes them).

It refuses as a finger would fail, with a retryable error: the element is
not visible, its center is outside the window ("scroll the element into
view"), or the backend's hit test finds something else on top ("another
element covers the point … — View test_id=\"scrim\" would receive the
tap"). It answers `{mechanism}`:

| Backend | Mechanism | How |
|---|---|---|
| Web | `native` | DOM `pointerdown`/`mousedown`/…/`click` at `elementFromPoint` (moves and the up go to the down target, a touch's implicit capture); a focusable target takes focus. On a page with touch input (`navigator.maxTouchPoints > 0`, e.g. `idealyst test --device`) it is a touch: `pointerdown`+`touchstart`, `pointermove`+`touchmove`, `pointerup`+`touchend`, then the compatibility `mousedown`/`mouseup` and `click`, which a prevented `touchstart`/`touchend` suppresses |
| macOS | `native` | `NSEvent` mouse events posted to the app (`postEvent:`), so tracking loops get their mouse-up |
| Android | `native` | `MotionEvent`s to the decor view's `dispatchTouchEvent` |
| Windows | `native` | `WM_LBUTTONDOWN`/`WM_MOUSEMOVE`/`WM_LBUTTONUP` posted to the child window a real click lands on |
| iOS | `hit_test` | An app can't create touches: UIKit's hit test checks the point, then the press runs as `click` on the deepest element that hit test finds there |
| Linux | — | Not yet (XTest); `unsupported` |

The hit test (`IntrospectionOps::hit_contains`) is the toolkit's own:
`elementFromPoint`, `-[NSView hitTest:]`, `-[UIWindow hitTest:]`, the
Windows scene's hit walk. UIKit's never returns a view with
`userInteractionEnabled = NO` — every label and image — so on iOS a tap
on a text that lands on its interactive ancestor (the pressable or row
around it) reaches the text, as the event bubbles there on web; one that
lands there through an interactive view the hit test skipped
(`pointer_events: none`), or outside a view on the way (a row overhanging
its cell), does not. Android has none (Android delivers a touch to
the deepest clickable view, which geometry can't tell), so there the
injected event is the whole check.

Every tier presses what is under the point, not the element the test
addressed: `tap_native` on a wrapper view whose child is the pressable
presses the pressable, as a finger at the wrapper's center would. The real
input tiers get this from the toolkit; the `hit_test` tier presses the
deepest element in the addressed subtree whose hit test contains the
point, then bubbles up from there as `click` does.

robot-test's `Locator::click()` is native-first: it waits for the element
to be visible and enabled, scrolls it into view when it is out of it (below),
then `tap_native`, falling back to the robot's `click` only where the backend
answers `unsupported`. `tap_native()` has no fallback; `dispatch_click()` is
the robot's `click` on purpose.

### Resolved style

`get_style {element_id}` reports the element's resolved visual style:
`{source, style: {color, background, opacity, font_size, font_weight,
font_style, font_family, text_decoration, text_align, border_width,
border_color, corner_radius}}`. Colors are sRGB `[r, g, b, a]` in `0..=1`,
lengths logical px, `font_weight` the CSS number, `text_decoration` one of
`"none"`, `"underline"`, `"line-through"`, `"underline line-through"`.

`source` says where it was read. `"native"`: the toolkit resolved it
(`IntrospectionOps::computed_style` — the web's `getComputedStyle`, where
CSS decides state pseudo-classes, `@media`, inheritance and preminted
classes, none of which the framework resolves itself). `"framework"`: the
rules the framework resolved and last handed the backend
(`style_attach`'s style mirror, kept current through reactive styles and
theme swaps) — on every native backend that IS what was applied. A field the
framework's rules don't set is absent (no inherited values on native); the
web reports every computed value. robot-test: `Locator::style()` returns a
typed `Style` (`is_line_through()`, `is_underline()`, `is_bold()`), and
`assert_style(|s| …)` waits for a style that lands late. For what the
native widget draws, beside what it was handed, use `introspect_native`.

### Scrolling into view

`scroll_into_view {element_id}` does what Playwright's
`scrollIntoViewIfNeeded` does: every scroll view the element sits in,
innermost first, moves by the least that shows the whole element inside it —
and, for the outermost, inside the window. Each moves only along the axes it
scrolls (a horizontal tab strip never gets a vertical offset); an element
bigger than its scroll view is aligned to the start; an element already in
view moves nothing. It answers `{scrolled: [{element_id, x, y, page?}]}`, the
scrollers it moved and their new offsets.

The scrollers are every registered ancestor the toolkit scrolls — a
`scroll_view`, and any other element whose native object scrolls
(`IntrospectionOps::scroll_port`: a virtualizer's own scroller, a web element
whose CSS `overflow` scrolls) — and then the scroller above the app's tree
(`ScrollTarget::Host`). That last one is the web page: when the app's root
outgrows the window and no `scroll_view` holds it, the document scrolls, and
an element below the fold is reached by scrolling it (`page: true` in the
answer, with the root element's id). A native window doesn't scroll, so
there is none off the web. Each scroller moves only within its range, and
one with no range (a `scroll_view` that grew to its content) is passed over
for the one outside it.

It is implemented once, in the vocabulary (`Robot::scroll_into_view`), over
what every backend reports — the window frames of the element and its
scrollers (`IntrospectionOps::absolute_frame`) and each scroller's offset and
range (`scroll_port`; web, macOS, iOS, Android, Windows and Linux report one)
— and moves a `scroll_view` through the same route as `set_scroll`, any
other scroller through `IntrospectionOps::set_scroll_offset` (web, macOS,
iOS, Android). The shifts
come from one read of the frames (scrolling a view by `d` moves its content
by `-d`), so a backend that settles a scroll's frames a pass later is not
scrolled twice.

robot-test's `click()` and `tap_native()` use it before tapping an element
that is outside the window, and again — then tap at once — when the tap is
refused as outside the window or covered (an element in the window but
clipped by its scroll view: the point lands on what is beside it). Every
other real-input action does the same before it sends: `long_press`,
`context_click`, `secondary_click`, `press_native`, `hover_native` and
`drag_native` (`release_native` scrolls nothing: it lifts where the press
went down). Opt out
per locator with `.no_scroll()`, or for the app with
`App::set_auto_scroll(false)`; `Locator::scroll_into_view()` scrolls without
tapping. An app from before the verb answers `unknown command`, and the tap
goes ahead as before.

A virtualized list (`flat_list`, `virtual_grid`) is an element of kind
`Virtualizer` (`flat_list(test_id = …)` anchors it), and its rows register
under it — each row sits in its own native cell on every backend, but
`get_parent`, `get_children`, `within` and the snapshot see it inside the
list. Only a mounted element can be scrolled to. A virtualized list's rows
beyond its window aren't elements yet, and the list has no `set_scroll`,
so `scroll_into_view` does not move it; bring the
row in with the list's own scroll (its handle's `scroll_to_index`, exposed
to a test through a component `#[method]` or a robot function).

`hover_native {element_id}` moves a real mouse over the element's center
with no button held (`on_hover`, the `state hovered` style).
`press_native` / `release_native {element_id}` split a tap in two so a test
can check the `state pressed` style in between, or hold a long press: the
press stays down for the real time between the two verbs. Same point
checks as `tap_native`. Both answer `{mechanism}`: `native` with real input
(web, macOS, Windows, Android: one gesture, one `downTime`), `hit_test`
where the backend can't inject input (iOS, Linux). There the robot holds
the press itself: `press_native` sends `Began` to the touch handlers under
the point (as `tap_native`'s `hit_test` tier finds them), `release_native`
sends `Ended` stamped with the time that passed, so a long-press
recognizer's timer fires while the press is held and a pressable fires on
the release. `hover_native` has no fallback:

| Backend | Hover |
|---|---|
| Web | `pointerout`/`mouseout`, `pointerleave`/`mouseleave`, `pointerover`/`mouseover`, `pointerenter`/`mouseenter`, then `pointermove`/`mousemove`, as a mouse sends them. Untrusted events don't move CSS `:hover` |
| macOS | AppKit enters a view from its tracking areas, driven by the window server's cursor, which a posted event doesn't move. The injection hit-tests the point, sends `mouseExited:` / `mouseEntered:` to the tracking areas it left and reached (the events the tracking manager would send), then posts the mouse-moved event |
| Windows | `WM_MOUSEMOVE` with no button, posted where a real move lands |
| iOS, Android, Linux | `unsupported` (no hovering pointer; Linux needs XTest) |

macOS posts its mouse events, so a pressed state lands on the next run-loop
turn: poll for it.

### Right click and long press

`context_click {element_id}` sends a secondary (right) click to the
element's center, with the same point checks, and answers `{mechanism}`:

| Backend | Right click |
|---|---|
| Web | `pointerdown` + `mousedown` (`button: 2`), `contextmenu` (skipped when the `mousedown` was prevented, as a browser skips it), `pointerup` + `mouseup`: Chrome's order on macOS and Linux. Always a mouse, also on a touch page |
| macOS | posted `rightMouseDown` / `rightMouseUp` events |
| Windows | `WM_RBUTTONDOWN` (`MK_RBUTTON`) / `WM_RBUTTONUP`, posted where a real click lands |
| Android, iOS, Linux | `hit_test`: the robot sends a `PointerButton::Secondary` `Began` into the touch handlers under the point. It bubbles past handlers that ignore it and stops at a pressable, as a right click does on a desktop backend |

An app handles both gestures in one touch handler: a press whose
`pointer_button().opens_context_menu()` is the right click, any other
press goes to a `long_press` recognizer.

robot-test's `Locator::context_click()` opens a context menu the way the
platform's user does: the `context_click` verb on web and desktop, a long
press on iOS and Android (`press_native`, `LONG_PRESS` (800 ms) of real
time, `release_native` on the same element). `long_press(hold)` holds a
press for `hold` on every platform; `secondary_click()` is always the
`context_click` verb.

### Keys: `key`

`key {key, modifiers?, element_id?}` presses a key as a user at the keyboard
would: a key down and up, to whatever holds keyboard focus. With
`element_id` the element is focused first (Playwright's `locator.press`; a
disabled field refuses); without, the key goes to the current focus or the
window (`page.keyboard.press`). `key` takes Playwright's names: `Enter`,
`Escape`, `Tab`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`,
`PageDown`, `ArrowUp`/`Down`/`Left`/`Right`, `Space`, `F1`–`F24`, `Shift`,
`Control`, `Alt`, `Meta`, `KeyA`–`KeyZ`, `Digit0`–`Digit9`, or any single
character. Modifiers are `Shift`, `Control`, `Alt`, `Meta` and
`ControlOrMeta` (Command on Apple platforms and on a web page in a Mac
browser, Control elsewhere), given in `modifiers` or joined to the key with
`+` (`"Meta+K"`, `"Shift+Tab"`). Every press carries the `key` and `code` a
US keyboard reports, a capital or shifted symbol carries Shift, and a
capital with a shortcut modifier means the letter (`"Meta+K"` reports
`key: "k"`, as Cmd+K does). An unknown name is an error. The answer is
`{mechanism}`:

| Backend | Mechanism | How |
|---|---|---|
| Web | `native` | `keydown` / `keypress` / `keyup` `KeyboardEvent`s on `document.activeElement` (or `<body>`), bubbling, so the field's `on_key_down`, `on_key` listeners and an overlay's Escape listener hear them. Script-dispatched events are untrusted and the browser performs no default action, so the backend performs the ones a test relies on when `keydown` isn't prevented: typing, Backspace/Delete, arrows/Home/End and Ctrl/⌘+A in a field (then its `input` event, so `on_change` runs), a newline for Enter in a `text_area`, Tab/Shift+Tab focus traversal, Enter/Space clicking a focused button or link |
| macOS | `native` | `NSEvent` key events posted to the app (`postEvent:`), with `flagsChanged` for each modifier around them, after making the window key: the key window's first responder types or deletes, the app key monitor hears them. Shift+Tab carries the back-tab character (U+0019), as AppKit types it, so the field editor moves focus back |
| Windows | `native` | `WM_KEYDOWN` / `WM_KEYUP` (`WM_SYS*` with Alt) posted to the focused child, the modifier bits set in the thread's keyboard state while they're delivered; `TranslateMessage` makes the `WM_CHAR` |
| Android | `native` | `KeyEvent` down and up to the decor view's `dispatchKeyEvent`, the hardware-keyboard path; a character no key types goes as `ACTION_MULTIPLE` text. That call enters below `ViewRootImpl`'s input stage, so a Tab the view tree leaves unhandled gets that stage's focus navigation from the injection itself: `focusSearch(FOCUS_FORWARD / FOCUS_BACKWARD)` from the focused view, then `requestFocus` |
| iOS, Linux | `dispatch` | An app can't create key events (no public API on iOS; Linux needs XTest). The framework's key paths run instead: the focused field's `on_key_down`, the app's `on_key` listeners, then, unless either prevented it, the field's text edit (a typed character appended, Backspace removing the last one, Enter a newline and Tab a tab character in a `text_area`: the caret is taken to be at the end), any other Tab or Shift+Tab moves focus through the platform's own traversal (`IntrospectionOps::move_focus`), and Escape closes the topmost overlay's `on_dismiss` — then the release. A window-level press (no `element_id`) edits the `text_input` that reports focus |

A single-line field's Enter has no default anywhere: submitting is the
field's `on_key_down` seeing `"Enter"`. An Escape nothing consumed closes
the topmost shown overlay with an `on_dismiss`, and only that one, on every
backend: through the toolkit's key routing on web, macOS, Windows, Android
and Linux, and on `dispatch` through the framework's stack of open
dismissable portals (`runtime_vocabulary::escape_dismiss`) — the same
stack an iPad's hardware-keyboard Escape uses, since UIKit has no Escape
handling for the window-level views a portal mounts as.

Tab and Shift+Tab move focus in the toolkit's own order on every backend.
On `dispatch` the backend runs it: Linux emits `move-focus` on the GTK
window — the signal its Tab bindings emit — which walks the widget focus
chain and, past the last focusable widget, leaves focus where it is (GTK4
doesn't wrap); iOS moves first responder among the window's enabled text
fields and editable text views in view-tree order, skipping hidden,
transparent and interaction-disabled subtrees, wrapping past the last —
the stops a real hardware-keyboard Tab moves between on iPhone, which
UIKit's text fields do on their own. Pressables are Tab stops where the
toolkit makes them ones (web always; macOS, Windows, Android, GTK per
their keyboard-navigation settings); iOS moves Tab among text inputs only.

## Functions

Some things a test needs have no element to act on: seed data, fake a
location, take the device offline, read what notifications the app posted.
The app or an SDK registers them as robot functions, and a test calls them
by name.

```rust
/// Fill the inbox with `count` sample messages.
#[robot_fn(name = "demo.seed")]
fn seed_inbox(count: u32) -> Result<Vec<String>, String> { … }

// At boot, on the UI thread (no cfg: inert outside robot builds):
runtime_vocabulary::robot_fn::register(seed_inbox::robot());
```

`#[robot_fn]` keeps the function as written and adds `<name>::robot()`, its
record (name, doc comment, parameters, return type). The name defaults to
the function's; SDKs namespace theirs. Arguments arrive as a JSON object
keyed by parameter name, each decoded with `serde` (a missing `Option` is
`None`); the return value is encoded back, and a `Result`'s `Err` fails the
call with its `Display`. Free functions only, no generics or borrowed
parameters. Only robot builds compile the adapter, so a release build
carries none of it.

| Verb | Answers |
|---|---|
| `list_functions` | `[{name, doc, args: [{name, type}], returns, async}]` |
| `call_function {name, args}` | `{done: true, value}`, or for an `async fn` `{done: false, call}` |
| `poll_function {call}` | `{done: false}` while it runs; then `{done: true, value}` (or its error), once |

An async function runs on the backend's executor and never holds the
bridge: every transport answers each request within its own timeout, and a
function may take longer. Functions run on the UI thread outside any
`World`, like an event handler, and the robot settles after each call.

### SDK test controls

| SDK | Register | Functions |
|---|---|---|
| `location` | `location::register_robot_controls()` | `location.set {latitude, longitude, accuracy_m?, …}`, `location.clear` |
| `connectivity` | `connectivity::register_robot_controls()` | `connectivity.set {online, transport?}`, `connectivity.clear` |
| `notifications` | `notifications::register_robot_controls()` | `notifications.log`, `notifications.clear_log` |
| `permissions` | `permissions::register_robot_controls()` | `permissions.set {permission, status}`, `permissions.clear`, `permissions.requests` |

Anything that asks the OS — a permission prompt, a sensor, the network —
should be fakeable, so a test assumes the answer it needs instead of
meeting a dialog it can't drive. A fake replaces the platform for the
SDK's whole surface: `location.set`
makes `current()` resolve the fix (with no permission prompt) and sends it
to every live `watch`, and the platform's own updates are dropped until
`location.clear`. The fakes and their hooks compile only into robot builds
(`robot_only!` / `robot_select!`). Each SDK's README has the details.

Deep links need no SDK: `open_link {url}` delivers a URL as the OS would
(below).

## Other verbs

| Area | Verbs |
|---|---|
| Tree | `get_snapshot`, `get_children`, `get_parent`, `count_elements` |
| Geometry / rendering | `get_frame`, `get_absolute_frame`, `get_device_frame`, `get_style`, `introspect_native`, `introspect_subtree`, `screenshot` |
| Navigation | `list_navigators`, `get_navigator_state`, `navigate {nav_id, action: push\|replace\|reset\|pop, path}`, `open_link {url}` (a deep / universal link, as the OS delivers one: observers, interceptors, then the navigators; answers `{reached, complete, routed}` — the deepest path the shown navigators reached, whether that is the whole link, and whether a navigator took it at all; a nested navigator that mounts later (a lazy screen, a data fetch) is moved by `open_link {url, resume: true}`, which routes again without re-running observers or interceptors). Navigator entries carry `query` (the active screen's decoded query parameters) and `href` (`active_path` plus the encoded query); `active_path` itself is path-only |
| Components | `list_components` (mounted instances), `get_component {instance_id}` (live props), `invoke_method {instance_id, method, args}` |
| Signals | `list_watched_signals`, `read_signal`, `write_signal` (`watch_signal_writable` only), `get_signal_history` |
| Functions | `list_functions`, `call_function`, `poll_function` (above) |
| Viewport | `get_viewport` → `{width, height, breakpoint}` (logical px; `breakpoint` is `xs`…`xl`, what the app's responsive layout sees); `set_viewport {width, height}` resizes the window so its content area is that size — desktop backends only (macOS `setContentSize:`, Windows `SetWindowPos` on the top-level window, Linux the GTK window's default size), through `IntrospectionOps::resize_window`; elsewhere an error naming the alternative (the runner's `--viewport` on web, a device or simulator on mobile). A maximized or full-screen window refuses; the new size reaches `get_viewport` through the window's ordinary resize path |
| Page (relay) | Answered by the relay, never forwarded to the app (as `screenshot`'s `cdp` is), so `call_verb` reaches them too. `load_url {url}` boots the app again at `url` and answers `{url}` once the new instance has said `hello`: on web the relay navigates the driven tab over DevTools (`url` resolves against the page's address; a change of `#fragment` only reloads); on Android it relaunches the app with the link through the dev session's hook (`am start -S -W -a VIEW -d <link>`; an app path `/x` becomes `https://localhost/x`, whose route is `/x`); iOS and desktop apps answer that the session can't relaunch them (a link launch on the simulator drops the environment the app reconnects with; a desktop dev session ends with its app). `set_cookie {name, value, url?, domain?, path?, http_only?, secure?, same_site?, expires?}` (default: the page's origin; HttpOnly allowed), `get_cookies` (the cookies sent to the page's address, HttpOnly included), `clear_cookies {name?}`: web only, a native app's HTTP stack keeps its own cookie store. All four need a tab in a browser the dev session launched (`idealyst test --web`, the headless web client): the relay finds it by the page's pairing nonce among the browser profiles in the session's state directory |
| App | `get_logs`, `clear_logs`, `get_identity`, `ping`, `get_platform` → `{platform}` (`web`, `ios`, `android`, `macos`, `linux`, `windows` — the `idealyst test --<platform>` names — or `tvos`, `androidtv`, `roku`, a custom backend's own name, `unknown`) |

### Screenshots on web

`screenshot` takes a `source`; a web app answers three, and names the one it
used in the result:

- `dom` — `backend-web/src/robot_screenshot.rs` copies the DOM (live input
  state, scroll offsets and 2D canvas pixels included) into an SVG
  `<foreignObject>` and rasterizes it. No user step; WebGL, plugin content
  (a PDF viewer) and exact font rendering can differ.
- `tab` — a frame of the tab's screen-share stream (`dev-http`'s
  `tab_capture.js`). A browser opens `getDisplayMedia` only from a click,
  so the user starts it from the "allow capture" chip on the dev
  overlay's robot line; the browser then asks them for permission. Until
  then `tab` errors and the chip lights up as "agent wants capture ·
  allow" (and lights the overlay's glyph if they collapsed it). Only a
  browser tab — this one, where capture handles can tell — is accepted.
- `cdp` — `robot_relay::cdp`: the relay captures the driven tab with
  `Page.captureScreenshot`, for a tab in a Chromium the dev session
  launched with `--remote-debugging-port` (the headless web client). It
  finds the tab by the nonce the page sent in its `hello` and left on
  `window.__idealyst_robot_nonce`.
- `auto` — `cdp` if reachable, else `tab` if shared, else `dom`.

## Clients

### robot-test (`#[robot_test]`)

Host-side tests over the bridge (`crates/dev/robot-test`), run by
`idealyst test`. Playwright-shaped, and **auto-waiting**: an action waits for
its element to exist and accept it (a disabled control refuses, so it waits
for it to be enabled), an assertion retries until it holds — both for up to
`App::set_timeout` (5 s), then panic with the last thing they saw. A test
never sleeps for a reactive update, a load or a navigation to land.

```rust
#[robot_test]
fn sign_in(app: &mut App) {
    app.navigate("/login");
    app.test_id("email").fill("me@example.com");
    app.test_id("submit").assert_disabled();     // waits for the rule to apply
    app.test_id("password").fill("hunter2");
    app.test_id("submit").click();               // waits for it to be enabled
    app.assert_path("/home");
    app.role("Toggle").nth(0).assert_checked(false);
}
```

- Locate: `test_id`, `text` (label substring), `label` (exact), `role` (kind),
  then narrow with `.with_label(..)`, `.with_text(..)`, `.with_role(..)`,
  `.with_test_id(..)`, `.within(container)` and `.nth(i)` —
  `app.role("TextInput").with_label("Email").within(Selector::test_id("login"))`.
  All of it goes to the app as ONE `find_all_elements` query per look. A
  `Selector` is the same thing without the app borrowed (to keep in a
  variable, or pass to `within`); `locator.selector()` detaches one, and
  `app.locate(selector)` turns it back into a locator.
- Act: `click` (native tap first, see above), `tap_native`,
  `dispatch_click`, `fill` / `type_text`, `set_toggle`, `set_slider`,
  `focus`, `blur`, `scroll_to`, `scroll_into_view`, `drag`,
  `drag_with_hold`, `drag_native`, `hover_native`, `press_native`,
  `release_native`, `context_click`, `long_press(hold)`,
  `secondary_click` (above); `.no_scroll()` before `click` / `tap_native` /
  any other real-input action acts without scrolling first.
- Read: `state()` ([`ElementState`]), `is_visible()`, `label()`, `count()`.
- Assert: `assert_visible`, `assert_hidden`, `assert_in_viewport`, `assert_text`,
  `assert_text_contains`, `assert_value`, `assert_checked`,
  `assert_enabled`, `assert_disabled`, `assert_focused`, `assert_hovered`,
  `assert_pressed`, `assert_count`.
- Platform: `platform()` (`Platform::Ios`, … with `is_mobile()` /
  `is_desktop()` / `is_native()`; `try_platform()` for an app too old to
  say) — pair with `viewport().breakpoint` for the form factor.
- Viewport: `viewport()` (`width`, `height`, `breakpoint`),
  `set_viewport(w, h)` (waits for the app to report the size; panics where
  the app can't resize itself) / `try_set_viewport` (returns why instead).
- App: `navigate` / `navigate_with` / `back` / `assert_path` /
  `assert_query(key, value)` / `assert_href(href)` / `navigators()`
  (each with `query` and `href`), `open_link(url)` (waits until the whole
  path shows, nested navigators that mount late included, and panics
  naming the path it reached otherwise; `try_open_link` returns the
  `LinkOutcome` instead), `call_function(name, args)` /
  `try_call_function` / `functions()` (waits for the function to register
  and for an async one to finish), `component(name).invoke(method, args)` / `.prop(name)`,
  `signal(name).assert_eq` / `.value()` / `.set(v)`, `logs()`,
  `screenshot()`, and `call(verb, args)` for any other verb.
- Page: `load_url(url)` boots the app again at `url` and waits for the new
  instance to answer (web and Android; up to the larger of the timeout and
  `LOAD_URL_TIMEOUT`, 60 s; `try_load_url` returns the error), and
  `set_cookie(Cookie::new(name, value).http_only())`, `cookies()`,
  `clear_cookie(name)`, `clear_cookies()` on web. Set a cookie before
  `load_url` for the server to see it at boot.

`assert_visible` needs the element mounted and `visible`; `assert_hidden`
passes when nothing matches or the match is not visible (a closed overlay,
a navigated-away screen).

A busy app is "not yet". When a query goes unanswered — the relay replies
`app response timed out: the relay waited N ms for the app to answer …`
(the app is connected but its UI thread is busy), a direct bridge replies
the same about its UI thread, or no reply reaches the client within
`App::set_io_timeout` (default 8 s; the client then says it gave up waiting
on the relay/bridge) — or the relay has no app for a moment (a reload),
every waiting path asks again until `set_timeout`'s deadline, then fails
with the last error. `count()`, `label()`, `is_visible()` and the other
reads that don't wait for a condition still wait out a busy app. An
action is never sent twice: one the app did not answer may yet run, so it
fails at once, saying so. Each request carries `timeout_ms` (the I/O
timeout less 0.5 s), which the relay and the app's bridge use as their
wait, so the app side reports first; a late reply is skipped by id. The
typed error is `robot_test::CallError` (`AppTimedOut`, `ClientTimedOut`,
`NoApp`, `Verb`).

A test that only means something on some platforms declares them:
`#[robot_test(platforms = [ios, android])]` (names `web`, `ios`,
`android`, `macos`, `linux`, `windows`; groups `mobile`, `desktop`,
`native`). Elsewhere it is never reported passed. libtest can't skip at
run time, so the macro puts the platforms in the test's name and its
`#[ignore]`: the test is `<fn>::robot_only_ios_android`, ignored with the
reason. `idealyst test` lists the binary's tests, asks the app
`get_platform`, and runs the ignored tests that belong there by name
(`--include-ignored --exact …`), so the others count as `filtered out`
and are named in a `[test] not on web: …` line; when none belong, the
run is the plain one and they show `ignored`. Under a bare `cargo test`
they show `ignored, runs on ios, android only: …`; `cargo test --
--include-ignored` runs them, and one whose app is on another platform
prints `SKIP` and returns. The body sits in a module (`use super::*`),
so it names the file's items through `crate::`, not `super::`. A test
with no app to reach still prints `SKIP` and returns — bare `cargo test`
without `idealyst dev` up stays green; `idealyst test` always has an app.
The app an `idealyst test` run launched is that run's alone: its relay
marks the registration `under_test`, a bare `cargo test` never discovers
it (it skips, or finds your own `idealyst dev`), and the MCP's robot tools
refuse it. A second driver would interleave its steps with the suite's —
its `set_count(0)` landing between the suite's tap and its assertion.

Per-test setup is a `Fixture` the test asks for by type
(`fn t(app: &mut App, user: &SignedIn)`, by `&` or `&mut`): `setup(app)`
runs before the body in parameter order, `teardown(&mut self, app)`
after it in reverse — also when the body panics. A fixture can build on
another by calling its `setup`. See `crates/dev/robot-test/src/fixture.rs`
and the testing guide.

### Running a suite on each platform

`idealyst test --web | --macos | --ios | --android | --linux | --windows
<project>` builds and launches the app (`idealyst dev --local`), waits for
THIS project's app to register — a registration from another project or an
earlier run is ignored — and runs the `tests/robot.rs` suite against it
(compiled before the app launches, then run directly). The dev session's
output goes to a log file named on failure. `--windows` runs on a remote
machine with `IDEALYST_WINDOWS_REMOTE` (docs/windows.md); `--linux` needs a
Linux host. `--attach` drives an app already running — never one another
`idealyst test` run launched (its registration is `under_test`).

The session gets its own port (`--port`, else a free one) on every
platform, so a full-stack app's server — and the `IDEALYST_SERVER_URL` a
native app dials — is the run's, never the developer's `idealyst dev`.
The session also runs as `idealyst dev --isolated test` (`--isolated
<key>` / `--no-isolated` to change it): its build dirs, log, lock and
`.idealyst/isolated/test/` relay files are its own, so it runs beside the
developer's session of the project. The runner drives the registration the
session it launched wrote, never another session of the project: only a
registration under the dev process's pid (its relay's — web, macOS, iOS,
Android) or a process it started (a Linux app's own bridge) is taken, and
until it appears the runner keeps waiting rather than falling back to
another new one. `--windows`, whose app (under Wine, or remote) registers
under a pid that is no local process, takes a new registration no other
live `idealyst dev` wrote. A launched session that exits before
registering (a refusal, a failed build) ends the wait with the reason it
printed. `--web`
on a project without a server can't be isolated (it stages into the
project's own `pkg/`) and runs unisolated.

One session runs per key and project, so two runs with the default key
can't both launch. Before building anything the runner looks up the
session registry (`~/.idealyst/dev-sessions/`): if a live session already
holds the run's key (another `idealyst test`, or an `idealyst dev
--isolated test`), the run is refused with that session's pid, launcher
and age. Runs in parallel take a key each — `--isolated <key>`, or
`--isolated auto` for the first free one of `test`, `test-2`, … (a
`--parity` run suffixes each key with the platform). An entry whose
process is gone (a crashed run) holds nothing.

One run can cover several configurations, each a full pass of the suite
with its own line in the summary (any failure fails the run):

- `--viewport 390x844,1440x900`: web launches once and resizes the page
  over the DevTools protocol (`Emulation.setDeviceMetricsOverride`, so
  390 is 390 — headless Chrome's window can't go below 500 px); desktop
  apps relaunch per size via `IDEALYST_WINDOW_SIZE`.
- `--device iphone-14,pixel-7` / `--mobile` (web): device metrics, pixel
  ratio, touch emulation and user agent, on one DevTools session held for
  the whole run (an override ends when its session detaches). The page is
  reloaded for each size, so the app boots on the screen it is tested on.
- `--simulator "iPhone 15,iPad Air 11-inch (M2)"` (iOS) /
  `--emulator Pixel_8_API_35,emulator-5556` (Android): launch, run, stop,
  next.

Tests read `IDEALYST_TEST_VIEWPORT`, `IDEALYST_TEST_DEVICE` and
`IDEALYST_TEST_PORT`. `--env KEY=VALUE` reaches the app on every platform
(as `idealyst dev --app-env`), the dev session (a full-stack server), the
tests and the hooks; `--before` /
`--after` shell hooks run around the whole run (a failing `--before` stops
it; `--after` always runs). See the testing guide.

The same suite passes unchanged on web, macOS, iOS (simulator), Android
(emulator) and Windows — `crates/dev/robot-e2e/examples/robot-e2e-demo`.

An app with an in-app self-test should stand it down when a driver owns
it: `runtime_vocabulary::robot::is_test_driven()`. `idealyst test` sets
`IDEALYST_TEST_DRIVER`, which the tooling carries to every platform — a
child process inherits it; Android gets it through the manifest
(`IdealystEnv`), iOS through `simctl`, a remote Windows app through its
launch environment, and a web page as a global the dev server injects.

### MCP server

Every verb is reachable: dedicated tools for the element verbs (`find_element`, `click`,
`tap_native`, `drag_native`, `pointer_native` (hover / press / release), `get_state`, `set_scroll`, …), `list_navigators` / `get_navigator_state` /
`navigate`, `list_watched_signals` / `read_signal` / `write_signal`,
`list_component_instances` / `get_component_instance` / `invoke_method`
(`list_components` is the *catalog*), `open_link`, `list_functions` /
`call_function` (which polls an async function to completion), and
`call_verb` for any verb without
its own tool (`get_device_frame`, `get_signal_history`, an SDK's custom verb).
