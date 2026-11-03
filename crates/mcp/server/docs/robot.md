# Robot tools

Robot tools drive a RUNNING app through its Robot bridge: find elements, read state, press, type, drag, scroll, navigate, call test hooks, and capture screenshots. The app must be built with the robot feature (`idealyst dev` turns it on unless you pass `--no-robot`), and must be registered: `list_apps` shows what is up, and `wait_for_app` blocks until it is. Every Robot tool takes an optional `app` selector (see `get_docs("app")`).

`read_guide testing` explains writing a cross-platform E2E suite on top of these tools.

## elements

Elements are found by criteria and then addressed by the numeric `element_id` the find returns. Ids are valid while the element stays mounted.

Find criteria. Every criterion given must match (they combine with AND); with none, every element matches.

- `test_id`: exact match on the author's `test_id`. Prefer this; it is stable across platforms. A `test_id` shared by repeated rows matches every row.
- `label`: exact accessible label / visible text.
- `label_contains`: label substring.
- `kind`: framework element kind (`View`, `Text`, `Button`, `Pressable`, `TextInput`, `Toggle`, `Slider`, …). An unknown kind is an error.
- `within`: only descendants of a container (never the container itself). Either an `element_id`, or a find object with the same criteria plus an optional `nth` (0-based, document order) that picks one of the containers it matches: `{ "kind": "TextInput", "label": "Email", "within": { "test_id": "login" } }` is the Email field inside the login form, not its caption and not a field elsewhere.

Apps built before criteria combined honored only the first present (test_id, label, label_contains, kind) and ignored `within`.

## find_element

Returns the first match as `{ id, kind, test_id, label, visible, in_viewport, state? }`. `visible` means the toolkit shows it with a non-empty box. `state` is the same object `get_state` returns.

## find_all_elements, count_elements

`find_all_elements` returns every match, in the same shape as `find_element`. `count_elements` returns only how many match.

## get_children, get_parent

Walk the element tree from an element: its children, or its parent (`null` at the root). Each element has the `find_element` shape.

## get_state

An element's own STATE, as a user would see it, with no help from the app: `{ value, checked, disabled, focused, hovered, pressed }`. `value` is a TextInput's text or a Slider's value; `checked` a Toggle's; `focused` a TextInput's. `hovered` / `pressed` are whether the element's `state hovered` / `state pressed` style is on; they are omitted on web (CSS owns hover there) and on elements whose style has no state variants. A TextInput adds `keyboard` (the soft-keyboard kind asked for: `default`, `email`, `phone`, `number`, `decimal`, `url`) and, on web/iOS/Android, `native_keyboard` (the kind the native widget holds).

To read an element's resolved style, `call_verb` with `get_style` and `{ "element_id": … }`: `{ source, style: { color, background, opacity, font_size, font_weight, font_style, font_family, text_decoration, text_align, border_width, border_color, corner_radius } }`. `source` is `native` on web (the browser's computed style) and `framework` elsewhere (the rules the backend was handed); `text_decoration` is `none` / `underline` / `line-through` / `underline line-through`, colors `[r, g, b, a]` in 0..1. Absent fields: the style doesn't set them. Fields the element doesn't have are omitted; a View/Text/Image returns `{}`.

Robot actions on a disabled element fail with "refused: the element is disabled", the same way the control refuses a user.

## click

Presses an element inside the framework. A Button/Pressable/Link runs its press handler; anything else gets a synthetic tap (began → ended at its center) delivered to the nearest `on_touch` handler, bubbling to ancestors, so clicking a Text inside a clickable Table row presses the row.

It skips the OS event path, hit testing and overlays. Use `tap_native` to check that a real tap would land. A disabled control refuses.

## tap_native

Taps an element's center with REAL platform input, through the toolkit's own event path: hit testing, overlays, gesture recognizers and the native disabled state all take part, which `click` skips.

It refuses the way a finger would fail: the element is not visible, its center is outside the window (scroll it into view), or another element covers it (the error names it).

Returns `{ mechanism }`:

