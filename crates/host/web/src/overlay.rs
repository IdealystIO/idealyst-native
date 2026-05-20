//! Browser-side implementation of [`render_wgpu::DomOverlay`].
//!
//! Mounts an absolutely-positioned `<div>` over the canvas and
//! maintains `<iframe>` / `<video>` children keyed by the wgpu
//! tree's node identities. The renderer drives:
//!
//! - `begin_frame()` — clear the "seen this frame" set.
//! - `place_iframe(key, url, rect, opacity)` / `place_video(...)`
//!   — find-or-create the matching child, sync its CSS position
//!   and attributes, mark seen.
//! - `end_frame()` — drop children whose key wasn't touched.
//!
//! The overlay wrapper has `pointer-events: none` so the canvas
//! still receives every pointer event that doesn't land on an
//! `<iframe>` / `<video>` interactive surface. Children are
//! `pointer-events: auto` to let the user actually click into
//! the embedded content.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use render_wgpu::{DomOverlay, DomOverlayKey, DomVideoSpec};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, HtmlElement, HtmlIFrameElement, HtmlVideoElement};

thread_local! {
    /// One process-wide rejection sink for `<video>.play()`
    /// Promises. Holding it in a thread-local means we register
    /// it exactly once at first use; reattaching the same
    /// `Function` to subsequent Promises is cheap (no fresh
    /// wasm-bindgen closure allocation per frame) and keeps it
    /// alive across calls.
    static PLAY_REJECT_SINK: Closure<dyn FnMut(JsValue)> =
        Closure::new(|_err: JsValue| {});
}

/// Swallow a rejected `play()` Promise so it doesn't surface as
/// an "Uncaught (in promise)" log on every render tick. The
/// shared `PLAY_REJECT_SINK` closure lives for the lifetime of
/// the wasm instance, so attaching it as a `.catch()` handler
/// doesn't leak anything that needs reaping later.
fn attach_silent_catch(promise: &js_sys::Promise) {
    PLAY_REJECT_SINK.with(|sink| {
        let _ = promise.catch(sink);
    });
}

/// Vertical inset between the canvas's bounding rect and the
/// overlay wrapper. Zero by default so the iframe rects line up
/// with the canvas pixel grid; exposed as a constant for clarity
/// — the wrapper is positioned with `inset: 0` against its
/// offset parent, not against window coords.
const OVERLAY_INSET_PX: i32 = 0;

/// Live entry per DOM child. `kind` distinguishes iframe vs.
/// video so `place_*` can short-circuit when the wrong placement
/// type fires for a key.
struct OverlayChild {
    element: HtmlElement,
    kind: ChildKind,
    /// Last-applied attribute value for cheap diffing. Avoids
    /// thrashing the iframe (re-navigating) or `<video>.src`
    /// (re-loading) when the framework re-emits an unchanged
    /// placement.
    last_src: String,
    last_rect: (f32, f32, f32, f32),
    last_opacity: f32,
    last_muted: bool,
    last_playing: bool,
    last_volume: f32,
}

enum ChildKind {
    Iframe,
    Video,
}

pub struct OverlayManager {
    document: Document,
    wrapper: HtmlElement,
    children: RefCell<HashMap<DomOverlayKey, OverlayChild>>,
    /// Keys touched between `begin_frame` and `end_frame`.
    /// Children not in this set when `end_frame` fires get
    /// removed from the DOM.
    seen: RefCell<HashSet<DomOverlayKey>>,
}

impl OverlayManager {
    /// Build an overlay wrapper sized to fill the canvas's
    /// parent element and attach it to that parent. The wrapper
    /// is `position: absolute; inset: 0; pointer-events: none`;
    /// the canvas keeps receiving pointer events for any space
    /// not covered by an interactive child.
    ///
    /// Returns `None` if the canvas isn't attached to a parent
    /// — fail-soft so the bring-up path doesn't panic before the
    /// `<canvas>` is inserted into the DOM.
    pub fn new(canvas: &web_sys::HtmlCanvasElement) -> Option<Self> {
        let document = canvas.owner_document()?;
        let parent = canvas.parent_element()?;
        // Ensure the parent positions absolute children against
        // itself, not against the document. Without this the
        // overlay rect floats wherever `body` happens to be.
        let parent_html: HtmlElement = parent.clone().dyn_into().ok()?;
        let parent_style = parent_html.style();
        let computed_position = web_sys::window()
            .and_then(|w| w.get_computed_style(&parent).ok().flatten())
            .and_then(|cs| cs.get_property_value("position").ok())
            .unwrap_or_default();
        if computed_position == "static" || computed_position.is_empty() {
            let _ = parent_style.set_property("position", "relative");
        }

        let wrapper: HtmlElement = document
            .create_element("div")
            .ok()?
            .dyn_into()
            .ok()?;
        let style = wrapper.style();
        let _ = style.set_property("position", "absolute");
        let _ = style.set_property("inset", &format!("{0}px", OVERLAY_INSET_PX));
        let _ = style.set_property("pointer-events", "none");
        // Above the canvas, below any author UI the parent
        // element might layer on top. The framework's own
        // overlays paint into the canvas itself, so they remain
        // visually correct relative to iframes / videos (the
        // canvas pixels are below the overlay layer).
        let _ = style.set_property("z-index", "1");
        let _ = style.set_property("overflow", "hidden");
        let _ = parent.append_child(&wrapper);
        Some(Self {
            document,
            wrapper,
            children: RefCell::new(HashMap::new()),
            seen: RefCell::new(HashSet::new()),
        })
    }

