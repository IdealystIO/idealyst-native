//! Which capture answers the web Robot's `screenshot` verb.
//!
//! A page has two ways to picture itself, and the verb's `source` argument
//! picks between them:
//!
//! - `dom` — [`robot_screenshot`](crate::robot_screenshot): copy the DOM
//!   into an SVG and rasterize it. Needs nothing from the user; misses what
//!   the DOM copy cannot carry (plugin content such as a PDF viewer, WebGL
//!   canvases, exact font rendering).
//! - `tab` — a frame of the tab's own screen-share stream: the browser's
//!   real pixels. The stream exists only after the user shared the tab from
//!   the dev overlay's button (a browser opens `getDisplayMedia` only from a
//!   click), so until then this answers with what to ask the user, and the
//!   overlay lights the button up.
//! - `auto` (or none, or `client`) — `tab` while the tab is shared, else
//!   `dom`.
//!
//! `cdp` — the browser's own screenshot over the DevTools Protocol — is the
//! relay's to serve (`robot_relay::cdp`): a page cannot reach its browser's
//! debugging port. A `cdp` request that reaches the page means the relay
//! could not, and the page says so.
//!
//! Every success names the `source` that produced it.
//!
//! The tab stream lives in the dev overlay's script (`dev-http`'s
//! `tab_capture.js`), not here: the button that starts it is there, and the
//! script runs on a page whose wasm failed to build as well. This module
//! reaches it through [`TAB_CAPTURE_GLOBAL`].

use std::rc::Rc;
use std::cell::RefCell;
use web_glue::JsValue;

/// The page global the dev overlay's tab-capture controller sits on.
/// `dev-http`'s `TAB_CAPTURE_GLOBAL` names the same; its `tab_capture` test
/// holds the two together.
pub const TAB_CAPTURE_GLOBAL: &str = "__idealyst_tab_capture";

/// A finished capture.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub png_base64: String,
    pub width: u32,
    pub height: u32,
    /// `dom` or `tab`.
    pub source: &'static str,
}

impl Shot {
    /// The bridge's `ok` payload.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "png_base64": self.png_base64,
            "width": self.width,
            "height": self.height,
            "source": self.source,
        })
    }
}

/// What a `source` argument asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Dom,
    Tab,
    Auto,
}

impl Source {
    pub(crate) fn parse(source: Option<&str>) -> Result<Source, String> {
        match source {
            None | Some("auto") | Some("client") => Ok(Source::Auto),
            Some("dom") => Ok(Source::Dom),
            Some("tab") => Ok(Source::Tab),
            Some("cdp") => Err(
                "cdp screenshot unavailable: the dev relay serves `cdp`, and this page was reached \
                 without one. Use source \"tab\" or \"dom\""
                    .into(),
            ),
            Some(other) => Err(format!(
                "unknown screenshot source \"{other}\" for a web app: use \"auto\", \"cdp\", \"tab\" or \"dom\""
            )),
        }
    }
}

/// The error a `tab` request gets while the tab is not shared. The agent
/// reads it, so it says what the user has to do.
pub(crate) const NOT_SHARED: &str = "the user has not shared this tab with the agent. The dev \
    overlay now shows \"Agent asks to see this tab\" in the page's bottom-right corner: ask the \
    user to click it and allow sharing this tab, then call screenshot again. Source \"dom\" works \
    without them, rendered from a copy of the DOM (less exact: canvases drawn with WebGL, PDF \
    viewers and some font rendering can differ)";

/// Capture with `source` and hand `done` the result. Async: both captures
/// finish after an image or a video frame loads.
pub fn capture(source: Option<&str>, done: Box<dyn FnOnce(Result<Shot, String>)>) {
    let source = match Source::parse(source) {
        Ok(s) => s,
        Err(e) => return done(Err(e)),
    };
    let controller = tab_controller();
    let shared = controller.as_ref().is_some_and(is_sharing);
    match (source, controller) {
        (Source::Dom, _) | (Source::Auto, None) => dom(done),
        (Source::Auto, Some(_)) if !shared => dom(done),
        (Source::Tab, None) => done(Err(
            "tab capture needs the idealyst dev overlay, which this page does not have (it is \
             injected by `idealyst dev`). Use source \"dom\""
                .into(),
        )),
        (Source::Tab, Some(c)) if !shared => {
            let _ = c.call_method("request", &[]);
            done(Err(NOT_SHARED.into()));
        }
        (Source::Tab | Source::Auto, Some(c)) => tab(c, source == Source::Auto, done),
    }
}

fn dom(done: Box<dyn FnOnce(Result<Shot, String>)>) {
    crate::robot_screenshot::capture(Box::new(move |res| {
        done(res.map(|(png_base64, width, height)| Shot { png_base64, width, height, source: "dom" }))
    }));
}

/// The overlay's controller, when this page has one.
fn tab_controller() -> Option<JsValue> {
    JsValue::global().get(TAB_CAPTURE_GLOBAL).ok().filter(|c| c.is_object())
}

