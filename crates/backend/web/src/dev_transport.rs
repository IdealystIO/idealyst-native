//! Browser WebSocket transport for the runtime-server dev-client.
//!
//! Lives here, not in `dev-client`, because every wire-level piece
//! it touches is a web platform implementation: `web_sys::WebSocket`,
//! `ArrayBuffer`, `MessageEvent`, `requestAnimationFrame`-driven
//! outbound pump, etc. `dev-client` exposes the platform-agnostic
//! [`WireBackend`] replay engine; this file connects it to a
//! browser.
//!
//! Lifecycle:
//!   1. [`connect_web`] opens a `web_sys::WebSocket` with
//!      `binaryType = "arraybuffer"` so incoming binary frames
//!      arrive as `ArrayBuffer` (not `Blob`).
//!   2. On `open`, ships an `AppToDev::Hello`.
//!   3. On `message`, decodes the frame and applies it to the
//!      shared [`WireBackend`].
//!   4. A `requestAnimationFrame` pump drains the outbound channel
//!      each tick and forwards events back over the socket.
//!
//! The returned [`WebClientHandle`] owns every closure + the raf
//! loop. Dropping it severs the socket and frees the closures —
//! useful for hot-reload-restart scenarios where the page wants to
//! reconnect to a fresh dev server.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

thread_local! {
    /// Last `rebuilt_at_ms` seen in a `DevToApp::Hello`. Consumed
    /// (cleared) by the first `Commands` apply after Hello, where
    /// we log the end-to-end "change → apply" latency.
    static REBUILT_AT_MS: Cell<Option<u64>> = const { Cell::new(None) };

    /// Session id the server assigned to *this* WebSocket connection.
    /// Set on every Hello so the user can read it from devtools. The
    /// client never picks a session — assignment is server-side
    /// driven by `SessionMode`. Empty string until the first Hello
    /// arrives (or if the server is older than v5).
    static SESSION_ID: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Best-effort human-readable device label for the future
/// session-picker dev tool. Pulls `navigator.userAgent` since web
/// has no equivalent of "iPhone 15 Pro Sim". Falls back to `None`
/// when no window is reachable (worker context, etc.).
fn browser_device_label() -> Option<String> {
    let win = web_sys::window()?;
    let nav = win.navigator();
    nav.user_agent().ok().filter(|s| !s.is_empty())
}

/// Current `window.innerWidth` / `innerHeight` in CSS pixels. The
/// sidecar caches this per session and serves it through
/// `RecordingViewOps::frame(...)` so author code (welcome's planet
/// orbit math, anything reading `page_ref.with(|h| h.frame())`)
/// gets the *client's* viewport instead of None. `None` only when
/// there's no window (worker context) or `innerWidth/Height`
/// reflection failed.
fn browser_viewport() -> Option<wire::WireViewport> {
    let win = web_sys::window()?;
    let w = win.inner_width().ok()?.as_f64()? as f32;
    let h = win.inner_height().ok()?.as_f64()? as f32;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(wire::WireViewport { width: w, height: h })
}

use dev_client::WireBackend;
use runtime_core::{Backend, RafLoop};
use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};
use wire::{AppToDev, DevToApp};

/// Handle returned by [`connect_web`]. Owns the socket + event
/// closures + raf loop. Drop to disconnect.
pub struct WebClientHandle {
    socket: WebSocket,
    // Closures must outlive the WebSocket — JS keeps function refs
    // to them; dropping invalidates the FFI pointer.
    _on_open: Closure<dyn FnMut(JsValue)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_error: Closure<dyn FnMut(Event)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
    _outbound_pump: RafLoop,
}

impl Drop for WebClientHandle {
    fn drop(&mut self) {
        let _ = self.socket.close();
    }
}

