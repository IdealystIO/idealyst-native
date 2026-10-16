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
