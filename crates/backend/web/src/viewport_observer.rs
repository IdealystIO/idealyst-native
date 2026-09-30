//! Window-resize observer that pushes `window.innerWidth /
//! innerHeight` into `runtime_shared::set_viewport_size`.
//!
//! Author code subscribes via [`runtime_shared::viewport_size()`] from
//! inside an effect / derived. The observer fires on initial install
//! (so the signal has a non-zero value before the first paint) and on
//! every `resize` event after that.
//!
//! Idempotent — re-calling [`install_viewport_observer`] replaces the
//! previous listener with a fresh one. The closure is `forget()`-leaked
//! into the JS heap and lives for the page's lifetime; that matches the
//! existing dev-transport resize listener at
//! [`crate::dev_transport`].


/// Install a `resize` listener on `window` and push the current viewport
/// once immediately so subscribers see a non-zero value on first read.
///
/// Safe to call without a browser context (worker, SSR) — no-ops if
/// `web_glue::dom::window()` returns `None`.
/// True if the mount element already has server-rendered children — the
/// signal to hydrate (adopt) rather than mount fresh.
pub fn page_is_prerendered(mount_selector: &str) -> bool {
    web_glue::dom::window()
        .and_then(|w| w.document())
        .and_then(|d| d.query_selector(mount_selector).ok().flatten())
        .map(|el| el.first_element_child().is_some())
        .unwrap_or(false)
}

/// Read the viewport the page was server-rendered at, from the mount's
/// `data-ssr-viewport="WxH"` attribute (emitted by `backend_ssr`). A
/// hydrating client seeds this BEFORE its first render so the initial
/// tree matches the server's; then [`install_viewport_observer`] pushes
/// the real viewport and reactivity reconciles. `None` if absent/malformed.
pub fn ssr_viewport(mount_selector: &str) -> Option<(f32, f32)> {
    let el = web_glue::dom::window()?
        .document()?
        .query_selector(mount_selector)
        .ok()??;
    let raw = el.get_attribute("data-ssr-viewport")?;
    let (w, h) = raw.split_once('x')?;
    // `parse_f32_plain`, not `str::parse::<f32>()`: this call was the one
    // reachable anchor linking core's dec2flt float-parse tables (~5-6 KB)
    // into every web bundle. The attribute is self-emitted by backend_ssr
    // (`{w}x{h}`, plain integers) — the tiny parser covers it exactly.
    Some((
        runtime_shared::num::parse_f32_plain(w.trim())?,
        runtime_shared::num::parse_f32_plain(h.trim())?,
    ))
}

thread_local! {
    static VIEWPORT_LISTENER: std::cell::RefCell<Option<web_glue::dom::Listener>> =
        const { std::cell::RefCell::new(None) };
}

pub fn install_viewport_observer() {
    let Some(win) = web_glue::dom::window() else { return };

    // Fire once synchronously so the initial value is correct by the
    // time the framework's first render runs.
    push_current_viewport(&win);

    // Page-lifetime scope: held in a thread-local rather than leaked, and
    // replaced (the old one detaching) if installed again.
    let listener = crate::glue_dom::listen(&win, "resize", web_glue::dom::ListenerOptions::default(), move |_| {
        if let Some(win) = web_glue::dom::window() {
            push_current_viewport(&win);
        }
    });
    VIEWPORT_LISTENER.with(|l| *l.borrow_mut() = Some(listener));
}

fn push_current_viewport(win: &web_glue::dom::Window) {
    let w = win.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let h = win.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    runtime_shared::set_viewport_size(runtime_shared::ViewportSize {
        width: w,
        height: h,
    });
}
