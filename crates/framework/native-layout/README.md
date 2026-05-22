# native-layout

Flex-layout helper for native backends (iOS, Android, macOS). Wraps
[`taffy`](https://crates.io/crates/taffy) — a pure-Rust flex engine matching
CSS semantics — and translates `framework_core::StyleRules` into Taffy
styles.

The DOM gives the web backend layout for free. UIKit / AppKit / Android
don't, so each native backend builds a parallel layout tree alongside its
native node tree, runs Taffy when the tree is complete, and applies the
resulting frames to its native views.

## Typical backend integration

```rust
use native_layout::{LayoutTree, LayoutNode};

struct MyBackend {
    layout: LayoutTree,
    // (LayoutNode → native view) association is the backend's choice —
    // a Vec, a HashMap keyed by view pointer, or stored alongside the
    // native view in an enum variant.
}

impl Backend for MyBackend {
    fn create_view(&mut self) -> Self::Node {
        let layout = self.layout.new_node();
        let native = make_native_view();
        MyNode::View { view: native, layout }
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        self.layout.add_child(parent.layout(), child.layout());
        attach_native(parent.view(), child.view());
    }

    fn apply_style(&mut self, node: &Self::Node, rules: &Rc<StyleRules>) {
        self.layout.set_style(node.layout(), rules);
        paint_native(node.view(), rules);
    }

    fn finish(&mut self, root: Self::Node) {
        let (w, h) = self.viewport_size();
        self.layout.compute(root.layout(), w, h);
        // walk and apply frames via the backend's own (LayoutNode → view) map
    }
}
```

## Per-backend gotchas worth knowing

These are documented here because they bite the *next* native backend the
same way:

- **`clear_children` must sync with Taffy.** Removing a child from the
  native parent (`removeFromSuperview`, `removeAllViews`, `removeFromParent`)
  without also calling `remove_child` + `mark_dirty(parent)` leaves Taffy
  with a stale child set and a cached parent size — ghost layout. See
  `project_ios_clear_children_taffy_sync` in memory.
- **Intrinsic sizes need `set_measure_fn`.** Native controls with their own
  intrinsic content size (UISwitch, UISlider, TextView, etc.) need a Taffy
  `measure_fn` that returns the platform's intrinsic content size — otherwise
  Taffy lays them out as `0×0` and they fail hit-testing. See
  `project_ios_intrinsic_size_measurer` and `project_android_taffy_layout`
  in memory.
- **`bounds.origin` must be preserved on iOS scroll views.** A naive
  `apply_frames` writes to `frame`, which on UIKit resets `bounds.origin`
  (which is `contentOffset`) — scroll position jumps to 0 on every layout
  pass. See `project_ios_scrollview_bounds_origin`.
- **iOS insert layout discriminator.** `insert()` decides sync vs deferred
  Taffy layout based on `parent.window != nil`, not a `mounted` flag —
  mid-build inserts on floating parents would corrupt cached sizes. See
  `project_ios_insert_layout_discriminator`.
- **Android `setTranslationX/Y` is in device px, not dp.** Taffy frames are
  in dp; convert via `dp_to_px` before calling the View setters. See
  `project_android_setTranslation_device_px`.

If you're writing a new native backend, read `docs/backend.md` first and
then come back here — the gotchas above are what you'll hit if you don't.

## Style translation

The `LayoutTree::set_style(node, rules)` call is the single point where
`framework_core::StyleRules` becomes a Taffy `Style`. Adding new layout
properties to the core style model means extending this translation
alongside whatever native paint changes the backend needs.

Layout-only properties (flex direction, justify, gap, padding, …) go
through Taffy. Paint properties (background color, border, shadow, …) are
the backend's job — this crate has no opinion on them.