    fn ensure_child<F>(
        &self,
        key: DomOverlayKey,
        expected: ChildKind,
        create: F,
    ) -> Option<HtmlElement>
    where
        F: FnOnce(&Document) -> Option<HtmlElement>,
    {
        let mut children = self.children.borrow_mut();
        // If the same key was used previously with a different
        // child kind (a rare case: a node id reused after a
        // primitive swap), tear down the old element so the new
        // one gets a clean attribute baseline.
        let needs_replace = children
            .get(&key)
            .map(|c| {
                !matches!(
                    (&c.kind, &expected),
                    (ChildKind::Iframe, ChildKind::Iframe)
                        | (ChildKind::Video, ChildKind::Video)
                )
            })
            .unwrap_or(false);
        if needs_replace {
            if let Some(old) = children.remove(&key) {
                let _ = self.wrapper.remove_child(&old.element);
            }
        }
        if !children.contains_key(&key) {
            let element = create(&self.document)?;
            // pointer-events: auto so users can interact with
            // the embedded content. The wrapper's `none` keeps
            // surrounding canvas hits from being captured.
            let style = element.style();
            let _ = style.set_property("position", "absolute");
            let _ = style.set_property("pointer-events", "auto");
            let _ = style.set_property("border", "0");
            let _ = self.wrapper.append_child(&element);
            children.insert(
                key,
                OverlayChild {
                    element,
                    kind: expected,
                    last_src: String::new(),
                    last_rect: (f32::NAN, f32::NAN, f32::NAN, f32::NAN),
                    last_opacity: -1.0,
                    last_muted: false,
                    last_playing: false,
                    last_volume: -1.0,
                },
            );
        }
        children.get(&key).map(|c| c.element.clone())
    }

    fn apply_position(
        &self,
        key: DomOverlayKey,
        rect: (f32, f32, f32, f32),
        opacity: f32,
    ) {
        let mut children = self.children.borrow_mut();
        let Some(child) = children.get_mut(&key) else { return };
        if child.last_rect != rect {
            let style = child.element.style();
            let _ = style.set_property("left", &format!("{}px", rect.0));
            let _ = style.set_property("top", &format!("{}px", rect.1));
            let _ = style.set_property("width", &format!("{}px", rect.2));
            let _ = style.set_property("height", &format!("{}px", rect.3));
            child.last_rect = rect;
        }
        if (child.last_opacity - opacity).abs() > 1e-3 {
            let _ = child
                .element
                .style()
                .set_property("opacity", &format!("{opacity}"));
            child.last_opacity = opacity;
        }
    }
}

impl Drop for OverlayManager {
    fn drop(&mut self) {
        // Remove the wrapper (and all children) on teardown.
        // The `WebHostHandle` owns this manager via `Rc`; when
        // the user drops the handle the canvas + overlay both
        // disappear.
        if let Some(parent) = self.wrapper.parent_element() {
            let _ = parent.remove_child(&self.wrapper);
        }
    }
}

impl DomOverlay for OverlayManager {
    fn begin_frame(&self) {
        self.seen.borrow_mut().clear();
    }

