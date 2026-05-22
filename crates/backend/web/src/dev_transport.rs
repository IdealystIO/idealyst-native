//! Browser WebSocket transport for the AAS dev-client.
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
}

use dev_client::WireBackend;
use framework_core::{Backend, RafLoop};
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
        };
        if let Ok(bytes) = serde_json::to_vec(&hello) {
            // Send as binary to match the dev server's send format.
            let arr = Uint8Array::from(&bytes[..]);
            let _ = socket_for_open.send_with_u8_array(&arr.to_vec());
        }
    }) as Box<dyn FnMut(JsValue)>);
    socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));

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

    // --- outbound pump --------------------------------------------
    // requestAnimationFrame-driven drain of the outbound channel.
    // Web doesn't have a thread to block on `outbound_rx.recv()`, so
    // we poll once per frame. Sub-16ms latency for event delivery is
    // fine for dev-mode UX.
    let socket_for_pump = socket.clone();
    let outbound_pump = framework_core::raf_loop(move || {
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

/// Print a one-line summary of an incoming AAS command batch into
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
        CreateVideo { .. } => "CreateVideo",
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
        UpdateVideoSrc { .. } => "UpdateVideoSrc",
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
