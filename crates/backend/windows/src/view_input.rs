//! Per-view raw input: `on_touch`, `on_wheel` and `on_hover` for painted
//! nodes.
//!
//! Views have no HWNDs, so every mouse message lands on the host window.
//! The host forwards them here; each one is routed along the hit chain
//! (`scene::hit_chain`: the target, then its ancestors) per the
//! framework's dispatch model (`runtime_shared::touch`):
//!
//! - **Touch.** The mouse is one pointer (`TouchId` [`MOUSE_TOUCH`]); the
//!   button that pressed is reported through `pointer_button()`, so
//!   recognizers ignore a right or middle press as they do elsewhere.
//!   A press offers `Began` to each handler on the chain, deepest first,
//!   until one consumes; that handler keeps the gesture. The host takes
//!   mouse capture for it, which is what keeps `Moved` / `Ended` arriving
//!   once the drag leaves the view or the window (UIKit `touchesMoved:`,
//!   AppKit `mouseDragged:` and web `setPointerCapture` give the same). A
//!   lost capture (another window took it, Alt-Tab) ends the gesture as
//!   `Cancelled`. Unpressed motion is `Hovered`, bubbled the same way. A
//!   pressable on the chain below a handler owns the press: the chain
//!   stops there, as on macOS where the pressable's `mouseDown:` does not
//!   call `super`.
//! - **Wheel.** Offered deepest first until one consumes; a consumed tick
//!   does not also scroll a scroll view (web `preventDefault`). Ctrl+wheel,
//!   which is also what a precision touchpad pinch sends to a window
//!   without DirectManipulation, is [`WheelKind::Zoom`].
//! - **Hover.** Every node on the chain with a handler is hovered (nested
//!   views stay hovered as the pointer moves into a child, like
//!   `mouseenter`); enter / leave fire on the difference. The host calls
//!   [`mouse_left`] on `WM_MOUSELEAVE`.
//!
//! - **File drop.** An OS file drag over the window (`crate::file_drop`,
//!   the window's `IDropTarget`) offers `Entered` along the chain, deepest
//!   first, on every move; the first view that accepts is the target, and a
//!   view that stops being it gets `Exited` (after the new one's `Entered`,
//!   as web's `dragenter` precedes `dragleave`). The release delivers
//!   `Dropped` to the target only.
//!
//! Handlers write signals whose effects re-borrow the backend, so every
//! entry point collects what to call under a short borrow and calls it
//! after releasing. A backend already borrowed (a handler re-entering the
//! message loop) drops the message rather than panicking.
//!
//! Positions are LOGICAL pixels (the host passes device pixels, as the
//! message carries them).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use runtime_shared::{
    DroppedFile, FileDropEvent, FileDropHandler, FileDropPhase, set_pointer_button, set_pointer_modifiers, HoverHandler, PointerButton, PointerModifiers,
    TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint, TouchResponse, WheelEvent,
    WheelHandler, WheelKind,
};

use crate::{scene, NodeKind, WindowsBackend};

/// The mouse's touch id: one pointer, so one id for every gesture.
pub const MOUSE_TOUCH: TouchId = TouchId(0);

/// Logical pixels one wheel unit scrolls (`WHEEL_DELTA` = 120 units per
/// notch → 40 px, the OS's three-line default). The same conversion the
/// host uses for scroll views, so a wheel handler sees the distance a
/// scroll view would have moved.
pub const WHEEL_PX_PER_UNIT: f32 = 40.0 / 120.0;

/// Zoom per pixel of Ctrl+wheel, `scale = exp(-dy * k)`, and the
/// per-event clamp that keeps one mouse notch to a ~13% step while a
/// touchpad pinch (many small deltas) is unaffected. The same values as
/// `backend_web::primitives::wheel`, so a pinch zooms alike in the browser
/// and the native app.
const ZOOM_PER_WHEEL_PX: f32 = 0.01;
const MAX_ZOOM_WHEEL_PX: f32 = 12.0;

/// Handler tables + gesture state, owned by the backend.
#[derive(Default)]
pub(crate) struct ViewInput {
    pub(crate) touch: HashMap<u64, TouchHandler>,
    pub(crate) wheel: HashMap<u64, WheelHandler>,
    pub(crate) hover: HashMap<u64, HoverHandler>,
    pub(crate) file_drop: HashMap<u64, FileDropHandler>,
    /// The view that accepted the OS file drag currently over the window.
    drag_target: Option<u64>,
    /// The handler that consumed the current press.
    active: Option<ActiveTouch>,
    /// Nodes currently hovered (with a hover handler).
    hovered: Vec<u64>,
}