- `"native"`: platform events (web DOM events, macOS NSEvents, Android MotionEvents, Windows window messages).
- `"hit_test"`: iOS, where an app can't create touches. UIKit's hit test confirms the point, then the element's press runs.

Android has no hit test, so there the injected event is the only check. Errors with "unsupported" on Linux; use `click`. On Android, `tap` injects at the OS level instead (adb).

## tap

Injects a REAL OS-level touch on an Android emulator/device via `adb shell input tap`, through the full Android input stack (InputManager → window → view hit-test). It exercises hit testing, overlays, z-order and disabled state the way a real finger would.

Target it one of three ways, in this precedence order:

1. `element_id` (from `find_element`).
2. `test_id` (resolved over the bridge).
3. Raw `x`/`y` in physical device pixels, screen top-left origin. Both are required together.

Element taps read the element's on-screen pixel rect via the bridge's `get_device_frame` and tap its center; the device reports physical pixels, so there is no host-side density math. Requires Android platform-tools `adb` on PATH and an attached device/emulator; `serial` (from `adb devices`) picks one when several are attached. Android only: other backends don't implement `device_frame`.

## drag

Drags one pointer along `points`, `[x, y]` pairs in logical px relative to the element's top-left. Delivered as touch events (Began at the first point, Moved through the rest, Ended at the last) to the element's touch handler or its nearest ancestor's: draw on a signature pad, drive a pan handle, drag to reorder. Events are one frame apart.

On a Slider, each point's x across the track sets the value (0 = min, right edge = max) through the slider's own change callback. A disabled slider returns an error.

`hold_ms` presses and holds at the first point before moving. It is required for drag-and-drop on iOS/Android, where a drag picks up after a 200 ms long press (pass 200 or more). The hold is carried by event timestamps, not real time, so it reaches recognizers that read the event clock (drag-and-drop does), but not other app timers.

## drag_native

Drags with REAL platform input along `points` (same coordinates as `drag`): down at the first point, moves, up at the last, through the toolkit's event path. The first point is checked like `tap_native`'s. Errors with "unsupported" where the backend can't synthesize input (iOS, Linux); use `drag` there.

## pointer_native

Real platform mouse input on an element, one `action` at a time:

- `"hover"`: the pointer moves over the element's center with no button held. Drives `on_hover` and the `state hovered` style.
- `"press"`: the primary button goes down on its center and stays down, so `get_state` can show `pressed: true`.
- `"release"`: the button comes up at its center. The press is held for the real time between `press` and `release`, so the pair is a long press.
- `"context_click"`: a secondary (right) click on its center, `contextmenu` included on web.

The point is checked like `tap_native`'s. Returns `{ mechanism }`: `native` for real input, `hit_test` where the backend can't inject it (iOS, Linux; `context_click` on Android too): the robot then sends the press (or the right click) through the framework's own touch handlers, so long-press recognizers and context-menu handlers still run. Some backends post their events (macOS), so the state can change a moment later: poll `get_state`. `hover` errors with "unsupported" on iOS, Android and Linux. On web the events reach `on_hover` handlers, but CSS `:hover` only follows a real pointer.

## type_text

Replaces a TextInput's current value with `text`.

## set_toggle, set_slider

Set a Toggle's `value` (true/false) or a Slider's `value` the way a user's input would: the native widget moves, then the control's change callback reports it. A disabled control refuses. To drag a Slider's thumb instead, use `drag`.

## focus, blur

Give an element (typically a TextInput) keyboard focus, or take it away. `get_state` reports `focused`.

## set_scroll

Scrolls a ScrollView's content to offset (`x`, `y`) in logical px through the native scroll view, so its `on_scroll` / `on_end_reached` fire as for a user's scroll.

To show an element rather than reach an offset, `call_verb` with `scroll_into_view` and `{ "element_id": … }`: every scroll view the element sits in (nested, either axis) moves the least that shows it in each and in the window, and the reply lists the scrollers it moved, `{ "scrolled": [{ "element_id", "x", "y", "page"? }] }`. Scrollers include any element whose native view scrolls, and on web the page itself (`page: true`) when the app's root outgrows the window. Already in view, nothing moves. Do this before `tap_native` on an element outside the window. `get_platform` (also through `call_verb`) names the app's platform: `web`, `ios`, `android`, `macos`, `linux`, `windows`.

