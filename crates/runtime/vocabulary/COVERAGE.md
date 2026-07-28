# Backend → capability-trait coverage (P2a)

Every method of `runtime_core::backend::Backend` (crates/runtime/core/src/backend.rs,
159 trait methods at the time of the split) accounted for. Grouping was derived
mechanically from which walker module calls which method (grep over
`crates/runtime/core/src/walker*`); each trait's rustdoc names its callers.

**Totals: 159 / 159 accounted. 152 methods on 30 Ops traits + 7 methods absorbed
by the P1 `runtime_scene::Host` seam. 0 deferred, 0 excluded** — methods expected
to retire in later phases are still assigned a trait now (flagged ⏳ below) so the
`LegacyBridge` surface is total and nothing is silently unaccounted for.

## Absorbed by `runtime_scene::Host` (P1 seam — NOT redeclared in caps)

| Backend method | Host method | Note |
|---|---|---|
| `insert` | `insert` | identical |
| `insert_many` | `insert_many` | identical (bridge delegates to Backend's default/override, not Host's default) |
| `insert_at` | `insert_at` | identical |
| `remove_child` | `remove_child` | identical |
| `clear_children` | `clear_children` | identical |
| `create_reactive_anchor` | `create_anchor` | renamed in the P1 extraction |
| `supports_child_splice` | `supports_splice` | renamed in the P1 extraction |

## `caps::AppEnvOps` (8) — mount-time environment + app chrome

`color_scheme`, `platform`, `url_opener`, `fullscreen_setter`,
`set_page_metadata`, `set_app_background`, `set_scrollbar_theme`,
`set_app_key_handler`

## `caps::LifecycleOps` (5) — build/flush lifecycle + policy flags

`finish`, `run_layout`, `schedule_layout_pass` (no `self`; stays an associated
fn so the navigator walker's `|| B::schedule_layout_pass()` hook shape is
preserved), `is_hydrating` ⏳ (web hydration policy — expected to fold into the
P3 web driver; assigned here so the bridge stays total), `renders_lazy_chunks`

## `caps::ViewOps` (2)

`create_view`, `make_view_handle`

## `caps::InputOps` (6) — shared by view, pressable, AND external walkers

`install_touch_handler`, `claim_touch`, `install_wheel_handler`,
`install_hover_handler`, `mark_preserves_focus`, `install_file_drop_handler`

## `caps::PressableOps` (2) — `: ViewOps` (default lowers to `create_view`)

`create_pressable`, `make_pressable_handle`

## `caps::TextOps` (11)

`create_text`, `create_styled_text`, `update_styled_text`, `update_text`,
`create_text_with_id`, `update_text_by_id`, `release_text_id`,
`supports_js_text_bindings`, `register_reactive_text_binding`,
`release_reactive_text_binding`, `make_text_handle`

## `caps::ButtonOps` (3) — `: TextOps` (`update_button_label` default lowers to `update_text`)

`create_button`, `update_button_label`, `make_button_handle`

## `caps::ImageOps` (6) — `: ExternalOps` (placeholder default)

`create_image`, `update_image_src`, `update_image_alt`,
`install_image_load_handler`, `install_image_error_handler`, `make_image_handle`

## `caps::IconOps` (6) — `: ExternalOps`

`create_icon`, `update_icon_color`, `update_icon_data`, `update_icon_stroke`,
`animate_icon_stroke`, `make_icon_handle`

## `caps::LinkOps` (3) — `: ViewOps` (default degrades to a container)

`create_link`, `update_link_url`, `make_link_handle`

## `caps::TextInputOps` (9) — `: ExternalOps`; input + area share a walker

`create_text_input`, `update_text_input_value`, `update_text_input_secure`,
`set_text_input_focus_handler`, `update_text_input_placeholder`,
`create_text_area`, `update_text_area_value`, `make_text_input_handle`,
`make_text_area_handle`

## `caps::ToggleOps` (3) — `: ExternalOps`

`create_toggle`, `update_toggle_value`, `make_toggle_handle`

## `caps::SliderOps` (3) — `: ExternalOps`

`create_slider`, `update_slider_value`, `make_slider_handle`

## `caps::ActivityIndicatorOps` (3) — `: ExternalOps`

`create_activity_indicator`, `update_activity_indicator_size`,
`make_activity_indicator_handle`

## `caps::ScrollOps` (4)

`create_scroll_view`, `node_scroll`, `set_node_scroll`,
`make_scroll_view_handle` — the generic offset pair lives here (not on
NavigatorOps) because it is scroll semantics on arbitrary nodes; the navigator
URL-sync is just its current caller.

## `caps::SafeAreaOps` (2)

`apply_safe_area_padding`, `apply_scroll_view_safe_area_inset` (default falls
back to the padding path, same trait)

## `caps::VirtualizerOps` (4) — `: ExternalOps`