struct ActiveTouch {
    node: u64,
    handler: TouchHandler,
    /// The button that started the gesture: every event of the gesture
    /// reports it, and only its release ends it.
    button: PointerButton,
}

impl ViewInput {
    /// A node left the tree: drop its handlers. An in-flight gesture on it
    /// is cancelled by the next event that finds the node gone.
    pub(crate) fn forget(&mut self, id: u64) {
        self.touch.remove(&id);
        self.wheel.remove(&id);
        self.hover.remove(&id);
        self.file_drop.remove(&id);
        if self.drag_target == Some(id) {
            self.drag_target = None;
        }
        self.hovered.retain(|h| *h != id);
    }
}

/// A touch handler on the hit chain, gathered under one borrow.
struct Target {
    node: u64,
    handler: TouchHandler,
    origin: (f32, f32),
}

fn timestamp_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

fn touch_event(phase: TouchPhase, point: (f32, f32), origin: (f32, f32)) -> TouchEvent {
    TouchEvent {
        id: MOUSE_TOUCH,
        phase,
        position: TouchPoint::new(point.0 - origin.0, point.1 - origin.1),
        window_position: TouchPoint::new(point.0, point.1),
        timestamp_ns: timestamp_ns(),
        force: None,
    }
}

/// Calls `handler` with the button and modifiers the dispatch model's side
/// channels report (`pointer_button()`, `pointer_modifiers()`).
fn deliver(handler: &TouchHandler, ev: &TouchEvent, button: PointerButton, mods: PointerModifiers) -> TouchResponse {
    set_pointer_button(button);
    set_pointer_modifiers(mods);
    handler(ev)
}

/// Touch handlers on the chain under `point`, deepest first. With
/// `stop_at_pressable`, a pressable below every remaining handler ends
/// the list (it owns presses).
fn touch_targets(b: &WindowsBackend, point: (f32, f32), stop_at_pressable: bool) -> Vec<Target> {
    let mut out = Vec::new();
    for n in scene::hit_chain(b, point.0, point.1) {
        if let Some(h) = b.view_input.touch.get(&n.id) {
            out.push(Target { node: n.id, handler: h.clone(), origin: n.origin });
        } else if stop_at_pressable
            && matches!(b.nodes.get(&n.id).map(|m| &m.kind), Some(NodeKind::View(v)) if v.on_click.is_some())
        {
            break;
        }
    }
    out
}

/// A mouse button went down at device `(x, y)`. Returns whether a handler
/// took the press — the host then captures the mouse and, for the left
/// button, fires no pressable on the release. A second button pressed
/// during a gesture is not a new gesture (one pointer, one gesture).
pub fn mouse_down(
    cell: &RefCell<WindowsBackend>,
    x: f32,
    y: f32,
    button: PointerButton,
    mods: PointerModifiers,
) -> bool {
    let Ok(b) = cell.try_borrow() else { return false };
    if b.view_input.active.is_some() {
        return false;
    }
    let point = b.dpi.point_to_logical(x, y);
    let targets = touch_targets(&b, point, true);
    drop(b);
    for t in targets {
        let r = deliver(&t.handler, &touch_event(TouchPhase::Began, point, t.origin), button, mods);
        if r.consumed {
            if let Ok(mut b) = cell.try_borrow_mut() {
                b.view_input.active = Some(ActiveTouch { node: t.node, handler: t.handler, button });
            }
            return true;
        }
    }
    false
}

/// The mouse moved to device `(x, y)`. During a gesture this is its
/// `Moved`; otherwise `Hovered` plus hover enter / leave.
pub fn mouse_moved(cell: &RefCell<WindowsBackend>, x: f32, y: f32, mods: PointerModifiers) {
    let Ok(b) = cell.try_borrow() else { return };
    let point = b.dpi.point_to_logical(x, y);
    if let Some(a) = &b.view_input.active {
        let (handler, button) = (a.handler.clone(), a.button);
        let origin = scene::window_origin(&b, a.node);
        drop(b);
        match origin {
            Some(origin) => {
                deliver(&handler, &touch_event(TouchPhase::Moved, point, origin), button, mods);
            }
            None => end_gesture(cell, TouchPhase::Cancelled, Some(point), mods),
        }
        return;
    }
    let targets = touch_targets(&b, point, false);
    let now_hovered: Vec<u64> = scene::hit_chain(&b, point.0, point.1)
        .into_iter()
        .map(|n| n.id)
        .filter(|id| b.view_input.hover.contains_key(id))
        .collect();
    drop(b);
    update_hover(cell, now_hovered);
    for t in targets {
        let ev = touch_event(TouchPhase::Hovered, point, t.origin);
        if deliver(&t.handler, &ev, PointerButton::Primary, mods).consumed {
            break;
        }
    }
}

