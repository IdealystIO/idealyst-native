# `pull-refresh`

Pull-to-refresh for any vertically scrolling child, drawn by the platform's
own refresh control.

`Refreshable` wraps one child. It is not a scroll view of its own: it gives
the scroll view its child renders the operating system's refresh control. The
child keeps its own layout and style; the wrapper takes no part in layout on
any backend.

## Usage

```rust
use std::rc::Rc;
use pull_refresh::Refreshable;
use runtime_core::{after_ms_detached, fixed_size, signal, ui, Color};

let loading = signal(false);
let rows = signal(vec!["first".to_string()]);
let reload: Rc<dyn Fn()> = Rc::new(move || {
    loading.set(true);
    after_ms_detached(1500, move || {
        rows.update(|r| { let mut r = r.clone(); r.push("new".into()); r });
        loading.set(false);
    });
});

ui! {
    Refreshable(refreshing = loading, on_refresh = reload, tint = Some(Color("#2563eb".into()))) {
        flat_list(
            data = rows,
            key = |idx, _row: &String| idx as u64,
            size = fixed_size(48.0),
            render = |_idx, row: &String| ui! { text { row.clone() } },
        )
    }
}
```

Install the handler at the boot seam, with the app's other SDK registrations:

```rust
pub fn register_scene_extensions<H: runtime_scene::Host + 'static>(
    registry: &mut runtime_scene::Registry<H>,
) {
    pull_refresh::register(registry);
}
```

## Props

| Prop | Type | Default | |
|---|---|---|---|
| `refreshing` | `bool`, reactive | `false` | Whether a refresh is running. A `Signal<bool>` / `ReadSignal<bool>` / memo all work. |
| `on_refresh` | `Rc<dyn Fn()>` | no-op | Called when the user completes a pull. |
| `tint` | `Option<Color>`, reactive | `None` | The spinner's color as a CSS color (`#rrggbb`, `rgb()`, `rgba()`). `None`, or a color that does not parse, keeps the platform's default. |
| children | one element | | The scrolling content: a `scroll_view`, a `flat_list`, a `virtual_grid`, or a component that renders one. |

### The refreshing contract

When the user completes a pull, the control turns its spinner on, then
`on_refresh` runs. Set `refreshing` to `true` inside `on_refresh` and back to
`false` when the work is done. If `refreshing` is still `false` once
`on_refresh` has returned and its writes are committed, the spinner ends right
away — so a static `refreshing` or an `on_refresh` that forgets it never
leaves a spinner running.

Setting `refreshing` to `true` from anywhere else (an initial load) shows the
spinner too. On iOS the list scrolls down to reveal it when it is at its top.

## Per platform

| Platform | What draws | Notes |
|---|---|---|
| iOS | `UIRefreshControl`, set as `refreshControl` on the first vertical `UIScrollView` in the child's tree (the child itself first) | UIKit draws the spinner, snaps, plays the haptic and holds the content down while refreshing. Text views and sideways scrollers are skipped. `alwaysBounceVertical` is turned on so short content can be pulled, and restored on unmount. |
| Android | androidx `SwipeRefreshLayout` (`RustSwipeRefresh`) around the child | A pull starts only while no scroller in the child's tree is away from its top. The layout clips the spinner to the child's box and refuses touches outside it. |
| web, macOS, Windows, Linux, SSR, GPU, test mocks | nothing | The child renders unchanged; `on_refresh` never fires. Desktop platforms have their own refresh conventions, and the web has none built in. |

The Android SDK declares its dependency itself
(`[package.metadata.idealyst.android].androidx` →
`androidx.swiperefreshlayout:swiperefreshlayout:1.1.0`); `idealyst run
android` links it. No permissions are needed anywhere.

## Limits

- Top edge only.
- iOS looks for the scroll view once, when the `Refreshable` mounts. A child
  that swaps its scroller later (an `if` around the list) keeps the control
  on the first one, or gets none if there was none yet: put the `Refreshable`
  inside the branch instead. Android looks at gesture time.
- A child with no scroll view renders unchanged on iOS. On Android the
  layout still wraps it, so a pull on it refreshes.
- One child. On Android the refresh layout hit-tests and clips to the first.
- `bounces = false` on the scroll view turns UIKit's pull off with it.

## Example

`examples/pull-refresh-demo` — a 200-row `flat_list` and a plain
`scroll_view`, each in a `Refreshable`, with a refresh counter:

```sh
idealyst dev --ios examples/pull-refresh-demo   # or --android, --web
```

## Testing

- `cargo test -p pull-refresh` — the sync / tint / scroller-search policy,
  the reactive binding against a fake control (including a pull the app
  ignores, and a pull delivered after unmount), and the passthrough mount on
  `host-mock`.
- The native legs need a device or simulator: pull each pane of the example,
  check the spinner holds for 1.5 s, a row is added and the counter goes up;
  check rows stay in place while the spinner holds; scroll the list down and
  check a downward drag scrolls rather than refreshes.
