//! `pull-refresh-demo` — the `pull-refresh` SDK's `Refreshable` around the
//! two kinds of scroller an app has.
//!
//! The top half is a 200-row `flat_list` (UICollectionView / RecyclerView);
//! the bottom half a plain `scroll_view`. Pull either one down: its
//! `on_refresh` sets `refreshing`, waits 1.5 s on a timer, appends a row and
//! clears `refreshing`, which ends the spinner. The header counts the
//! refreshes.
//!
//! On iOS and Android the platform draws the control; on web and desktop
//! the lists render unchanged and nothing happens on a pull.

use std::rc::Rc;

use pull_refresh::Refreshable;
use runtime_core::{
    after_ms_detached, fixed_size, signal, stylesheet, ui, Color, Element, SafeAreaSides, Signal,
};

/// How long the pretend reload takes.
const RELOAD_MS: i32 = 1500;

stylesheet! {
    pub Screen<()> {
        base(_t) {
            flex_grow: 1.0,
            background: Color("#f7f8fb".into()),
        }
    }
}

stylesheet! {
    pub Header<()> {
        base(_t) {
            padding: 12,
            gap: 4,
            background: Color("#e5e7eb".into()),
        }
    }
}

stylesheet! {
    pub Pane<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 0,
        }
    }
}

stylesheet! {
    pub Row<()> {
        base(_t) {
            height: 48,
            padding_left: 16,
            justify_content: runtime_core::JustifyContent::Center,
            border_bottom_width: 1.0,
            border_color: Color("#e5e7eb".into()),
        }
    }
}

stylesheet! {
    pub Label<()> {
        base(_t) {
            padding: 8,
            background: Color("#dbeafe".into()),
        }
    }
}

/// `on_refresh` for one pane: show the spinner, wait, add a row, end it.
fn reload(loading: Signal<bool>, rows: Signal<Vec<String>>, refreshes: Signal<u32>) -> Rc<dyn Fn()> {
    Rc::new(move || {
        loading.set(true);
        after_ms_detached(RELOAD_MS, move || {
            rows.update(|r| {
                let mut r = r.clone();
                r.push(format!("Added by refresh {}", r.len() + 1));
                r
            });
            refreshes.update(|n| n + 1);
            loading.set(false);
        });
    })
}

pub fn app() -> Element {
    let refreshes = signal(0u32);
    let list_loading = signal(false);
    let list_rows = signal((1..=200).map(|i| format!("List row {i}")).collect::<Vec<_>>());
    let page_loading = signal(false);
    let page_rows = signal((1..=8).map(|i| format!("Page row {i}")).collect::<Vec<_>>());

    let reload_list = reload(list_loading, list_rows, refreshes);
    let reload_page = reload(page_loading, page_rows, refreshes);
    let tint = Some(Color("#2563eb".into()));

    ui! {
        view(style = Screen(), safe_area = SafeAreaSides::TOP) {
            view(style = Header()) {
                text { move || format!("Refreshes: {}", refreshes.get()) }
                text {
                    move || format!(
                        "List: {} rows · Page: {} rows",
                        list_rows.with(|r| r.len()),
                        page_rows.with(|r| r.len()),
                    )
                }
            }
            view(style = Pane()) {
                text(style = Label()) { "flat_list (200 rows)" }
                Refreshable(refreshing = list_loading, on_refresh = reload_list, tint = tint.clone()) {
                    flat_list(
                        data = list_rows,
                        key = |idx, _row: &String| idx as u64,
                        size = fixed_size(48.0),
                        render = |_idx, row: &String| ui! {
                            view(style = Row()) { text { row.clone() } }
                        },
                        style = Pane(),
                    )
                }
            }
            view(style = Pane()) {
                text(style = Label()) { "scroll_view" }
                Refreshable(refreshing = page_loading, on_refresh = reload_page) {
                    scroll_view(style = Pane()) {
                        for row in page_rows, key = row.clone() {
                            view(style = Row()) { text { row.clone() } }
                        }
                    }
                }
            }
        }
    }
}

/// The Android wrapper's entry (see `examples/README.md`).
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam: installs `Refreshable`'s handler.
pub fn register_scene_extensions<H: runtime_scene::Host + 'static>(
    registry: &mut runtime_scene::Registry<H>,
) {
    pull_refresh::register(registry);
}