fn is_sharing(controller: &JsValue) -> bool {
    controller.call_method("active", &[]).ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

/// A frame from the shared tab. With `fall_back` (an `auto` request), a
/// grab that fails — the user stopped sharing between the check and the
/// frame — is answered by the DOM capture instead.
fn tab(controller: JsValue, fall_back: bool, done: Box<dyn FnOnce(Result<Shot, String>)>) {
    let promise = match controller.call_method("grab", &[]) {
        Ok(p) => p,
        Err(e) => return finish_tab(Err(format!("tab capture failed: {}", e.message())), fall_back, done),
    };
    let done = Rc::new(RefCell::new(Some(done)));
    web_glue::spawn_local(async move {
        let result = match web_glue::JsFuture::new(&promise).await {
            Ok(frame) => read_frame(&frame),
            Err(e) => Err(format!("tab capture failed: {}", e.message())),
        };
        if let Some(done) = done.borrow_mut().take() {
            finish_tab(result, fall_back, done);
        }
    });
}

fn finish_tab(result: Result<Shot, String>, fall_back: bool, done: Box<dyn FnOnce(Result<Shot, String>)>) {
    match result {
        Err(_) if fall_back => dom(done),
        other => done(other),
    }
}

/// The controller's `{png_base64, width, height}`.
fn read_frame(frame: &JsValue) -> Result<Shot, String> {
    let field = |k: &str| frame.get(k).ok();
    let png_base64 = field("png_base64").and_then(|v| v.as_string()).filter(|s| !s.is_empty());
    let size = |k: &str| field(k).and_then(|v| v.as_f64()).map(|n| n as u32);
    match (png_base64, size("width"), size("height")) {
        (Some(png_base64), Some(width), Some(height)) if width > 0 && height > 0 => {
            Ok(Shot { png_base64, width, height, source: "tab" })
        }
        _ => Err("tab capture returned no frame".into()),
    }
}

#[cfg(test)]
mod tests {
    //! `#[wasm_bindgen_test]`: backend-web only builds for wasm32. Run as
    //! the `robot_transport` tests are (see that file's footer).
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn sources_parse_and_cdp_names_the_alternatives() {
        assert_eq!(Source::parse(None), Ok(Source::Auto));
        assert_eq!(Source::parse(Some("client")), Ok(Source::Auto), "the MCP's real-client source");
        assert_eq!(Source::parse(Some("dom")), Ok(Source::Dom));
        assert_eq!(Source::parse(Some("tab")), Ok(Source::Tab));
        let cdp = Source::parse(Some("cdp")).unwrap_err();
        assert!(cdp.contains("\"tab\"") && cdp.contains("\"dom\""), "{cdp}");
        assert!(Source::parse(Some("replay")).unwrap_err().contains("unknown screenshot source"));
    }

    /// A fake overlay controller: `active` answers `sharing`, `grab`
    /// resolves a 2×1 frame, `request` counts.
    fn install_fake_controller(sharing: bool) -> JsValue {
        let make = web_glue::js::Function::new_with_args(
            "sharing",
            "var c = { requests: 0, grabs: 0 };
             c.active = function () { return sharing; };
             c.request = function () { c.requests++; };
             c.grab = function () { c.grabs++; return Promise.resolve({ png_base64: 'iVBOR', width: 2, height: 1 }); };
             return c;",
        );
        let controller = make.call1(&JsValue::UNDEFINED, &JsValue::from_bool(sharing)).unwrap();
        JsValue::global().set(TAB_CAPTURE_GLOBAL, &controller).unwrap();
        controller
    }

    async fn run(source: Option<&'static str>) -> Result<Shot, String> {
        let slot: Rc<RefCell<Option<Result<Shot, String>>>> = Rc::default();
        let sink = slot.clone();
        capture(source, Box::new(move |r| *sink.borrow_mut() = Some(r)));
        for _ in 0..50 {
            if let Some(r) = slot.borrow_mut().take() {
                return r;
            }
            let p = web_glue::js::Promise::resolve(&JsValue::UNDEFINED);
            let _ = web_glue::JsFuture::new(&p).await;
        }
        panic!("capture never finished");
    }

    #[wasm_bindgen_test]
    async fn a_shared_tab_answers_tab_and_auto() {
        let c = install_fake_controller(true);
        let shot = run(Some("tab")).await.unwrap();
        assert_eq!((shot.source, shot.width, shot.height), ("tab", 2, 1));
        assert_eq!(run(None).await.unwrap().source, "tab", "auto prefers the shared tab");
        assert_eq!(c.get("grabs").unwrap().as_f64(), Some(2.0));
        let _ = JsValue::global().set(TAB_CAPTURE_GLOBAL, &JsValue::UNDEFINED);
    }

    /// The agent asked to see the tab before the user shared it: the
    /// answer says what to ask the user, and the overlay is told to ask.
    #[wasm_bindgen_test]
    async fn an_unshared_tab_asks_the_user_through_the_overlay() {
        let c = install_fake_controller(false);
        let err = run(Some("tab")).await.unwrap_err();
        assert_eq!(err, NOT_SHARED);
        assert_eq!(c.get("requests").unwrap().as_f64(), Some(1.0), "the overlay's button is lit");
        assert_eq!(c.get("grabs").unwrap().as_f64(), Some(0.0));
        let _ = JsValue::global().set(TAB_CAPTURE_GLOBAL, &JsValue::UNDEFINED);
    }

    #[wasm_bindgen_test]
    async fn a_page_without_the_overlay_cannot_share_its_tab() {
        let _ = JsValue::global().set(TAB_CAPTURE_GLOBAL, &JsValue::UNDEFINED);
        let err = run(Some("tab")).await.unwrap_err();
        assert!(err.contains("dev overlay"), "{err}");
    }
}