/// Open a WebSocket to the dev server and start the message pump.
///
/// `wire` is shared via `Rc<RefCell<...>>` because both the
/// `onmessage` event closure and the outbound raf pump need access
/// to it (the pump doesn't actually need `wire`, but the API stays
/// symmetric with the native transport).
///
/// `on_disconnect` is invoked from a short `setTimeout` after the
/// WebSocket closes. The typical implementation tears down the
/// current `WebBackend` + `WireBackend`, clears the DOM host
/// element, and calls `connect_web` again — giving a no-page-reload
/// auto-reconnect loop.
///
/// Returns immediately after binding the event handlers. The
/// returned [`WebClientHandle`] must be kept alive (typically in a
/// `thread_local!`) for the connection to function.
pub fn connect_web<B: Backend + 'static>(
    url: &str,
    wire: Rc<RefCell<WireBackend<B>>>,
    on_disconnect: Rc<dyn Fn()>,
) -> Result<WebClientHandle, JsValue>
where
    B::Node: 'static,
{
    // Build the outbound channel for this connection and retarget
    // the (persistent) `WireBackend`'s outbound sender at it. Any
    // handler closures the wire built on earlier connects keep
    // working — they hold the same `OutboundSender` wrapper whose
    // inner sender we just swapped.
    let (tx, outbound_rx) = mpsc::channel::<AppToDev>();
    // Hand one sender to the wire (used by event handlers, etc.);
    // keep a clone for the raf pump's per-frame `RequestFrame`
    // injection. mpsc::Sender is `Clone`; both sides drop
    // independently, with the channel staying open until the last
    // sender drops.
    let raf_tx = tx.clone();
    wire.borrow().outbound().set(tx);

    let socket = WebSocket::new(url)?;
    socket.set_binary_type(BinaryType::Arraybuffer);

    // --- on_open --------------------------------------------------
    let socket_for_open = socket.clone();
    let on_open = Closure::wrap(Box::new(move |_evt: JsValue| {
        let hello = AppToDev::Hello {
            app_name: env!("CARGO_PKG_NAME").to_string(),
            color_scheme: wire::WireColorScheme::Auto,
            // Web — tell the server our current URL so it can
            // reconcile its persisted nav stack with what the
            // browser preserved across reload.
            initial_url: web_sys::window()
                .and_then(|w| w.location().pathname().ok()),
            // Self-description for the server's logs and the future
            // session-picker dev tool. Session assignment itself is
            // entirely server-side (see SessionMode on the host).
            identity: wire::ClientIdentity {
                platform: wire::WirePlatform::Web,
                device_label: browser_device_label(),
            },
            // Capture the current window size so the sidecar's
            // per-session viewport is correct from frame zero —
            // welcome's planet-orbit math reads this to recentre.
            // Without it the recorder defaults to the welcome's
            // hardcoded fallback (393×800) and planets anchor at
            // the wrong x on any other browser width.
            viewport: browser_viewport(),
        };
        if let Ok(bytes) = serde_json::to_vec(&hello) {
            // Send as binary to match the dev server's send format.
            let arr = Uint8Array::from(&bytes[..]);
            let _ = socket_for_open.send_with_u8_array(&arr.to_vec());
        }
    }) as Box<dyn FnMut(JsValue)>);
    socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));

    // --- resize listener ------------------------------------------
    // Web only — push a `ViewportChanged` whenever the browser
    // window resizes (devtools toggle, orientation change, dragged
    // edge). The sidecar updates its per-session viewport and the
    // next raf tick's planet/orbit math sees the new size.
    let viewport_tx = raf_tx.clone();
    let on_resize = Closure::wrap(Box::new(move |_evt: web_sys::Event| {
        let Some(v) = browser_viewport() else { return };
        let _ = viewport_tx.send(AppToDev::ViewportChanged {
            width: v.width,
            height: v.height,
        });
    }) as Box<dyn FnMut(web_sys::Event)>);
    if let Some(win) = web_sys::window() {
        let _ = win.add_event_listener_with_callback(
            "resize",
            on_resize.as_ref().unchecked_ref(),
        );
    }
    // Forget the closure so it stays alive for the page lifetime.
    // The connection handle keeps the WS alive; the resize listener
    // outlives one reconnect attempt cycle by design — re-attaching
    // on every connect would leak listeners.
    on_resize.forget();

    // --- on_message -----------------------------------------------
    let wire_for_msg = wire.clone();
    let on_message = Closure::wrap(Box::new(move |evt: MessageEvent| {
        let data = evt.data();
        let bytes = if let Some(buffer) = data.dyn_ref::<ArrayBuffer>() {
            Uint8Array::new(buffer).to_vec()
        } else if let Some(s) = data.as_string() {
            s.into_bytes()
        } else {
            web_sys::console::warn_1(
                &"[dev-client] unsupported WebSocket frame type".into(),
            );
            return;
        };
        let msg: DevToApp = match serde_json::from_slice(&bytes) {
            Ok(m) => m,
            Err(e) => {
                web_sys::console::error_1(
                    &format!("[dev-client] decode failed: {}", e).into(),
                );
                return;
            }
        };
        apply_dev_msg(&wire_for_msg, msg);
    }) as Box<dyn FnMut(MessageEvent)>);
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

    // --- on_error -------------------------------------------------
    // WebSocket's `error` is a plain `Event`, NOT an `ErrorEvent` —
    // it has no `.message`, `.filename`, `.lineno`. Casting to
    // `ErrorEvent` and reading `.message()` returns JS `undefined`,
    // which wasm-bindgen rejects with a non-`catch` panic
    // ("expected a string argument, found undefined"). The close
    // event's `.reason` is the property that actually carries the
    // human-readable disconnect cause, and the close handler below
    // already logs it.
    let on_error = Closure::wrap(Box::new(move |_evt: Event| {
        web_sys::console::error_1(&"[dev-client] websocket error".into());
    }) as Box<dyn FnMut(Event)>);
    socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));

    // --- on_close -------------------------------------------------
    // When the dev server goes away (it likely just restarted itself
    // to pick up source changes), clear the wire's outbound sender
    // so any in-flight events drop instead of going to a dead
    // channel, then schedule the supplied disconnect callback —
    // which typically rebuilds the WebSocket against the new
    // server and retargets `outbound` at the fresh channel.
    let on_disconnect_for_close = on_disconnect.clone();
    let wire_for_close = wire.clone();
    let on_close = Closure::wrap(Box::new(move |evt: CloseEvent| {
        web_sys::console::log_2(
            &"[dev-client] websocket closed:".into(),
            &evt.reason().into(),
        );
        wire_for_close.borrow().outbound().clear();
        let cb = on_disconnect_for_close.clone();
        schedule_callback(100, move || cb());
    }) as Box<dyn FnMut(CloseEvent)>);
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

    // --- outbound pump + animation tick driver --------------------
    // requestAnimationFrame-driven. Two jobs per tick:
    //
    // 1. Inject one `AppToDev::RequestFrame { dt_ms }` so the dev
    //    side advances its animation clock by exactly the wall-clock
    //    elapsed between browser-paint frames. The client is the
    //    source of truth for animation cadence — when the browser
    //    backgrounds the tab and throttles raf, dev-side ticks stop
    //    automatically.
    //
    // 2. Drain anything queued on the outbound channel (user-fired
    //    events, the `RequestFrame` we just queued, …) and ship over
    //    the WebSocket.
    //
    // Order matters: queue `RequestFrame` BEFORE draining so it goes
    // out in the same WebSocket send burst — minimizes the
    // "client→server→client" loop latency to one tick.
    let socket_for_pump = socket.clone();
    let mut last_raf_ms = now_ms();
    let outbound_pump = runtime_core::raf_loop(move || {
        // Skip the entire tick while the socket is still mid-handshake
        // — `send_with_u8_array` would throw `InvalidStateError` and
        // the raf would spam console errors until `onopen` fires.
        // `readyState` 1 = OPEN per the WHATWG spec.
        if socket_for_pump.ready_state() != web_sys::WebSocket::OPEN {
            return;
        }
        let now = now_ms();
        let dt_ms = now.saturating_sub(last_raf_ms) as u32;
        last_raf_ms = now;
        // Clamp the first frame's reported dt (which would otherwise
        // be the full elapsed wall-clock since connect) to one frame
        // budget. Without this the first server tick after connect
        // jumps every animation forward by seconds — visible as
        // animations "skipping the intro" on a slow page load.
        let dt_ms = if dt_ms > 100 { 16 } else { dt_ms };
        let _ = raf_tx.send(AppToDev::RequestFrame { dt_ms });
        // Drain everything currently queued.
        loop {
            match outbound_rx.try_recv() {
                Ok(msg) => {
                    if let Ok(bytes) = serde_json::to_vec(&msg) {
                        if let Err(e) = socket_for_pump.send_with_u8_array(&bytes) {
                            web_sys::console::error_2(
                                &"[dev-client] ws send error:".into(),
                                &e,
                            );
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
    });

    Ok(WebClientHandle {
        socket,
        _on_open: on_open,
        _on_message: on_message,
        _on_error: on_error,
        _on_close: on_close,
        _outbound_pump: outbound_pump,
    })
}

/// Wall-clock now() in ms since Unix epoch, via JS `Date.now()`.
fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

/// Print a one-line summary of an incoming runtime-server command batch into
/// the browser console, then a collapsed group with each command's
/// kind + the most useful payload bits (text content, node ids,
/// handler ids). Lets you see at a glance what the server emitted
/// in response to each interaction.
fn log_command_batch(cmds: &[wire::Command]) {
    use wire::Command;
    if cmds.is_empty() {
        return;
    }
    // Roll up counts per command kind for the headline.
    let mut counts: std::collections::BTreeMap<&'static str, u32> =
        std::collections::BTreeMap::new();
    for c in cmds {
        *counts.entry(command_kind(c)).or_insert(0) += 1;
    }
    let breakdown = counts
        .iter()
        .map(|(k, v)| format!("{}× {}", v, k))
        .collect::<Vec<_>>()
        .join(", ");
    let header = format!("[aas] batch · {} commands · {}", cmds.len(), breakdown);
    web_sys::console::group_collapsed_1(&header.into());
    for c in cmds {
        let line = format_command(c);
        web_sys::console::log_1(&line.into());
    }
    web_sys::console::group_end();

    // Special-case event-shaped commands (UpdateText / UpdateButtonLabel)
    // with a brief top-level line too, so you can see them in the
    // console without expanding the group.
    for c in cmds {
        match c {
            Command::UpdateText { node, content } => {
                web_sys::console::log_1(
                    &format!("[aas]   {} → text {:?}", node, content).into(),
                );
            }
            Command::UpdateButtonLabel { node, label } => {
                web_sys::console::log_1(
                    &format!("[aas]   {} → button label {:?}", node, label).into(),
                );
            }
            _ => {}
        }
    }
}

fn command_kind(c: &wire::Command) -> &'static str {
    use wire::Command::*;
    match c {
        CreateView { .. } => "CreateView",
        CreateText { .. } => "CreateText",
        CreateButton { .. } => "CreateButton",
        CreatePressable { .. } => "CreatePressable",
        CreateReactiveAnchor { .. } => "CreateReactiveAnchor",
        CreateImage { .. } => "CreateImage",
        CreateIcon { .. } => "CreateIcon",
        CreateTextInput { .. } => "CreateTextInput",
        CreateToggle { .. } => "CreateToggle",
        CreateSlider { .. } => "CreateSlider",
        CreateScrollView { .. } => "CreateScrollView",
        CreateActivityIndicator { .. } => "CreateActivityIndicator",
        CreateLink { .. } => "CreateLink",
        CreatePortal { .. } => "CreatePortal",
        CreateExternal { .. } => "CreateExternal",
        CreateTextArea { .. } => "CreateTextArea",
        CreateGraphics { .. } => "CreateGraphics",
        CreateVirtualizer { .. } => "CreateVirtualizer",
        CreateNavigator { .. } => "CreateNavigator",
        CreateTabNavigator { .. } => "CreateTabNavigator",
        CreateDrawerNavigator { .. } => "CreateDrawerNavigator",
        Insert { .. } => "Insert",
        InsertMany { .. } => "InsertMany",
        ClearChildren { .. } => "ClearChildren",
        UpdateText { .. } => "UpdateText",
        UpdateButtonLabel { .. } => "UpdateButtonLabel",
        UpdateImageSrc { .. } => "UpdateImageSrc",
        UpdateIconColor { .. } => "UpdateIconColor",
        UpdateIconStroke { .. } => "UpdateIconStroke",
        AnimateIconStroke { .. } => "AnimateIconStroke",
        UpdateTextInputValue { .. } => "UpdateTextInputValue",
        UpdateTextAreaValue { .. } => "UpdateTextAreaValue",
        UpdateToggleValue { .. } => "UpdateToggleValue",
        UpdateSliderValue { .. } => "UpdateSliderValue",
        SetDisabled { .. } => "SetDisabled",
        RegisterStyle { .. } => "RegisterStyle",
        UnregisterStyle { .. } => "UnregisterStyle",
        ApplyStyle { .. } => "ApplyStyle",
        ApplyStyledStates { .. } => "ApplyStyledStates",
        AttachStates { .. } => "AttachStates",
        OnNodeUnstyled { .. } => "OnNodeUnstyled",
        ApplyPresence { .. } => "ApplyPresence",
        NavigatorAttachInitial { .. } => "NavigatorAttachInitial",
        NavigatorPush { .. } => "NavigatorPush",
        NavigatorPop { .. } => "NavigatorPop",
        NavigatorReplace { .. } => "NavigatorReplace",
        NavigatorReset { .. } => "NavigatorReset",
        NavigatorMountTab { .. } => "NavigatorMountTab",
        DrawerAttachSidebar { .. } => "DrawerAttachSidebar",
        AttachNavigatorLayout { .. } => "AttachNavigatorLayout",
        OpenDrawer { .. } => "OpenDrawer",
        CloseDrawer { .. } => "CloseDrawer",
        ToggleDrawer { .. } => "ToggleDrawer",
        ApplyNavigatorHeaderStyle { .. } => "ApplyNavigatorHeaderStyle",
        ApplyNavigatorTitleStyle { .. } => "ApplyNavigatorTitleStyle",
        ApplyNavigatorButtonStyle { .. } => "ApplyNavigatorButtonStyle",
        ApplyNavigatorBodyStyle { .. } => "ApplyNavigatorBodyStyle",
        ApplyDrawerSidebarStyle { .. } => "ApplyDrawerSidebarStyle",
        ApplyDrawerScrimStyle { .. } => "ApplyDrawerScrimStyle",
        ApplyTabBarStyle { .. } => "ApplyTabBarStyle",
        ApplyTabIconStyle { .. } => "ApplyTabIconStyle",
        ApplyTabLabelStyle { .. } => "ApplyTabLabelStyle",
        VirtualizerDataChanged { .. } => "VirtualizerDataChanged",
        VirtualizerAttachItem { .. } => "VirtualizerAttachItem",
        NavigatorSelect { .. } => "NavigatorSelect",
        Finish { .. } => "Finish",
        ReleaseNode { .. } => "ReleaseNode",
        InstallThemeVariables { .. } => "InstallThemeVariables",
        RegisterAsset { .. } => "RegisterAsset",
        UnregisterAsset { .. } => "UnregisterAsset",
        RegisterTypeface { .. } => "RegisterTypeface",
        UnregisterTypeface { .. } => "UnregisterTypeface",
        UpdateAccessibility { .. } => "UpdateAccessibility",
        AnnounceForAccessibility { .. } => "AnnounceForAccessibility",
        SetAnimatedF32 { .. } => "SetAnimatedF32",
        SetAnimatedColor { .. } => "SetAnimatedColor",
        ApplySafeAreaPadding { .. } => "ApplySafeAreaPadding",
        ApplyScrollViewSafeAreaInset { .. } => "ApplyScrollViewSafeAreaInset",
    }
}

fn format_command(c: &wire::Command) -> String {
    use wire::Command::*;
    match c {
        CreateView { id, .. } => format!("CreateView {}", id),
        CreateText { id, content, .. } => format!("CreateText {} {:?}", id, content),
        CreateButton { id, label, .. } => format!("CreateButton {} {:?}", id, label),
        Insert { parent, child } => format!("Insert {} → {}", child, parent),
        UpdateText { node, content } => format!("UpdateText {} {:?}", node, content),
        UpdateButtonLabel { node, label } => {
            format!("UpdateButtonLabel {} {:?}", node, label)
        }
        ApplyStyle { node, style } => format!("ApplyStyle {} ← {}", node, style),
        RegisterStyle { id, .. } => format!("RegisterStyle {}", id),
        ReleaseNode { node } => format!("ReleaseNode {}", node),
        Finish { root } => format!("Finish {}", root),
        other => format!("{}", command_kind(other)),
    }
}

/// Schedule a one-shot Rust callback to run `delay_ms` from now via
/// `window.setTimeout`. Used by the close handler to give the
/// browser a moment for the dev server to finish restarting before
/// we attempt to reconnect.
fn schedule_callback<F: FnOnce() + 'static>(delay_ms: i32, f: F) {
    let Some(window) = web_sys::window() else { return };
    let cb = Closure::once_into_js(move || f());
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        cb.as_ref().unchecked_ref(),
        delay_ms,
    );
}

fn apply_dev_msg<B: Backend + 'static>(wire: &Rc<RefCell<WireBackend<B>>>, msg: DevToApp)
where
    B::Node: 'static,
{
    match msg {
        DevToApp::Hello {
            protocol_version,
            rebuilt_at_ms,
            session,
            ..
        } => {
            if protocol_version != wire::PROTOCOL_VERSION {
                web_sys::console::warn_1(
                    &format!(
                        "[dev-client] protocol version mismatch: dev={}, app={}",
                        protocol_version,
                        wire::PROTOCOL_VERSION
                    )
                    .into(),
                );
            }
            // Stash the rebuild timestamp so the next Commands
            // apply can log the end-to-end latency.
            REBUILT_AT_MS.with(|slot| slot.set(rebuilt_at_ms));
            // Log + remember the session id the server assigned. Lets
            // the user verify in devtools that two tabs ended up on
            // the same / different sessions as intended.
            if !session.is_empty() {
                web_sys::console::log_1(
                    &format!("[dev-client] session: {}", session).into(),
                );
            }
            SESSION_ID.with(|s| *s.borrow_mut() = session);
        }
        DevToApp::Commands(cmds) => {
            log_command_batch(&cmds);
            if let Err(e) = wire.borrow_mut().apply_batch(cmds) {
                web_sys::console::error_1(
                    &format!("[dev-client] replay error: {:?}", e).into(),
                );
            }
            // First apply after a Hello carrying a rebuild
            // timestamp → log the latency.
            REBUILT_AT_MS.with(|slot| {
                if let Some(started) = slot.take() {
                    let now = now_ms();
                    let elapsed = now.saturating_sub(started);
                    web_sys::console::log_1(
                        &format!(
                            "[dev-client] hot-reload latency: change detected → apply = {}ms",
                            elapsed
                        )
                        .into(),
                    );
                }
            });
        }
        DevToApp::Rebuilding => {
            web_sys::console::log_1(&"[dev-client] dev is rebuilding…".into());
        }
        DevToApp::Error { message } => {
            web_sys::console::error_2(&"[dev-client] dev error:".into(), &message.into());
        }
        DevToApp::ThemeChanged { .. } => {
            // Theme application is a follow-up.
        }
    }
}
