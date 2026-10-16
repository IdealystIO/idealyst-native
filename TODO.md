This is a temporary list of todos so I don't forget.

1. Revisit install theme requirements

## Open gaps

- **macOS button icons** — `create_button_impl` (`crates/backend/macos/src/imp/mod.rs`) builds a real `NSButton` but ignores `leading_icon` / `trailing_icon`; NSButton's single image slot needs a custom container to composite icon + label.
- **Remote (wire) replay of virtualizers** — `apply_create_virtualizer` in `crates/dev/client/src/lib.rs` is a stub, so a `flat_list` / virtualizer in a non-`--local` `idealyst dev` session never registers its node. Since `ac621e29` the client skips the resulting `UnknownNode` commands (logged once) instead of blanking the app, so the symptom is "the virtualized list is missing". `--local` mode is unaffected.

## A11y

### Polish (not blocking)
- Per-backend a11y integration tests for web / iOS / Android / macOS. The shared tables (`runtime_shared::accessibility`), the wire round-trip, and the wgpu/AccessKit path have dedicated tests; the widget backends mostly rely on wire round-trip + build-as-smoke-test.
- `AccessibilityAction` handlers cross the wire as `HandlerId` trampolines and work, but no example app demonstrates an AT-triggered action yet.

### Intentional gaps (forward-compat by design — leave alone unless requirements change)
- Future `Role` variants surface as `WireRole::Unknown` on older replayers → `None`.
- No standalone iOS-shell / AT-SPI bridges: AccessKit covers UIA / AT-SPI / macOS AX through one API, so the GPU desktop path (`crates/gpu-backend/host/winit` with `crates/gpu-backend/host/wgpu-accesskit`) is end-to-end.
