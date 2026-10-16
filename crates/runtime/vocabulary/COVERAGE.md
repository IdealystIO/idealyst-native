# Backend capability coverage

A backend is `runtime_scene::Host` (the structural seam:
[`crates/runtime/scene/src/host.rs`](../scene/src/host.rs)) plus the
capability traits in [`src/caps/`](src/caps). This file lists every
capability trait and its methods. Methods marked **(required)** have no
default; every other method has one, and the defaults degrade rather than
crash:

- the `create_*` method of a widget trait declared `: ExternalOps`
  (`image`, `icon`, `text_input`, `toggle`, `slider`,
  `activity_indicator`, `virtualizer`, `virtual_grid`, `graphics`, `portal`)
  defaults to `ExternalOps::missing_primitive_placeholder` — a visible,
  labeled "unsupported" box;
- a container trait declared `: ViewOps` (`pressable`, `link`, presence
  placeholders, `DocumentOps::create_element`) defaults to `create_view`;
- `ButtonOps::update_button_label` defaults to `TextOps::update_text`, and
  `BatchOps::execute_batch_with_attach` to `execute_batch` +
  `Host::insert_many`.

`caps::AllCaps` (in [`src/caps/mod.rs`](src/caps/mod.rs)) is the bound the
builtin handlers need — every trait below. Each backend implements them in
its `src/newcore.rs`; `tests/caps_conformance.rs` and
`tests/builtin_surface.rs` pin the surface.

Free functions and boot-time installs (`install_scheduler`, the time
source, `open_url`, `announce`, …) are not trait methods: they live in
[`src/backend.rs`](src/backend.rs) (the one-stop backend-author import)
and `runtime_shared::backend`, which documents the required boot order.

## `caps::AppEnvOps` (11) — `: Host`

Mount-time app environment: platform identity, appearance, the self-contained closures `mount(...)` stashes in thread-locals (`open_url` / `set_fullscreen`), and app-level chrome (page metadata, host-surface background, scrollbar theme, app keyboard sink).

`color_scheme`, `platform`, `url_opener`, `fullscreen_setter`, `set_page_metadata`, `set_app_background`, `set_scrollbar_theme`, `set_keyboard_sink`, `set_pointer_sink`, `set_pointer_lock`, `set_cursor_hidden`

## `caps::LifecycleOps` (8) — `: Host`

Build/flush lifecycle: the end-of-build commit, layout scheduling, and the hydration / lazy-chunk policy flags the mount handlers consult.

`finish` **(required)**, `run_layout`, `schedule_layout_pass`, `is_hydrating`, `renders_lazy_chunks`, `annotate_nav_outlet`, `hydrate_nav_screen_begin`, `hydrate_nav_screen_end`

## `caps::ViewOps` (2) — `: Host`

The `view` primitive — the container every other default lowers to.

`create_view` **(required)**, `make_view_handle`

## `caps::InputOps` (6) — `: Host`

Raw input channels installed on already-created nodes: touch, wheel, hover, OS file drag-and-drop, and the focus-preservation mark.

`install_touch_handler`, `claim_touch`, `install_wheel_handler`, `install_hover_handler`, `mark_preserves_focus`, `install_file_drop_handler`

## `caps::PressableOps` (2) — `: ViewOps`

The `pressable` primitive — a tappable container.

`create_pressable`, `make_pressable_handle`

## `caps::TextOps` (12) — `: Host`

The `text` primitive: plain and styled-run creation/update, the batched `*_by_id` FFI fast path, and backend-side reactive text bindings.

`create_text` **(required)**, `create_styled_text`, `update_styled_text`, `update_text` **(required)**, `create_text_with_id`, `update_text_by_id`, `release_text_id`, `supports_js_text_bindings`, `register_reactive_text_binding`, `release_reactive_text_binding`, `notify_signal_text_js`, `make_text_handle`

## `caps::ButtonOps` (3) — `: TextOps`

The `button` primitive.

`create_button` **(required)**, `update_button_label`, `make_button_handle`

## `caps::ImageOps` (6) — `: ExternalOps`

The `image` primitive.

`create_image`, `update_image_src`, `update_image_alt`, `install_image_load_handler`, `install_image_error_handler`, `make_image_handle`

## `caps::IconOps` (6) — `: ExternalOps`

The `icon` primitive (vector path data + stroke animation).

`create_icon`, `update_icon_color`, `update_icon_data`, `update_icon_stroke`, `animate_icon_stroke`, `make_icon_handle`

## `caps::LinkOps` (3) — `: ViewOps`

The `link` primitive (in-app navigation container).

`create_link`, `update_link_url`, `make_link_handle`

## `caps::TextInputOps` (10) — `: ExternalOps`

The `text_input` + `text_area` primitives (single- and multi-line editors share a handler and a controlled-value pattern).

`create_text_input`, `update_text_input_value`, `update_text_input_secure`, `update_text_input_keyboard`, `set_text_input_focus_handler`, `update_text_input_placeholder`, `create_text_area`, `update_text_area_value`, `make_text_input_handle`, `make_text_area_handle`

## `caps::ToggleOps` (3) — `: ExternalOps`

The `toggle` primitive (switch / checkbox).

`create_toggle`, `update_toggle_value`, `make_toggle_handle`

## `caps::SliderOps` (3) — `: ExternalOps`

The `slider` primitive.

`create_slider`, `update_slider_value`, `make_slider_handle`

## `caps::ActivityIndicatorOps` (3) — `: ExternalOps`

The `activity_indicator` primitive (loading spinner).

`create_activity_indicator`, `update_activity_indicator_size`, `make_activity_indicator_handle`

