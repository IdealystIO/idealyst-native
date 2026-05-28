# backend-ios-mobile

iOS backend (phone form factor). Builds UIKit views via
[`objc2`](https://crates.io/crates/objc2). The `tv` sibling crate is a stub
for tvOS; the shared core (registry, scheduler) lives in
[`../core`](../core) and is reused across mobile/tv variants.

## Bootstrap: every iOS host must do this

```rust
backend_ios_mobile::install_scheduler();
let backend = IosBackend::new(...);
backend_ios_mobile::install_global_self(&backend); // for AnimatedValue::bind
```

- **`install_scheduler`** wires `runtime_core::scheduling::after_ms` /
  `schedule_microtask` / `raf` to `dispatch_after` / `dispatch_async(main)` /
  `CADisplayLink`.
- **`install_global_self`** stores a `Weak<RefCell<IosBackend>>` so animated
  property writes can find the backend without threading it through
  closures. Without it, `AnimatedValue::bind` silently no-ops.

Call `runtime_core::mount(backend, app)`, not `render(backend, app())`.
See `project_mount_vs_render` in memory.

## Layout

Layout is driven through [`runtime-layout`](../../../framework/runtime-layout)
(Taffy). Each `IosNode` carries its UIKit view *and* its Taffy `LayoutNode`;
the `finish` hook runs `LayoutTree::compute` against the root and walks the
tree applying frames.

## UIKit quirks the backend works around

These are sharp edges of UIKit that *will* bite you if you copy this
backend's structure but skip the workarounds:

- **`bounds.origin` vs `frame.origin`.** UIKit's `UIScrollView.contentOffset`
  is just `bounds.origin`. A naive `apply_frames` that writes to `frame`
  resets scroll position to 0 on every layout pass. `apply_frames` preserves
  `bounds.origin` for scroll views. See `project_ios_scrollview_bounds_origin`.
- **Intrinsic content size for native controls.** `UISwitch`, `UISlider`,
  `UITextField`, etc. have an intrinsic content size UIKit reports through
  `intrinsicContentSize`. Taffy doesn't know about it. Each control's
  `create_*` registers a Taffy `measure_fn` that returns the platform's
  intrinsic size. Without it, the control gets laid out as `0×0` and fails
  hit-testing. See `project_ios_intrinsic_size_measurer`.
- **`clear_children` + Taffy sync.** `clear_children` must call `remove_child`
  on Taffy and `mark_dirty(parent)` before/after `removeFromSuperview`, or
  stale Taffy child-set + cached parent size produces ghost layout. See
  `project_ios_clear_children_taffy_sync`.
- **`insert` sync-vs-deferred discriminator.** `insert()` decides whether to
  recompute Taffy layout synchronously based on `parent.window != nil`, not
  a `mounted` flag. Inserting into a floating parent and computing layout
  early would corrupt cached sizes. See `project_ios_insert_layout_discriminator`.
- **`UIImageView` tinting under `UINavigationController`.** Colored PNGs
  render as black silhouettes unless `imageWithRenderingMode:Always­Original`
  AND `setTintAdjustmentMode:Normal` are both pinned. See
  `project_ios_uiimageview_tinting`.
- **`cornerRadius` clamping.** UIKit's `CALayer.cornerRadius` renders nothing
  when `cornerRadius > min(width, height) / 2`. The backend clamps using
  `style.width` / `style.height` before writing the layer. See
  `project_ios_cornerradius_unclamped`.
- **Gradient frame sync.** `CAGradientLayer` is added as a sublayer; its
  frame doesn't follow the host view automatically. The layout pass
  synchronizes gradient frames after Taffy computes the host's frame. See
  `project_gradient_native`.

If you're patching a primitive on this backend and you find yourself
adding a per-platform hack to the *call site* (in runtime-core or in
author code), stop. The fix belongs in this backend. See
`feedback_backend_owns_rendering` in memory and the project CLAUDE.md §7.

## Element coverage status

Not all primitives are implemented yet. As of this README, the iOS backend
is missing:

- **`create_virtualizer`**: no `UICollectionView`-backed list yet, even
  though the framework's `FlatList` / `Virtualizer` works on web and Android.

The root README's per-backend matrix is the source of truth for parity.

## File layout

- **`src/imp/`**: the real backend. Compiled under `target_os = "ios"`.
- **`src/stub.rs`**: type-checking stub for cross-compile from non-iOS
  hosts. Lets `cargo check` work workspace-wide from a Linux laptop.
- **`runtime_kotlin` is not relevant here**: iOS doesn't need a JVM-side
  runtime the way Android does (Android needs Kotlin trampolines for
  `View.OnClickListener` and `ValueAnimator`; UIKit's blocks + `@selector`
  bridge through objc2 directly).

## Building from Xcode

This repo's iOS examples are built via Xcode, not `cargo build` directly.
See `feedback_build_from_xcode` in memory.

The `runtime_ios_files` metadata in `Cargo.toml` declares the Swift/ObjC
files that need to be copied into the Xcode project; the `idealyst` CLI
reads it via `cargo metadata` when scaffolding / building.