## get_frame, get_absolute_frame

An element's layout frame relative to its parent. `get_absolute_frame` returns it in window coordinates.

## get_snapshot

The whole element tree of the running app.

## get_logs, clear_logs

The running app's captured logs. `clear_logs` empties the buffer, so a later `get_logs` shows only what happened after.

## list_navigators, get_navigator_state

The app's navigators: `[{ nav_id, element_id, type_name, active_route, active_path, query, href, depth, can_go_back, is_current, base, stack: [{ route, path }], controllable }]`. `active_path` is path-only; `query` is the active screen's query parameters as an object of decoded values (a repeated key gives its first value) and `href` is `active_path` plus the encoded query — where screen state such as a filter shows. `get_navigator_state` returns one entry.

## navigate

Moves navigator `nav_id` without clicking through the UI. `action` is `push`, `replace`, `reset` or `pop`; `path` is resolved like a deep link (ignored for `pop`).

## viewport

With no arguments, reads the app's viewport: `{ width, height, breakpoint }`, logical px, with `breakpoint` one of `xs`, `sm`, `md`, `lg`, `xl` — what the app's responsive layout sees (bridge verb `get_viewport`).

With `width` and `height`, first resizes the app's window so its viewport is that size (bridge verb `set_viewport`), then returns `"ok"`; read it again until it reports the new size. Desktop apps only (macOS, Windows, Linux). Elsewhere it is an error: on web the test runner sizes the browser, and on iOS and Android the size is the device's. A maximized or full-screen window refuses.

## open_link

Opens a URL in the app as if the OS delivered it: a deep link (`myapp://items/42`) or a universal link (`https://example.com/items/42`). The app's link observers see it, an interceptor may claim it, otherwise the navigators move to its path — every level of it, as a cold load of the URL would: a nested navigator that mounts later (a lazy screen, a data fetch) is moved once it is up, for up to 5 s. Returns `{ reached, complete, routed }`: the deepest path the shown navigators reached, whether that is the whole link (path and query), and whether a navigator took it at all (`false` when an interceptor claimed it or no screen matches). `complete: false` with `routed: true` means a segment matched no screen; `reached` says where it stopped.

## load_url, set_cookie, get_cookies, clear_cookies (via call_verb)

Served by the relay, not the app. `load_url {url}` boots the app again at `url` and returns `{url}` once the new instance has connected: the web tab navigates (`/x` resolves against the page), an Android app is relaunched with the link; iOS and desktop apps answer that the session can't relaunch them. `set_cookie {name, value, url?, domain?, path?, http_only?, secure?, same_site?, expires?}` (default: the page's origin), `get_cookies`, `clear_cookies {name?}` reach the browser's cookies, HttpOnly ones included; web only. They need a tab in a browser the dev session launched (`idealyst test --web`, the headless web client).

## list_watched_signals, read_signal

The signals the app exposes by name with `watch_signal` / `watch_signal_writable`: `[{ id, name, value, writable, … }]`. Values are their Debug form. `read_signal` reads one by `name` or `id`.

## write_signal

Writes a signal the app registered with `watch_signal_writable`, by `name` or `id`. `value` is JSON, deserialized into the signal's type. Effects run as for any write.

## list_functions

Functions the app registered with the robot (`#[robot_fn]`, plus SDK test controls such as `location.set` / `connectivity.set` / `notifications.log`): `[{ name, doc, args: [{ name, type }], returns, async }]`. Use them to seed data or fake a location, permission or network state.

## call_function

Calls a registered function with `args` keyed by parameter name and returns its result as JSON. An async function is waited for up to `timeout_ms` (default 10000). A failure returns the function's own error message.

## list_component_instances

The component INSTANCES mounted in the app (`list_components` lists the catalog instead): `[{ instance_id, name, file, line, element_id, methods: [{ name, args }] }]`. `instance_id` is what `invoke_method` and `get_component_instance` take; `element_id` is the element the instance renders as.