    fn place_iframe(
        &self,
        key: DomOverlayKey,
        url: &str,
        rect: (f32, f32, f32, f32),
        opacity: f32,
    ) {
        self.seen.borrow_mut().insert(key);
        let _element = self.ensure_child(key, ChildKind::Iframe, |document| {
            let iframe: HtmlIFrameElement = document
                .create_element("iframe")
                .ok()?
                .dyn_into()
                .ok()?;
            // sandbox + referrerpolicy keep embedded content
            // from poking at the host page. Authors can widen
            // these later if a use case needs it.
            iframe.set_attribute("sandbox", "allow-scripts allow-forms allow-same-origin").ok()?;
            iframe.set_attribute("referrerpolicy", "no-referrer").ok()?;
            iframe.set_attribute("loading", "lazy").ok()?;
            Some(iframe.unchecked_into())
        });
        // Diff src to avoid re-navigating on every frame.
        {
            let mut children = self.children.borrow_mut();
            if let Some(child) = children.get_mut(&key) {
                if child.last_src != url {
                    let _ = child.element.set_attribute("src", url);
                    child.last_src = url.to_string();
                }
            }
        }
        self.apply_position(key, rect, opacity);
    }

    fn place_video(
        &self,
        key: DomOverlayKey,
        spec: DomVideoSpec<'_>,
        rect: (f32, f32, f32, f32),
        opacity: f32,
    ) {
        self.seen.borrow_mut().insert(key);
        let _element = self.ensure_child(key, ChildKind::Video, |document| {
            let video: HtmlVideoElement = document
                .create_element("video")
                .ok()?
                .dyn_into()
                .ok()?;
            video.set_attribute("playsinline", "").ok()?;
            video.set_attribute("preload", "metadata").ok()?;
            // Fit the source's aspect inside the placement rect.
            // `object-fit: contain` matches the iOS / Android
            // <Video> default; if authors want fill, they can
            // override via canvas-side styling in a follow-up.
            let _ = video.style().set_property("object-fit", "contain");
            let _ = video
                .style()
                .set_property("background", "black");
            Some(video.unchecked_into())
        });
        // Sync src / controls flags / playback state. Each gated
        // by an equality check so we don't trigger redundant
        // browser work (re-loading the source, toggling controls
        // chrome, …) on every animation tick.
        {
            let mut children = self.children.borrow_mut();
            let Some(child) = children.get_mut(&key) else { return };
            if child.last_src != spec.src {
                let _ = child.element.set_attribute("src", spec.src);
                child.last_src = spec.src.to_string();
                // Static at-creation flags — replay them
                // whenever the src changes (the browser resets
                // on src swap).
                if spec.autoplay {
                    let _ = child.element.set_attribute("autoplay", "");
                } else {
                    let _ = child.element.remove_attribute("autoplay");
                }
                if spec.loop_playback {
                    let _ = child.element.set_attribute("loop", "");
                } else {
                    let _ = child.element.remove_attribute("loop");
                }
            }
            // Round-trip muted / volume / play-pause / seek
            // through the live media element. The downcast hits
            // the same DOM node we cached as `HtmlElement`.
            if let Ok(media) =
                child.element.clone().dyn_into::<HtmlVideoElement>()
            {
                if child.last_muted != spec.muted {
                    media.set_muted(spec.muted);
                    child.last_muted = spec.muted;
                }
                if (child.last_volume - spec.volume).abs() > 1e-3 {
                    media.set_volume(spec.volume as f64);
                    child.last_volume = spec.volume;
                }
                // Play/pause: drive the call only when the
                // framework's desired state changes. Comparing
                // against `media.paused()` instead would mean we
                // keep calling `play()` every frame while the
                // source is still loading (or while the browser
                // is rejecting autoplay), drowning the console in
                // `NotSupportedError` / `NotAllowedError`. After
                // a one-shot transition the DOM owns the
                // play/pause state; the framework only nudges it
                // again on the next toggle.
                if spec.playing != child.last_playing {
                    if spec.playing {
                        if let Ok(promise) = media.play() {
                            // The play() Promise rejects when
                            // autoplay is blocked or the source
                            // fails to load. Attach a no-op
                            // catch handler so the rejection
                            // doesn't surface as an "Uncaught
                            // (in promise)" entry on every
                            // frame. The user's next interaction
                            // (clicking the controls) gives a
                            // gesture and play() succeeds.
                            attach_silent_catch(&promise);
                        }
                    } else {
                        let _ = media.pause();
                    }
                    child.last_playing = spec.playing;
                }
                if let Some(secs) = spec.seek {
                    media.set_current_time(secs);
                }
            }
        }
        self.apply_position(key, rect, opacity);
    }

    fn end_frame(&self) {
        // Reap stale children: anything in `children` whose key
        // wasn't touched this frame is detached from the DOM
        // and dropped.
        let mut children = self.children.borrow_mut();
        let seen = self.seen.borrow();
        let stale: Vec<DomOverlayKey> = children
            .keys()
            .copied()
            .filter(|k| !seen.contains(k))
            .collect();
        for key in stale {
            if let Some(child) = children.remove(&key) {
                let _ = self.wrapper.remove_child(&child.element);
            }
        }
    }
}