/// A mouse button came up at device `(x, y)`. Returns whether it ended a
/// gesture a handler had taken (the host then releases capture and fires
/// no pressable). The release of a button that did not start the gesture
/// changes nothing.
pub fn mouse_up(
    cell: &RefCell<WindowsBackend>,
    x: f32,
    y: f32,
    button: PointerButton,
    mods: PointerModifiers,
) -> bool {
    let Ok(b) = cell.try_borrow() else { return false };
    let point = b.dpi.point_to_logical(x, y);
    let ends = b.view_input.active.as_ref().is_some_and(|a| a.button == button);
    drop(b);
    if ends {
        end_gesture(cell, TouchPhase::Ended, Some(point), mods);
    }
    ends
}

/// The host lost mouse capture mid-gesture (`WM_CAPTURECHANGED`): the
/// gesture ends as `Cancelled`. A no-op after [`mouse_up`] already ended
/// it (releasing capture there sends the same message).
pub fn capture_lost(cell: &RefCell<WindowsBackend>) {
    end_gesture(cell, TouchPhase::Cancelled, None, PointerModifiers::default());
}

/// Clears the active gesture, then reports `phase` to its handler,
/// measured from the node's current origin. `point` = `None` (no pointer
/// position with the message) reports the node's origin; a node that is
/// gone turns the phase into `Cancelled`.
fn end_gesture(cell: &RefCell<WindowsBackend>, phase: TouchPhase, point: Option<(f32, f32)>, mods: PointerModifiers) {
    let Ok(mut b) = cell.try_borrow_mut() else { return };
    let Some(a) = b.view_input.active.take() else { return };
    let origin = scene::window_origin(&b, a.node);
    drop(b);
    let (phase, origin) = match origin {
        Some(o) => (phase, o),
        None => (TouchPhase::Cancelled, (0.0, 0.0)),
    };
    let point = point.unwrap_or(origin);
    deliver(&a.handler, &touch_event(phase, point, origin), a.button, mods);
}

/// The pointer left the window (`WM_MOUSELEAVE`): every hovered node
/// leaves.
pub fn mouse_left(cell: &RefCell<WindowsBackend>) {
    update_hover(cell, Vec::new());
}

fn update_hover(cell: &RefCell<WindowsBackend>, now: Vec<u64>) {
    let Ok(mut b) = cell.try_borrow_mut() else { return };
    let before = std::mem::replace(&mut b.view_input.hovered, now.clone());
    let left: Vec<HoverHandler> = before
        .iter()
        .filter(|id| !now.contains(id))
        .filter_map(|id| b.view_input.hover.get(id).cloned())
        .collect();
    let entered: Vec<HoverHandler> = now
        .iter()
        .filter(|id| !before.contains(id))
        .filter_map(|id| b.view_input.hover.get(id).cloned())
        .collect();
    drop(b);
    for h in left {
        h(false);
    }
    for h in entered {
        h(true);
    }
}

fn drop_event(phase: FileDropPhase, point: (f32, f32), origin: (f32, f32)) -> FileDropEvent {
    FileDropEvent { phase, position: TouchPoint::new(point.0 - origin.0, point.1 - origin.1) }
}

/// An OS file drag entered or moved to device `(x, y)`. Returns whether a
/// view accepts it there (the drop target then shows the copy cursor).
pub fn drag_moved(cell: &RefCell<WindowsBackend>, x: f32, y: f32) -> bool {
    let Ok(b) = cell.try_borrow() else { return false };
    let point = b.dpi.point_to_logical(x, y);
    let targets: Vec<(u64, FileDropHandler, (f32, f32))> = scene::hit_chain(&b, point.0, point.1)
        .into_iter()
        .filter_map(|n| b.view_input.file_drop.get(&n.id).map(|h| (n.id, h.clone(), n.origin)))
        .collect();
    drop(b);
    let mut accepted = None;
    for (id, h, origin) in targets {
        if h(&drop_event(FileDropPhase::Entered, point, origin)).consumed {
            accepted = Some(id);
            break;
        }
    }
    let previous = match cell.try_borrow_mut() {
        Ok(mut b) => std::mem::replace(&mut b.view_input.drag_target, accepted),
        Err(_) => return accepted.is_some(),
    };
    if previous.is_some() && previous != accepted {
        exit_drag_target(cell, previous);
    }
    accepted.is_some()
}