## `caps::ScrollOps` (8) — `: Host`

The `scroll_view` primitive plus generic node scroll-offset access (the navigator substrate's URL-sync scroll snapshot/restore reads any node's offset, so `node_scroll`/`set_node_scroll` live here with the scrolling capability rather than on the navigator).

`create_scroll_view`, `observe_scroll_end`, `node_scroll`, `set_node_scroll`, `apply_scroll_view_bounces`, `apply_scroll_view_always_bounce`, `apply_virtualizer_safe_area_inset`, `make_scroll_view_handle`

## `caps::SafeAreaOps` (3) — `: Host`

Safe-area treatment for opted-in containers (`.safe_area(...)`).

`apply_safe_area_padding`, `apply_scroll_view_safe_area_inset`, `mark_keyboard_avoiding`

## `caps::VirtualizerOps` (4) — `: ExternalOps`

The `flat_list` / `virtualizer` primitive — backend-owned windowed rendering over framework-mounted rows.

`create_virtualizer`, `virtualizer_data_changed`, `release_virtualizer`, `make_virtualizer_handle`

## `caps::GridOps` (4) — `: ExternalOps`

The `virtual_grid` primitive — backend-owned windowed rendering over a TWO-axis grid.

`create_virtual_grid`, `virtual_grid_data_changed`, `release_virtual_grid`, `make_virtual_grid_handle`

## `caps::PortalOps` (4) — `: ExternalOps`

The `overlay` / portal primitive — render children at a window-level target, escaping layout and clipping.

`create_portal`, `release_portal`, `set_portal_hidden`, `make_portal_handle`

## `caps::PresenceOps` (3) — `: ViewOps`

The `presence` primitive — enter/exit transforms around mount and deferred unmount.

`create_presence_placeholder`, `apply_presence`, `make_presence_handle`

## `caps::NavigatorOps` (4) — `: ExternalOps`

Navigator hooks for the registry-dispatched stack and swap navigators: release, slot styling, the imperative handle, and attaching the initial screen.

`release_navigator`, `apply_navigator_slot_style`, `make_navigator_handle`, `navigator_attach_initial`

## `caps::GraphicsOps` (3) — `: ExternalOps`

The `graphics` primitive — a raw GPU-drawable surface.

`create_graphics`, `release_graphics`, `make_graphics_handle`

## `caps::ExternalOps` (3) — `: Host`

Third-party primitives mounted through the scene `Registry` (`create_external` / `release_external`), and `missing_primitive_placeholder` — the labeled "unsupported" box every `create_*` default on an `: ExternalOps` trait degrades to, uniformly, instead of panicking.

`create_external`, `release_external`, `missing_primitive_placeholder`

## `caps::DocumentOps` (7) — `: ViewOps`

Document-backed structural affordances: real HTML tags, id/class/ inline-declaration stamping, and raw CSS registration.

`create_element`, `attach_html_id`, `attach_html_class`, `detach_html_class`, `attach_html_style`, `attach_html_attribute`, `register_raw_css`

## `caps::StyleOps` (23) — `: Host`

The style engine's backend surface: applying resolved rules, class minting for the batched/signal-class paths, declarative state + breakpoint + container overlays, token variables, interaction-state wiring, and per-node style teardown.

`apply_style` **(required)**, `apply_inline_style`, `mint_style_class`, `mint_class_for_app`, `apply_styled_states`, `apply_styled_variants`, `mark_container`, `handles_states_natively`, `token_updates_propagate_via_cascade`, `register_stylesheet`, `unregister_stylesheet`, `install_tokens`, `update_tokens`, `install_theme_palettes`, `on_node_unstyled`, `attach_states`, `set_disabled`, `supports_preminted_styles`, `apply_default_text_font`, `supports_js_class_bindings`, `register_reactive_class_binding`, `release_reactive_class_binding`, `notify_signal_value_js`

## `caps::AssetOps` (4) — `: Host`

Static assets (fonts, images) and typeface families.

`register_asset`, `unregister_asset`, `register_typeface`, `unregister_typeface`

## `caps::A11yOps` (3) — `: Host`

The accessibility surface that does NOT live on `create_*` prop bags: reactive prop-bag replacement, live-region announcements, and the GPU-backend parallel semantics tree.

`update_accessibility`, `announce_for_accessibility`, `dump_accessibility_tree`

## `caps::AnimationOps` (2) — `: Host`

Per-frame animated property writes from `AnimatedValue` listeners.

`set_animated_f32`, `set_animated_color` **(required)**

## `caps::IntrospectionOps` (8) — `: Host`

Dev/robot-facing geometry and introspection: frames in three coordinate systems, native render-tree reads, and real-surface screenshots.

`frame`, `absolute_frame`, `device_frame`, `supports_native_introspection`, `introspect_native`, `note_introspection_root`, `supports_screenshot`, `capture_screenshot`

## `caps::BatchOps` (3) — `: Host`

Batched materialization of static View+Text repeat rows.

`supports_batched_repeat`, `execute_batch`, `execute_batch_with_attach`

## `caps::WireBindingOps` (9) — `: Host`

Declarative binding metadata for generator / wire backends (Roku, remote renderers): text, `when` / `switch`, and repeat / virtualizer binding notes plus lazy slot capture. Effect-driven backends leave every default no-op in place.

`note_text_binding`, `note_signal_initial`, `note_when_binding`, `note_switch_binding`, `note_repeat_binding`, `note_virtualizer_binding`, `supports_lazy_slot_capture`, `begin_slot_capture`, `end_slot_capture`