`create_virtualizer`, `virtualizer_data_changed`, `release_virtualizer`,
`make_virtualizer_handle`

## `caps::GraphicsOps` (3) — `: ExternalOps`

`create_graphics`, `release_graphics`, `make_graphics_handle`

## `caps::PortalOps` (4) — `: ExternalOps`

`create_portal`, `release_portal`, `set_portal_hidden`, `make_portal_handle`

## `caps::PresenceOps` (3) — `: ViewOps` (placeholder default is a plain view)

`create_presence_placeholder`, `apply_presence`, `make_presence_handle`

## `caps::NavigatorOps` (5) — `: ExternalOps`

`create_navigator`, `release_navigator`, `apply_navigator_slot_style`,
`make_navigator_handle`, `navigator_attach_initial`

## `caps::ExternalOps` (3) ⏳ — kept during transition; dissolves into the scene Registry contract at P7

`create_external`, `release_external`, `missing_primitive_placeholder`
(`#[doc(hidden)]`, default delegates to `create_external` in-trait; the
supertrait target for every `prim-*` placeholder default)

## `caps::DocumentOps` (5) — `: ViewOps` (`create_element` default is a container)

`create_element`, `attach_html_id`, `attach_html_class`, `attach_html_style`,
`register_raw_css`

## `caps::StyleOps` (20)

`apply_style`, `mint_style_class`, `mint_class_for_app`, `apply_styled_states`
(default → `apply_style`, in-trait), `apply_styled_variants` (default →
`apply_styled_states`, in-trait), `mark_container`, `handles_states_natively`,
`token_updates_propagate_via_cascade`, `register_stylesheet`,
`unregister_stylesheet`, `install_tokens`, `update_tokens`, `on_node_unstyled`,
`attach_states`, `set_disabled`, `supports_preminted_styles`,
`apply_default_text_font`, `supports_js_class_bindings`,
`register_reactive_class_binding`, `release_reactive_class_binding`

## `caps::AssetOps` (4)

`register_asset`, `unregister_asset`, `register_typeface`, `unregister_typeface`

## `caps::A11yOps` (3)

`update_accessibility`, `announce_for_accessibility`, `dump_accessibility_tree`

## `caps::AnimationOps` (2)

`set_animated_f32`, `set_animated_color`

## `caps::IntrospectionOps` (8) — dev/robot geometry + native reads + screenshots

`frame`, `absolute_frame`, `device_frame`, `supports_native_introspection`,
`introspect_native`, `note_introspection_root`, `supports_screenshot`,
`capture_screenshot`

## `caps::BatchOps` (3)

`supports_batched_repeat`, `execute_batch`, `execute_batch_with_attach`
(default → `execute_batch` + `Host::insert_many`, via the Host supertrait)

## `caps::WireBindingOps` (9) ⏳ — declarative wire/generator backends only; expected to retire with the adopt-sentinel (design §8)

`note_text_binding`, `note_signal_initial`, `note_when_binding`,
`note_switch_binding`, `note_repeat_binding`, `note_virtualizer_binding`
(default → `note_repeat_binding`, in-trait), `supports_lazy_slot_capture`,
`begin_slot_capture`, `end_slot_capture`

## Not trait methods (out of scope, listed for completeness)

- Free functions and installers in `backend.rs` (`platform()`, `open_url`,
  `set_fullscreen`, `announce`, `install_*`) — thread-local plumbing around the
  trait, not part of it; they move with the flush-driver work, not the split.
- `Platform` inherent methods (`is_apple`, `canonical`, …) — data-type methods.
- The private `Noop*Ops` handle ZSTs — copied into `caps::noop` (they back the
  frozen `make_*_handle` defaults and were not exported by runtime-core).

## Judgment calls on defaults (cross-trait delegation)

Frozen defaults that call other Backend methods were kept as defaults by
declaring the dependency as a supertrait (never moved to the bridge, so future
direct implementors keep today's behavior):

1. Placeholder defaults (`create_image`, `create_icon`, `create_text_input`,
   `create_text_area`, `create_toggle`, `create_slider`,
   `create_activity_indicator`, `create_virtualizer`, `create_graphics`,
   `create_portal`, `create_navigator`) → `: ExternalOps` for
   `missing_primitive_placeholder`.
2. Container-degradation defaults (`create_pressable`, `create_link`,
   `create_presence_placeholder`, `create_element`) → `: ViewOps`.
3. `update_button_label` → `: TextOps` on `ButtonOps` (heaviest coupling in the
   split; accepted because every real backend implements both and the
   alternative — dropping the frozen default — would change direct-implementor
   behavior).
4. `execute_batch_with_attach` uses `Host::insert_many` through the universal
   `: Host` supertrait.
5. `missing_primitive_placeholder`'s local `MissingPrimitive` TypeId differs
   from runtime-core's private one; both are registry-unknown, so dispatch
   behavior is identical (documented at the trait).