/// The OS file drag left the window, or was dropped somewhere nothing
/// accepted it: the target gets `Exited`.
pub fn drag_left(cell: &RefCell<WindowsBackend>) {
    let previous = match cell.try_borrow_mut() {
        Ok(mut b) => b.view_input.drag_target.take(),
        Err(_) => return,
    };
    exit_drag_target(cell, previous);
}

fn exit_drag_target(cell: &RefCell<WindowsBackend>, id: Option<u64>) {
    let Some(id) = id else { return };
    let Ok(b) = cell.try_borrow() else { return };
    let handler = b.view_input.file_drop.get(&id).cloned();
    let origin = scene::window_origin(&b, id).unwrap_or((0.0, 0.0));
    drop(b);
    if let Some(h) = handler {
        h(&drop_event(FileDropPhase::Exited, origin, origin));
    }
}

/// Files were released at device `(x, y)`. The view that accepted the drag
/// gets `Dropped`; returns whether there was one. An empty `files` (a drag
/// of nothing with a local path) ends the drag as `Exited` instead.
pub fn dropped(cell: &RefCell<WindowsBackend>, x: f32, y: f32, files: Vec<DroppedFile>) -> bool {
    if files.is_empty() {
        drag_left(cell);
        return false;
    }
    let Ok(mut b) = cell.try_borrow_mut() else { return false };
    let point = b.dpi.point_to_logical(x, y);
    let Some(id) = b.view_input.drag_target.take() else { return false };
    let handler = b.view_input.file_drop.get(&id).cloned();
    let origin = scene::window_origin(&b, id);
    drop(b);
    match (handler, origin) {
        (Some(h), Some(origin)) => {
            h(&drop_event(FileDropPhase::Dropped(files), point, origin));
            true
        }
        _ => false,
    }
}

/// One wheel message at device `(x, y)` (client coordinates). `units` is
/// the signed `WHEEL_DELTA` count as the message carries it: positive =
/// away from the user for `WM_MOUSEWHEEL`, right for `WM_MOUSEHWHEEL`.
/// Returns whether a handler consumed it (the host then skips scroll-view
/// scrolling).
pub fn wheel(
    cell: &RefCell<WindowsBackend>,
    x: f32,
    y: f32,
    units: f32,
    horizontal: bool,
    mods: PointerModifiers,
) -> bool {
    let Ok(b) = cell.try_borrow() else { return false };
    let point = b.dpi.point_to_logical(x, y);
    let targets: Vec<(WheelHandler, (f32, f32))> = scene::hit_chain(&b, point.0, point.1)
        .into_iter()
        .filter_map(|n| b.view_input.wheel.get(&n.id).map(|h| (h.clone(), n.origin)))
        .collect();
    drop(b);
    if targets.is_empty() {
        return false;
    }
    let (kind, delta_x, delta_y, scale) = wheel_values(units, horizontal, mods.ctrl);
    for (h, origin) in targets {
        let ev = WheelEvent {
            kind,
            delta_x,
            delta_y,
            scale,
            rotation: 0.0,
            position: TouchPoint::new(point.0 - origin.0, point.1 - origin.1),
            window_position: TouchPoint::new(point.0, point.1),
            timestamp_ns: timestamp_ns(),
        };
        let r: TouchResponse = h(&ev);
        if r.consumed || r.claim {
            return true;
        }
    }
    false
}

/// `(kind, delta_x, delta_y, scale)` for a wheel message. Deltas follow
/// web's sign (positive = scroll down / right), so a notch toward the user
/// is `delta_y > 0`; with Ctrl held it is a zoom out.
pub(crate) fn wheel_values(units: f32, horizontal: bool, ctrl: bool) -> (WheelKind, f32, f32, f32) {
    if horizontal {
        return (WheelKind::Scroll, units * WHEEL_PX_PER_UNIT, 0.0, 1.0);
    }
    let dy = -units * WHEEL_PX_PER_UNIT;
    if ctrl {
        let dy = dy.clamp(-MAX_ZOOM_WHEEL_PX, MAX_ZOOM_WHEEL_PX);
        (WheelKind::Zoom, 0.0, 0.0, (-dy * ZOOM_PER_WHEEL_PX).exp())
    } else {
        (WheelKind::Scroll, 0.0, dy, 1.0)
    }
}

