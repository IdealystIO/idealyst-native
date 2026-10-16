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
  (`watch_signal`), `robot_props.rs`, `robot_ext.rs` (SDK nodes).
- `crates/runtime/shared/src/robot/` — the transport (TCP bridge, relay
  client) and the registry-independent verbs (`get_logs`, `screenshot`, …).
- Clients: `crates/dev/robot-test` (`#[robot_test]` host-side tests),
  `crates/dev/robot-e2e` (in-app suites), the MCP server.

## Elements

`find_element` / `find_all_elements` take one of `test_id`, `label`,
`label_contains` or `kind`, and return

```json
{"id": 7, "kind": "Toggle", "test_id": "wifi", "label": null,
 "state": {"checked": true, "disabled": false}}
```

`state` is present on elements that have any (below); `get_snapshot`
nodes carry it too.

### State

`get_state {element_id}` returns the element's own state — what a user
would see on screen — with no help from the app:

| Field | Primitives | Meaning |
|---|---|---|
| `value` | `text_input`, `text_area` (string), `slider` (number) | The text the field shows; the slider's value, snapped to `step` |
| `checked` | `toggle` | What the switch shows |
| `disabled` | toggle, slider, text input/area, pressable, button | Whether it refuses input |
| `focused` | `text_input` | Whether it holds keyboard focus |

It is the primitive's view, not the author's: an uncontrolled toggle reports
what it was last flipped to, a controlled one the author's value after each
write-back. Fields a primitive doesn't have are left out; a view, text or
image reports `{}`.

## Actions behave like a user

`click`, `type_text`, `set_toggle`, `set_slider`, `focus`, `blur`,
`set_scroll` and `drag` act on a control the way a person would:

- **A disabled control refuses.** The action fails with
  `action 'set_toggle' refused: the element is disabled`, and nothing
  reaches the author — so a test can assert that `disabled` blocks input.
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
delivery or hit testing.

## Other verbs

| Area | Verbs |
|---|---|
| Tree | `get_snapshot`, `get_children`, `get_parent`, `count_elements` |
| Geometry / rendering | `get_frame`, `get_absolute_frame`, `get_device_frame`, `introspect_native`, `introspect_subtree`, `screenshot` |
| Navigation | `list_navigators`, `get_navigator_state`, `navigate {nav_id, action: push\|replace\|reset\|pop, path}` |
| Components | `list_components` (mounted instances), `get_component {instance_id}` (live props), `invoke_method {instance_id, method, args}` |
| Signals | `list_watched_signals`, `read_signal`, `write_signal` (`watch_signal_writable` only), `get_signal_history` |
| App | `get_logs`, `clear_logs`, `get_identity`, `ping` |

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
  then `.nth(i)`.
- Act: `click`, `fill` / `type_text`, `set_toggle`, `set_slider`, `focus`,
  `blur`, `scroll_to`, `drag`, `drag_with_hold`.
- Read: `state()` ([`ElementState`]), `label()`, `count()`.
- Assert: `assert_visible`, `assert_hidden`, `assert_text`,
  `assert_text_contains`, `assert_value`, `assert_checked`,
  `assert_enabled`, `assert_disabled`, `assert_focused`, `assert_count`.
- App: `navigate` / `navigate_with` / `back` / `assert_path` /
  `navigators()`, `component(name).invoke(method, args)` / `.prop(name)`,
  `signal(name).assert_eq` / `.value()` / `.set(v)`, `logs()`,
  `screenshot()`, and `call(verb, args)` for any other verb.

`assert_visible` checks that the element is mounted; an element hidden by its
layout still passes.

### MCP server

Every verb is reachable: dedicated tools for the element verbs (`find_element`, `click`,
`get_state`, `set_scroll`, …), `list_navigators` / `get_navigator_state` /
`navigate`, `list_watched_signals` / `read_signal` / `write_signal`,
`list_component_instances` / `get_component_instance` / `invoke_method`
(`list_components` is the *catalog*), and `call_verb` for any verb without
its own tool (`get_device_frame`, `get_signal_history`, an SDK's custom verb).