## get_component_instance

One mounted instance with its LIVE props: `{ instance_id, name, file, line, element_id, methods, props: [{ name, type, mode (live/static), value }] }`.

## invoke_method

Calls a `#[method]`-tagged method on a component instance, with `args` keyed by parameter name (omit or `{}` for none).

## call_verb

Calls any bridge verb by name with JSON `args` and returns its raw result. It covers verbs no dedicated tool covers, such as custom verbs an SDK or host registered. The verb set is documented in `docs/robot.md` in the framework repo.

### Pressing keys: `key`

`call_verb` with `key` and `{ "key": "Enter", "element_id"?: … }` presses a key as a user at the keyboard would: a key down and up, to whatever holds keyboard focus. With `element_id`, the element is focused first (Playwright's `locator.press`); without, the key goes to the current focus or the window. `key` uses Playwright's names: `Enter`, `Escape`, `Tab`, `Backspace`, `Delete`, `ArrowUp`/`Down`/`Left`/`Right`, `Home`, `End`, `PageUp`, `PageDown`, `Space`, `F1`–`F24`, any single character, joined to modifiers with `+` (`Shift+Tab`, `Meta+K`). `ControlOrMeta` is Command on Apple platforms and Control elsewhere. An unknown name is an error. A disabled field refuses.

Returns `{ mechanism }`:

- `"native"`: real key events through the platform's key path (web DOM KeyboardEvents, macOS posted NSEvents, Windows window messages, Android KeyEvents): the field's own editing (Backspace deletes, a character types), its `on_key_down` (Enter submits), app `on_key` listeners, Escape closing an overlay where the platform does that.
- `"dispatch"`: iOS and Linux, where an app can't create key events. The framework's own key paths run instead: the focused field's `on_key_down`, its text edit (caret taken to be at the end), then the app's `on_key` listeners. Toolkit defaults such as Tab moving focus don't happen.

## screenshot

Captures a PNG of the running app, saves it to disk, and returns `{ path, width, height }`. `path` is absolute (`~/.idealyst/screenshots/<app>-<timestamp>.png` by default). The image bytes are deliberately NOT returned; read the file at `path` to see it.

`source` picks the capture, and the result's `source` says which one produced it.

A web app has three captures, which trade exactness against needing the user:

- `dom`: copies the page's DOM into an SVG and rasterizes it. Needs nothing from anyone and always works, but it is a copy: WebGL canvases, PDF viewers and other plugin content, and some font rendering can come out wrong or blank. Live input state (checked boxes, typed text, selected options), scroll positions, 2D canvases and open portals (dialogs, menus, popovers, toasts) are carried over.
- `tab`: the browser's real pixels of the user's own tab. A browser only lets a page share its tab after the user clicks, so the user does it once from the "allow capture" chip on the dev overlay's robot line (bottom-right of the page); the browser then asks them for permission. Before that, `tab` returns an error saying so, and the chip lights up as "agent wants capture · allow" (if they collapsed the overlay to its glyph, the glyph lights up instead; clicking it expands the overlay): ask the user to click it and allow the browser's prompt, then call `screenshot` again. The share lasts until the user stops it or the page fully reloads. A browser under automation (`navigator.webdriver`: Playwright, Selenium) shows no overlay corner at all, so it has no chip and no `tab` capture; use `cdp` or `dom` there. Chromium offers only that tab in its dialog; other browsers show their full picker, and anything but that tab is refused. This is the way to see what the user is seeing.
- `cdp`: the browser's real pixels over the DevTools Protocol, with no user step, but only for a tab in a browser the dev session launched: the headless web client (`run_dev` with `headless_client`, or the one `idealyst dev` starts on a machine with no display). Use it to look at your own headless tab. It errors for the user's own browser.
- `auto` (default): `cdp` when the tab is reachable, else `tab` while it is shared, else `dom`.

The relay serves `cdp` and the robot drives one tab at a time (the one that connected last), so `cdp` and `tab` both picture the tab the other Robot tools act on.

Native apps capture their real rendered surface whatever the `source`. A runtime-server session has its own two:

- `client`: the connected client's real surface, captured over the wire.
- `replay`: rasterizes the current scene with the framework's wgpu renderer server-side. Always available, even with no client attached, but it is the framework's renderer, not the platform's.
- `auto` (default): tries `client`, falls back to `replay`.

`width`/`height` (physical px) apply to `replay` only (default: the session's viewport); every other capture returns the surface's own pixel size.

Use `get_snapshot` / `introspect_native` for structured assertions. Errors if the session hasn't registered the screenshot verb.

## introspect_native

Reads an element's PLATFORM-NATIVE render state: the resolved geometry and visual properties as the backend itself reports them (a macOS `CALayer`'s backgroundColor/cornerRadius/borderWidth + the resolved NSFont; web's `getComputedStyle` + `getBoundingClientRect`), NOT the styles the author wrote.

Returns a `NativeNode` tree:

- `class`: the platform class/tag (`NSView`, `div`).
- `role`.
- `frame`: logical px, window-relative.
- `props`: canonical keys, each a typed `{ type, value }`: `background_color`, `opacity`, `corner_radius`, `border_width`, `border_color`, `text_color`, `font_family`, `font_size`, `font_weight`, `text`, `shadow_radius`, `shadow_color`, `hidden`. A key is ABSENT when the platform doesn't expose it (missing ≠ 0).
- `children`: the native sub-objects composing THIS primitive, pruned at child framework elements.

`null` means the element isn't laid out yet or the backend doesn't implement the read (web, macOS and Linux/GTK do; iOS/Android/terminal are stubs). Pair with `compare_native_parity`.

## compare_native_parity

The automated cross-platform PARITY check: compares the platform-native render state of two running apps, `app_a` and `app_b` (`list_apps` selectors such as `todo:web` vs `todo:macos`).

For a meaningful result both apps must show the SAME screen at the SAME viewport size; otherwise responsive layout makes the trees legitimately diverge.

### Alignment

The two element trees are aligned STRUCTURALLY by a cross-platform signature: `test_id` where present, else framework kind + label. Raw positional index is never used, because it collapses when one platform nests an extra wrapper. Each aligned pair's `introspect_native` is then diffed.

### Geometry

Geometry is where layout bugs show, and it is invisible in the prop maps:

- `frame.width` / `frame.height` compared absolutely.
- `frame.dx` / `frame.dy` compared RELATIVE to the nearest aligned ancestor, so a shifted container doesn't report every descendant.
- `frame.child_axis`: the axis a node's children progress along (row/column), compared as a category. It catches "the icon sits beside the label on web and above it on native" with no tolerance.

`geometry_tolerance` defaults to 2 px, since text shaped by two engines legitimately drifts a pixel. `no_geometry: true` turns geometry off.

### Props and normalization

Props are diffed with cross-platform NORMALIZATION:

- Font/text props are compared only on text-bearing elements (web reports inherited fonts everywhere; macOS doesn't).
- A transparent color is equivalent to an absent one.
- `font_family` is matched by family class: `-apple-system` ≡ `.AppleSystemUIFont` ≡ system, while `ui-monospace` stays distinct.

`raw: true` disables normalization and reports every representation difference. `color_tolerance` (per channel, `0.0..1.0`, default 0.02) absorbs color-space round-trip drift; `length_tolerance` (px, default 0.5) covers corner radius, font size and other lengths.

### Scoping

Pass `root` (a `test_id`) to compare only that subtree. Use a content anchor to exclude the navigator chrome: sidebars and headers are built per-platform with different native structure and legitimately don't align.

### Result

`{ app_a, app_b, aligned, structural: { only_a, only_b, items: [{ path, kind, only_in }] }, prop_divergences, mismatches: [{ path, key, kind, detail }] }`

- `aligned`: matched element count.
- `structural`: elements one platform renders and the other doesn't.
- `mismatches[].kind`: `value_differs` or `prop_missing`. A `frame.*` key is a geometry finding.

Empty `mismatches` + empty `structural` = full parity. The tool finds WHERE platforms diverge; reason about WHY (expected platform difference vs real bug) and, if asked, propose the upstream fix.