/// Keyboard modifiers right now, for a mouse message being handled
/// (`GetKeyState` reports the state as of the message being processed).
pub fn current_modifiers() -> PointerModifiers {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    let down = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    PointerModifiers {
        shift: down(VK_SHIFT),
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        meta: down(VK_LWIN) || down(VK_RWIN),
    }
}

#[cfg(test)]
mod tests {
    //! Routing against a hand-built node tree (no HWNDs: the host's
    //! message handling is the thin part, the chain walk is the logic).

    use super::*;
    use crate::{NodeMeta, ViewVisual};
    use runtime_shared::PointerEvents;
    use std::rc::Rc;
    use windows::Win32::Foundation::HWND;

    type Log = Rc<RefCell<Vec<String>>>;

    fn put(b: &mut WindowsBackend, id: u64, parent: Option<u64>, frame: (f32, f32, f32, f32)) {
        let mut m = NodeMeta::new(NodeKind::View(ViewVisual::default()));
        m.frame = frame;
        b.nodes.insert(id, m);
        match parent {
            Some(p) => b.children.entry(p).or_default().push(id),
            None => b.root_id = Some(id),
        }
    }

    /// A handler that logs `tag:phase@x,y` (local position) and answers
    /// `response`.
    fn logger(log: &Log, tag: &'static str, response: TouchResponse) -> TouchHandler {
        let log = log.clone();
        Rc::new(move |ev: &TouchEvent| {
            log.borrow_mut().push(format!("{tag}:{:?}@{},{}", ev.phase, ev.position.x, ev.position.y));
            response
        })
    }

    /// Installs through the backend's `InputOps`, the path the `view`
    /// handler takes (the old empty impl dropped every handler here).
    fn install_touch(cell: &RefCell<WindowsBackend>, id: u64, h: TouchHandler) {
        let node = crate::WindowsNode { id, hwnd: HWND(std::ptr::null_mut()) };
        runtime_vocabulary::caps::InputOps::install_touch_handler(&mut *cell.borrow_mut(), &node, h);
    }

    fn install_wheel(cell: &RefCell<WindowsBackend>, id: u64, h: WheelHandler) {
        let node = crate::WindowsNode { id, hwnd: HWND(std::ptr::null_mut()) };
        runtime_vocabulary::caps::InputOps::install_wheel_handler(&mut *cell.borrow_mut(), &node, h);
    }

    fn take(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    /// Root 800×600 with a child view at (100,100) 400×300 — the canvas3d
    /// demo's shape: the orbit handler on the view that holds the canvas.
    fn fixture() -> RefCell<WindowsBackend> {
        let mut b = WindowsBackend::new(HWND(std::ptr::null_mut()));
        put(&mut b, 1, None, (0.0, 0.0, 800.0, 600.0));
        put(&mut b, 2, Some(1), (100.0, 100.0, 400.0, 300.0));
        RefCell::new(b)
    }

    const NONE: PointerModifiers = PointerModifiers { shift: false, ctrl: false, alt: false, meta: false };
    const LEFT: PointerButton = PointerButton::Primary;

    /// `on_touch` was never wired on Windows (`InputOps` was the empty
    /// default), so the canvas3d orbit handler heard nothing: no drag to
    /// orbit, no pan. A press on the view starts a gesture that keeps
    /// reporting, from the same view's origin, until the release — also
    /// outside the view and the window, which the host's capture delivers.
    #[test]
    fn regression_view_on_touch_receives_a_whole_drag() {
        let cell = fixture();
        let log: Log = Default::default();
        install_touch(&cell, 2, logger(&log, "view", TouchResponse::CONSUMED));
        assert!(mouse_down(&cell, 150.0, 120.0, LEFT, NONE), "a consumed press asks for capture");
        mouse_moved(&cell, 160.0, 130.0, NONE);
        mouse_moved(&cell, 900.0, -20.0, NONE);
        assert!(mouse_up(&cell, 910.0, -30.0, LEFT, NONE));
        assert_eq!(
            take(&log),
            ["view:Began@50,20", "view:Moved@60,30", "view:Moved@800,-120", "view:Ended@810,-130"]
        );
        // The gesture is over: the next move is a hover, not a drag.
        mouse_moved(&cell, 150.0, 120.0, NONE);
        assert_eq!(take(&log), ["view:Hovered@50,20"]);
    }

    /// Deepest first; an ignored `Began` bubbles to the nearest ancestor
    /// handler, which then owns the gesture.
    #[test]
    fn began_bubbles_until_a_handler_consumes() {
        let cell = fixture();
        let log: Log = Default::default();
        {
            let mut b = cell.borrow_mut();
            b.view_input.touch.insert(2, logger(&log, "child", TouchResponse::IGNORED));
            b.view_input.touch.insert(1, logger(&log, "root", TouchResponse::CONSUMED));
        }
        assert!(mouse_down(&cell, 150.0, 120.0, LEFT, NONE));
        mouse_moved(&cell, 151.0, 120.0, NONE);
        assert_eq!(take(&log), ["child:Began@50,20", "root:Began@150,120", "root:Moved@151,120"]);
        mouse_up(&cell, 151.0, 120.0, LEFT, NONE);

        // Nobody consumes: no gesture, no capture.
        cell.borrow_mut().view_input.touch.insert(1, logger(&log, "root", TouchResponse::IGNORED));
        assert!(!mouse_down(&cell, 150.0, 120.0, LEFT, NONE));
        take(&log);
        mouse_moved(&cell, 160.0, 120.0, NONE);
        assert_eq!(take(&log), ["child:Hovered@60,20", "root:Hovered@160,120"], "a hover, not a drag");
    }

    /// A pressable between the pointer and a handler owns the press (a
    /// button over a canvas must not also orbit it).
    #[test]
    fn a_pressable_below_the_handler_owns_the_press() {
        let cell = fixture();
        let log: Log = Default::default();
        {
            let mut b = cell.borrow_mut();
            if let NodeKind::View(v) = &mut b.nodes.get_mut(&2).unwrap().kind {
                v.on_click = Some(Rc::new(|| {}));
            }
            b.view_input.touch.insert(1, logger(&log, "root", TouchResponse::CONSUMED));
        }
        assert!(!mouse_down(&cell, 150.0, 120.0, LEFT, NONE));
        assert!(take(&log).is_empty());
        assert!(cell.borrow().pressable_action(150.0, 120.0).is_some(), "the release fires the pressable");
    }

    /// The canvas3d HUD covers the canvas with `pointer_events: none` and
    /// opts its controls back in with `auto`. Windows ignored the style, so
    /// an always-mounted `none` layer took the hit itself.
    #[test]
    fn regression_pointer_events_none_is_transparent_and_auto_reenables() {
        let cell = fixture();
        {
            let mut b = cell.borrow_mut();
            // Overlay over everything (on top: inserted last), with a
            // pressable child at (0,0) 100×50 that opts back in.
            put(&mut b, 3, Some(1), (0.0, 0.0, 800.0, 600.0));
            b.nodes.get_mut(&3).unwrap().pointer_events = Some(PointerEvents::None);
            put(&mut b, 4, Some(3), (0.0, 0.0, 100.0, 50.0));
            b.nodes.get_mut(&4).unwrap().pointer_events = Some(PointerEvents::Auto);
            if let NodeKind::View(v) = &mut b.nodes.get_mut(&4).unwrap().kind {
                v.on_click = Some(Rc::new(|| {}));
            }
            // A pressable under the overlay — the AppShell-scrim shape.
            if let NodeKind::View(v) = &mut b.nodes.get_mut(&2).unwrap().kind {
                v.on_click = Some(Rc::new(|| {}));
            }
        }
        let b = cell.borrow();
        let chain = |x, y| scene::hit_chain(&b, x, y).iter().map(|n| n.id).collect::<Vec<_>>();
        assert_eq!(chain(150.0, 120.0), [2, 1], "the none overlay is not a target");
        assert_eq!(chain(10.0, 10.0), [4, 3, 1], "its auto child is, and bubbles through it");
        assert!(b.pressable_action(150.0, 120.0).is_some(), "the pressable under the overlay clicks");
        assert_eq!(chain(700.0, 500.0), [1]);
    }

    /// A press that a handler took ends `Cancelled` when capture is lost.
    #[test]
    fn lost_capture_cancels_the_gesture() {
        let cell = fixture();
        let log: Log = Default::default();
        cell.borrow_mut().view_input.touch.insert(2, logger(&log, "view", TouchResponse::CONSUMED));
        mouse_down(&cell, 150.0, 120.0, LEFT, NONE);
        capture_lost(&cell);
        capture_lost(&cell);
        assert_eq!(take(&log), ["view:Began@50,20", "view:Cancelled@0,0"], "once");
        assert!(!mouse_up(&cell, 150.0, 120.0, LEFT, NONE), "nothing left to end");
    }

    /// Only the button that started the gesture ends it, and every event
    /// reports that button (recognizers decline non-primary presses).
    #[test]
    fn the_pressing_button_is_reported_and_ends_the_gesture() {
        let cell = fixture();
        let seen: Rc<RefCell<Vec<PointerButton>>> = Default::default();
        let s = seen.clone();
        cell.borrow_mut().view_input.touch.insert(
            2,
            Rc::new(move |_| {
                s.borrow_mut().push(runtime_shared::pointer_button());
                TouchResponse::CONSUMED
            }),
        );
        assert!(mouse_down(&cell, 150.0, 120.0, PointerButton::Secondary, NONE));
        assert!(!mouse_down(&cell, 150.0, 120.0, LEFT, NONE), "a second button is not a new gesture");
        assert!(!mouse_up(&cell, 150.0, 120.0, LEFT, NONE));
        mouse_moved(&cell, 151.0, 120.0, NONE);
        assert!(mouse_up(&cell, 151.0, 120.0, PointerButton::Secondary, NONE));
        assert_eq!(*seen.borrow(), [PointerButton::Secondary; 3]);
    }

    /// A node removed mid-drag cancels the gesture instead of reporting
    /// positions measured from nowhere.
    #[test]
    fn a_removed_node_cancels_its_gesture() {
        let cell = fixture();
        let log: Log = Default::default();
        cell.borrow_mut().view_input.touch.insert(2, logger(&log, "view", TouchResponse::CONSUMED));
        mouse_down(&cell, 150.0, 120.0, LEFT, NONE);
        {
            let mut b = cell.borrow_mut();
            b.nodes.remove(&2);
            b.children.get_mut(&1).unwrap().clear();
        }
        mouse_moved(&cell, 160.0, 120.0, NONE);
        assert_eq!(take(&log), ["view:Began@50,20", "view:Cancelled@160,120"]);
    }

    /// `on_wheel` was never wired either (canvas3d's wheel zoom). A
    /// consumed tick reports the deltas web would (positive = down) and
    /// Ctrl+wheel — also a precision touchpad pinch — is a zoom.
    #[test]
    fn regression_view_on_wheel_receives_scroll_and_zoom() {
        let cell = fixture();
        let seen: Rc<RefCell<Vec<WheelEvent>>> = Default::default();
        let s = seen.clone();
        install_wheel(
            &cell,
            2,
            Rc::new(move |ev| {
                s.borrow_mut().push(*ev);
                TouchResponse::CONSUMED
            }),
        );
        assert!(!wheel(&cell, 10.0, 10.0, -120.0, false, NONE), "outside the view: unhandled");
        // One notch toward the user = scroll down.
        assert!(wheel(&cell, 150.0, 120.0, -120.0, false, NONE));
        let ctrl = PointerModifiers { ctrl: true, ..NONE };
        assert!(wheel(&cell, 150.0, 120.0, 120.0, false, ctrl));
        assert!(wheel(&cell, 150.0, 120.0, 120.0, true, NONE));
        let seen = seen.borrow();
        assert_eq!((seen[0].kind, seen[0].delta_y), (WheelKind::Scroll, 40.0));
        assert_eq!((seen[0].position.x, seen[0].position.y), (50.0, 20.0));
        assert_eq!(seen[1].kind, WheelKind::Zoom);
        assert!((seen[1].scale - 0.12f32.exp()).abs() < 1e-6, "a notch away zooms in one clamped step");
        assert_eq!((seen[2].kind, seen[2].delta_x, seen[2].delta_y), (WheelKind::Scroll, 40.0, 0.0));
    }

    /// Hover follows `mouseenter` semantics: moving into a child keeps the
    /// parent hovered; leaving the window leaves both.
    #[test]
    fn hover_enters_and_leaves_along_the_chain() {
        let cell = fixture();
        let log: Log = Default::default();
        for (id, tag) in [(1u64, "root"), (2, "child")] {
            let log = log.clone();
            cell.borrow_mut()
                .view_input
                .hover
                .insert(id, Rc::new(move |on: bool| log.borrow_mut().push(format!("{tag}:{on}"))));
        }
        mouse_moved(&cell, 10.0, 10.0, NONE);
        assert_eq!(take(&log), ["root:true"]);
        mouse_moved(&cell, 150.0, 120.0, NONE);
        assert_eq!(take(&log), ["child:true"]);
        mouse_moved(&cell, 160.0, 120.0, NONE);
        assert!(take(&log).is_empty());
        mouse_left(&cell);
        let mut left = take(&log);
        left.sort();
        assert_eq!(left, ["child:false", "root:false"]);
    }

    // --- file drop ---

    fn drop_logger(log: &Log, tag: &'static str, accept: bool) -> FileDropHandler {
        let log = log.clone();
        Rc::new(move |ev: &FileDropEvent| {
            let phase = match &ev.phase {
                FileDropPhase::Entered => "Entered".to_string(),
                FileDropPhase::Exited => "Exited".to_string(),
                FileDropPhase::Dropped(files) => {
                    format!("Dropped[{}]", files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(","))
                }
                _ => "?".to_string(),
            };
            log.borrow_mut().push(format!("{tag}:{phase}@{},{}", ev.position.x, ev.position.y));
            if accept { TouchResponse::CONSUMED } else { TouchResponse::IGNORED }
        })
    }

    fn file(name: &str) -> DroppedFile {
        DroppedFile { name: name.into(), mime: "text/plain".into(), size: None, path: None, source: None }
    }

    /// `on_file_drop` was never wired on Windows (the trait default), so
    /// the file-picker SDK's `FileDropZone` was inert. A drag offers
    /// `Entered` on every move; the accepting view gets the drop.
    #[test]
    fn regression_view_on_file_drop_receives_the_drag_and_the_drop() {
        let cell = fixture();
        let log: Log = Default::default();
        let node = crate::WindowsNode { id: 2, hwnd: HWND(std::ptr::null_mut()) };
        runtime_vocabulary::caps::InputOps::install_file_drop_handler(
            &mut *cell.borrow_mut(),
            &node,
            drop_logger(&log, "zone", true),
        );
        assert!(!drag_moved(&cell, 10.0, 10.0), "outside the zone: not accepted");
        assert!(drag_moved(&cell, 150.0, 120.0));
        assert!(drag_moved(&cell, 160.0, 130.0));
        assert!(dropped(&cell, 160.0, 130.0, vec![file("a.txt"), file("b.txt")]));
        assert_eq!(take(&log), ["zone:Entered@50,20", "zone:Entered@60,30", "zone:Dropped[a.txt,b.txt]@60,30"]);
    }

    /// Moving from one zone into a nested one: the inner accepts, then the
    /// outer is told the drag left it (web's `dragenter` before
    /// `dragleave`); leaving the window exits the inner.
    #[test]
    fn the_drag_target_moves_between_zones_and_leaves() {
        let cell = fixture();
        let log: Log = Default::default();
        {
            let mut b = cell.borrow_mut();
            b.view_input.file_drop.insert(1, drop_logger(&log, "outer", true));
            b.view_input.file_drop.insert(2, drop_logger(&log, "inner", true));
        }
        drag_moved(&cell, 10.0, 10.0);
        drag_moved(&cell, 150.0, 120.0);
        drag_left(&cell);
        assert_eq!(
            take(&log),
            ["outer:Entered@10,10", "inner:Entered@50,20", "outer:Exited@0,0", "inner:Exited@0,0"]
        );
        assert!(!dropped(&cell, 150.0, 120.0, vec![file("a.txt")]), "no drag in progress");
    }

    /// A declining zone bubbles to an accepting ancestor; a drop that
    /// carries nothing usable ends the drag as `Exited`.
    #[test]
    fn a_declining_zone_bubbles_and_an_empty_drop_exits() {
        let cell = fixture();
        let log: Log = Default::default();
        {
            let mut b = cell.borrow_mut();
            b.view_input.file_drop.insert(1, drop_logger(&log, "outer", true));
            b.view_input.file_drop.insert(2, drop_logger(&log, "inner", false));
        }
        assert!(drag_moved(&cell, 150.0, 120.0));
        assert!(!dropped(&cell, 150.0, 120.0, Vec::new()));
        assert_eq!(take(&log), ["inner:Entered@50,20", "outer:Entered@150,120", "outer:Exited@0,0"]);
    }
}
