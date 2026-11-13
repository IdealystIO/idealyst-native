//! Element identity / robot registry for vocabulary mounts.
//!
//! # Design: a vocabulary-owned registry
//!
//! `runtime_shared::robot`'s element registration surface is
//! `pub(crate)`, so there is no public write path a mount handler could
//! feed; the registry lives HERE, with the same entry shape, query
//! semantics, duplicate-id policy and register-at-mount /
//! deregister-at-teardown lifecycle, so the robot bridge's verbs
//! (`find_element`, `get_snapshot`, `tap`, `type_text`, `set_toggle`,
//! `set_slider`, `focus`/`blur`, `set_scroll`,
//! `get_frame`/`get_absolute_frame`, `introspect_native`, …) map 1:1.
//!
//! # The backend seam
//!
//! No robot-specific caps methods are required. Every node-resolving
//! action is a closure captured at mount over capabilities the traits
//! already expose:
//!
//! - geometry + native introspection ride
//!   [`IntrospectionOps`](crate::caps::IntrospectionOps)
//!   (`frame`/`absolute_frame`/`device_frame`/`introspect_native`,
//!   plus `note_introspection_root` at registration);
//! - `focus`/`blur` go through `make_text_input_handle(..).focus()`,
//!   `set_scroll` through `make_scroll_view_handle(..).scroll_to(..)`
//!   — the handle routes (see the borrow-safety note on the scroll
//!   action below);
//! - `click`/`set_text`/`set_toggle`/`set_slider` are the author
//!   callbacks the mount handler already holds.
//!
//! Every backend participates by implementing the caps traits it
//! already implements — the registry reads through them, so there are
//! no robot-specific backend hooks. The transport half is in place: [`bridge::invoke_command`] dispatches the wire verbs against
//! THIS registry, [`current_revision`] drives the live-update push, and
//! the [`install_driver_env`] seam lets the host supply `World::enter`
//! (label-resolving queries need the ambient world) plus a synchronous
//! settle (actions commit staged writes before returning). The web
//! relay client routes to all three when the boot installed the env
//! (`backend_web::newcore` ↔ `robot_transport`); the in-process
//! `robot-e2e` harness rides the same seam.
//!
//! # Lifecycle
//!
//! [`register_mount`] is called by every mount handler right after the
//! backend node exists and BEFORE children realize (children peek the
//! parent stack to link parenting; the returned guard pops on drop).
//! Deregistration is an [`on_teardown`](crate::style_attach::on_teardown)
//! probe registered at mount: when the realized subtree's `Owned`
//! drops (branch swap, keyed row removal, unmount), the entry is
//! removed. A stale entry after teardown is the classic bug this
//! prevents — see `regression_unmount_deregisters_entries` in the
//! vocabulary test suite.
//!
//! # Order, regions, touch
//!
//! - Queries (`find`, `find_all`, `count`, `snapshot` roots) walk the tree
//!   in DOCUMENT order — see [`OrderKey`]. Content a reactive region
//!   realizes after mount keeps its parent and its place through the
//!   scene's region-context seam ([`CapturedParent`]).
//! - `click` and `drag` drive touch handlers with synthesized events, so a
//!   tap recognizer on a view (or an extension node that registered
//!   through [`crate::robot_ext`]) is reachable — see [`Robot::click`],
//!   [`Robot::drag`].
//! - Text inputs report their accessible name (a11y label, else
//!   placeholder) as `label`.
//!
//! # Duplicate `test_id` policy
//!
//! The `by_test_id` index is LAST-WINS: `find(Query::TestId)` resolves
//! the most recently registered holder. `find_all(Query::TestId)` scans
//! every entry, returning ALL holders — sibling list rows legitimately
//! share one affordance test_id (Playwright's
//! `getByTestId(...).toHaveCount(n)`).
//!
//! # Feature gate
//!
//! The whole module is compiled only under the vocabulary's `robot`
//! Cargo feature (`runtime-core/robot` forwards to it): the identity
//! SLOTS (`test_id` prim fields + builder/glue setters) are always
//! present so `ui! { view(test_id = …) }` compiles in every build, but
//! the per-mount registration overhead exists only in robot builds.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_shared::primitives::portal::ViewportRect;

use crate::caps::{
    ContentClip, IntrospectionOps, NativeInjection, PointerStep, PointerStepPhase, ResolvedStyle, ScrollPort, ScrollTarget,
};

mod key;
pub use key::KeyMechanism;
pub(crate) use key::KeyTarget;

// The component `#[method]` registry + element link, and the signal
// watch registry, re-exported under `robot::…` (the same paths
// `runtime_shared::robot` uses, so suites swap only the crate root).
pub use crate::robot_methods::{
    component_for_element, component_props, components_for_element, invoke_method,
    list_components, register_component, ComponentInstanceId, ComponentRegistration,
    ComponentSnapshot, Method, PropMode, PropSnapshot,
};
pub use crate::robot_watch::{
    list_watched, read_watched_by_id, read_watched_by_name, unwatch_signal, watch_signal,
    watch_signal_writable, watched_history, write_watched, HistoryPoint, WatchTarget,
    WatchedSnapshot, WATCH_HISTORY_LEN,
};

// =============================================================================
// Element kinds (mirrors `runtime_shared::robot::ElementKind`)
// =============================================================================

/// Identifies what kind of primitive an element was built from. Same
/// variant set as `runtime_shared::robot::ElementKind`, so bridge verb
/// payloads (`kind` strings) match.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ElementKind {
    View,
    Text,
    Button,
    Pressable,
    Image,
    Icon,
    TextInput,
    Toggle,
    ScrollView,
    Slider,
    ActivityIndicator,
    Virtualizer,
    Graphics,
    Navigator,
    Link,
    Overlay,
    Presence,
}

/// Opaque identifier for a mounted element in the registry. Invalid
/// after the element's subtree tears down (the entry is removed).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ElementId(pub u32);

// =============================================================================
// Element state
// =============================================================================

/// What a primitive reports about itself: the state a user could see on
/// screen, read back so an E2E test can assert it with no help from the
/// app (no `watch_signal`, no component props).
///
/// Each mount handler that has state registers a closure producing this
/// (`MountActions::state`). It is the PRIMITIVE's view, not the author's:
/// a toggle's `checked` is what the native switch shows — the author's
/// controlled value after each write-back, or what the user (or robot)
/// last flipped it to when uncontrolled. The closures read only plain
/// cells the handler keeps current, never signals, so reading state
/// subscribes nothing and needs no reactive context.
///
/// Fields a primitive doesn't have are `None` and left out of the JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementState {
    /// A text field's text, or a slider's value.
    pub value: Option<StateValue>,
    /// A toggle's on/off.
    pub checked: Option<bool>,
    /// Whether the control refuses input (`disabled = …`). Robot actions
    /// on a disabled element fail with [`RobotError::Disabled`], as the
    /// control refuses a user.
    pub disabled: Option<bool>,
    /// Whether a text field holds keyboard focus.
    pub focused: Option<bool>,
    /// A scroll view's content offset `(x, y)`, logical px.
    pub scroll: Option<(f64, f64)>,
    /// Whether the pointer is over the element, as its style sees it (the
    /// `state hovered` variant). `None` on a node with no interaction
    /// states to report.
    pub hovered: Option<bool>,
    /// Whether the element is being pressed, as its style sees it (the
    /// `state pressed` variant).
    pub pressed: Option<bool>,
    /// A text field's soft-keyboard kind, as the author set it
    /// (`keyboard = KeyboardKind::…`; `Default` when unset).
    pub keyboard: Option<runtime_shared::primitives::text_input::KeyboardKind>,
    /// The keyboard kind the native widget holds, read back from it
    /// ([`IntrospectionOps::keyboard_kind`]): what a user's soft keyboard
    /// shows. `None` where the platform has no soft keyboard.
    pub native_keyboard: Option<runtime_shared::primitives::text_input::KeyboardKind>,
}

/// The robot's name for a keyboard kind (`"email"`, `"phone"`, …), as
/// `state.keyboard` reports it.
pub fn keyboard_name(kind: runtime_shared::primitives::text_input::KeyboardKind) -> &'static str {
    use runtime_shared::primitives::text_input::KeyboardKind as K;
    match kind {
        K::Default => "default",
        K::Email => "email",
        K::Phone => "phone",
        K::Number => "number",
        K::Decimal => "decimal",
        K::Url => "url",
        K::SignedDecimal => "signed_decimal",
    }
}

/// Whether an element can be seen: the toolkit shows it
/// ([`IntrospectionOps::is_shown`]: in a window, not hidden by itself or an
/// ancestor, not fully transparent), its laid-out box is not empty, and no
/// ancestor that clips without scrolling ([`ContentClip::Hidden`] — CSS
/// `overflow: hidden`) cuts that box away entirely — one outside a scroller
/// judges the scroller's box, which scrolling brings the content into.
/// Content a scroller or
/// the window cuts off (scrolled away, beyond the window) is still visible,
/// as in Playwright; [`in_viewport`](Self::in_viewport) says whether any
/// of it is on screen, and [`Robot::visible_frame`] what part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visibility {
    pub visible: bool,
    /// Whether part of the element's box is on screen — inside the window
    /// and every clipping ancestor; `None` when the frame or the window
    /// size is unknown.
    pub in_viewport: Option<bool>,
}

impl Visibility {
    /// From the toolkit's answer, the window-relative box and the window
    /// size. Unknown parts don't hide an element: with neither answer it is
    /// visible (mounted is all that's known).
    pub fn from_parts(shown: Option<bool>, frame: Option<ViewportRect>, window: Option<(f32, f32)>) -> Self {
        Self::clipped(shown, frame, &[], window).0
    }

    /// [`from_parts`](Self::from_parts), with the boxes of the element's
    /// clipping ancestors (`clips`, innermost first), and the part of the
    /// box left after cutting it by every one of them and the window — what
    /// [`Robot::visible_frame`] reports; empty (zero width or height) when
    /// nothing is left, `None` when the frame is unknown.
    ///
    /// A `Hidden` ancestor OUTSIDE a scroller cuts the scroller's box, not
    /// the scrolled content: scrolling brings the content into that box, so
    /// the element is hidden only if the box itself is cut away. Not "cut
    /// the element by every `Hidden` box": a horizontal tab strip's last tab
    /// sits thousands of points right of the screen-wide column holding the
    /// strip, and on Android, where every `ViewGroup` clips its children,
    /// that column cut the tab away entirely — the robot called it not
    /// visible and `click()` waited for it instead of scrolling to it.
    pub fn clipped(
        shown: Option<bool>,
        frame: Option<ViewportRect>,
        clips: &[(ContentClip, ViewportRect)],
        window: Option<(f32, f32)>,
    ) -> (Self, Option<ViewportRect>) {
        let Some(frame) = frame else {
            return (Self { visible: shown != Some(false), in_viewport: None }, None);
        };
        // Where the element can be brought by scrolling, as seen from the
        // ancestor being walked: its own box, until a scroller replaces it
        // with that scroller's box.
        let mut reach = frame;
        for (kind, rect) in clips {
            if is_empty(reach) {
                break;
            }
            match kind {
                ContentClip::Hidden => reach = clipped_to(reach, *rect),
                ContentClip::Scroll => reach = *rect,
                ContentClip::Visible => {}
            }
        }
        let visible = shown != Some(false) && !is_empty(frame) && !is_empty(reach);
        let mut seen = frame;
        for (kind, rect) in clips {
            if *kind != ContentClip::Visible {
                seen = clipped_to(seen, *rect);
            }
        }
        let window = window.filter(|(w, h)| *w > 0.0 && *h > 0.0);
        if let Some((w, h)) = window {
            seen = clipped_to(seen, ViewportRect { x: 0.0, y: 0.0, width: w, height: h });
        }
        if !visible {
            seen.width = 0.0;
            seen.height = 0.0;
        }
        let in_viewport = window.map(|_| visible && !is_empty(seen));
        (Self { visible, in_viewport }, Some(seen))
    }
}

fn is_empty(r: ViewportRect) -> bool {
    !(r.width > 0.0 && r.height > 0.0)
}

/// The overlap of `a` and `b`; zero-sized (at the nearer edge) when they
/// don't overlap.
fn clipped_to(a: ViewportRect, b: ViewportRect) -> ViewportRect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    ViewportRect { x, y, width: (right - x).max(0.0), height: (bottom - y).max(0.0) }
}

/// A primitive's value: a text field's text or a slider's number.
#[derive(Clone, Debug, PartialEq)]
pub enum StateValue {
    Text(String),
    Number(f64),
}

impl ElementState {
    /// `{"value":…,"checked":…,"disabled":…,"focused":…}`, absent fields
    /// omitted.
    pub fn to_json(&self) -> String {
        use runtime_shared::__serde_json as serde_json;
        let mut map = serde_json::Map::new();
        match &self.value {
            Some(StateValue::Text(t)) => {
                map.insert("value".into(), serde_json::Value::String(t.clone()));
            }
            Some(StateValue::Number(n)) => {
                map.insert("value".into(), serde_json::json!(n));
            }
            None => {}
        }
        for (key, v) in [
            ("checked", self.checked),
            ("disabled", self.disabled),
            ("focused", self.focused),
            ("hovered", self.hovered),
            ("pressed", self.pressed),
        ] {
            if let Some(v) = v {
                map.insert(key.into(), serde_json::Value::Bool(v));
            }
        }
        if let Some((x, y)) = self.scroll {
            map.insert("scroll".into(), serde_json::json!({ "x": x, "y": y }));
        }
        for (key, kind) in [("keyboard", self.keyboard), ("native_keyboard", self.native_keyboard)] {
            if let Some(kind) = kind {
                map.insert(key.into(), serde_json::Value::String(keyboard_name(kind).into()));
            }
        }
        serde_json::Value::Object(map).to_string()
    }

    fn is_disabled(&self) -> bool {
        self.disabled == Some(true)
    }
}

// =============================================================================
// Control handles
// =============================================================================

/// Type-erased action dispatch table for a mounted element.
#[derive(Clone, Default)]
pub(crate) struct ElementActions {
    pub click: Option<Rc<dyn Fn()>>,
    pub set_text: Option<Rc<dyn Fn(String)>>,
    pub set_toggle: Option<Rc<dyn Fn(bool)>>,
    pub set_slider: Option<Rc<dyn Fn(f32)>>,
    pub focus: Option<Rc<dyn Fn()>>,
    pub blur: Option<Rc<dyn Fn()>>,
    pub set_scroll: Option<Rc<dyn Fn(f32, f32)>>,
    /// The axes a scroll view scrolls — [`Robot::scroll_into_view`] moves
    /// only those (UIKit takes any content offset it is given, so asking
    /// a horizontal strip for a vertical one would shift it off its rail).
    pub scroll_axis: Option<runtime_shared::primitives::scroll_view::ScrollAxis>,
    pub frame: Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
    pub absolute_frame: Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
    pub device_frame: Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
    pub introspect: Option<Rc<dyn Fn() -> Option<runtime_shared::introspect::NativeNode>>>,
    /// The raw touch handler a view (or extension node) installed —
    /// already scope-guarded, exactly the handler the backend calls. The
    /// robot's `click` and `drag` synthesize touch streams into it (see
    /// [`Robot::click`] / [`Robot::drag`]).
    pub touch: Option<runtime_shared::TouchHandler>,
    /// A slider's drag: set the value at `fraction` (0..=1) of the track
    /// through the native widget's own change path. `false` when the
    /// slider is disabled. See [`Robot::drag`].
    pub slider_drag: Option<Rc<dyn Fn(f32) -> bool>>,
    /// The element's own state ([`ElementState`]).
    pub state: Option<Rc<dyn Fn() -> ElementState>>,
    /// The interaction bits its style's state machine holds
    /// (`style_attach::interaction_mirror`), linked by
    /// [`link_interaction`]. Fills [`ElementState::hovered`] / `pressed`.
    pub interaction: Option<Rc<std::cell::Cell<runtime_shared::StateBits>>>,
    /// The setter that style's state machine takes native input through, the
    /// one the backend was handed. [`Robot::press_native`]'s hit-test tier
    /// lights `pressed` through it where no real press can be injected.
    pub set_interaction: Option<Rc<dyn Fn(runtime_shared::StateBits, bool)>>,
    /// The toolkit's answer to "is it shown" ([`IntrospectionOps::is_shown`]).
    pub shown: Option<Rc<dyn Fn() -> Option<bool>>>,
    /// How it clips its descendants ([`IntrospectionOps::content_clip`]).
    pub clip: Option<Rc<dyn Fn() -> Option<ContentClip>>>,
    /// Whether a real tap at a window point reaches this element
    /// ([`IntrospectionOps::hit_contains`]).
    pub hit: Option<Rc<dyn Fn(f32, f32) -> Option<bool>>>,
    /// Real platform input in this element's window
    /// ([`IntrospectionOps::native_pointer`]): the injection, built under
    /// the backend borrow and run after it is released.
    pub native_pointer: Option<Rc<dyn Fn(&[PointerStep]) -> Option<NativeInjection>>>,
    /// Resize this element's window ([`IntrospectionOps::resize_window`]),
    /// built under the backend borrow and run after it is released. Only
    /// set on root elements — `set_viewport` resizes the first root's
    /// window, and a closure per element would be dead weight.
    pub resize_window: Option<Rc<dyn Fn(f32, f32) -> Option<NativeInjection>>>,
    /// Real key input in this element's window
    /// ([`IntrospectionOps::native_key`]); see [`Robot::press_key`].
    pub native_key: Option<key::NativeKey>,
    /// The platform's focus traversal in this element's window
    /// ([`IntrospectionOps::move_focus`]), for a Tab on
    /// [`Robot::press_key`]'s dispatch fallback.
    pub move_focus: Option<key::MoveFocus>,
    /// A text field's key handling, for [`Robot::press_key`]'s dispatch
    /// fallback ([`KeyTarget`]).
    pub keys: Option<KeyTarget>,
    /// The element's own scroller, or (`ScrollTarget::Host`) the one above
    /// the app's tree ([`IntrospectionOps::scroll_port`]); see
    /// [`Robot::scroll_into_view`].
    pub scroll_port: Option<Rc<dyn Fn(ScrollTarget) -> Option<ScrollPort>>>,
    /// Move that scroller ([`IntrospectionOps::set_scroll_offset`]), built
    /// under the backend borrow and run after it is released.
    pub set_scroll_port: Option<Rc<dyn Fn(ScrollTarget, f32, f32) -> Option<NativeInjection>>>,
    /// A text field's native keyboard kind ([`IntrospectionOps::keyboard_kind`]).
    pub native_keyboard: Option<Rc<dyn Fn() -> Option<runtime_shared::primitives::text_input::KeyboardKind>>>,
    /// The toolkit's resolved style ([`IntrospectionOps::computed_style`]).
    pub computed_style: Option<Rc<dyn Fn() -> Option<ResolvedStyle>>>,
    /// The rules the framework last resolved and handed the backend for
    /// this element (`style_attach::style_mirror`), linked by
    /// [`link_interaction`]; [`Robot::style`] reports them where the
    /// toolkit has no resolved style of its own to read.
    pub style_rules: Option<crate::style_attach::style_mirror::Cell>,
}

/// The handler-supplied half of [`ElementActions`] — what the mount
/// handler has in hand (author callbacks + handle routes). The
/// geometry/introspection closures are derived inside
/// [`register_mount`] from [`IntrospectionOps`].
#[derive(Default)]
pub(crate) struct MountActions {
    pub click: Option<Rc<dyn Fn()>>,
    pub set_text: Option<Rc<dyn Fn(String)>>,
    pub set_toggle: Option<Rc<dyn Fn(bool)>>,
    pub set_slider: Option<Rc<dyn Fn(f32)>>,
    pub focus: Option<Rc<dyn Fn()>>,
    pub blur: Option<Rc<dyn Fn()>>,
    pub set_scroll: Option<Rc<dyn Fn(f32, f32)>>,
    /// See [`ElementActions::scroll_axis`].
    pub scroll_axis: Option<runtime_shared::primitives::scroll_view::ScrollAxis>,
    pub slider_drag: Option<Rc<dyn Fn(f32) -> bool>>,
    pub state: Option<Rc<dyn Fn() -> ElementState>>,
    pub keys: Option<KeyTarget>,
}

// =============================================================================
// Registry
// =============================================================================

/// A single mounted element tracked by the registry.
pub(crate) struct RegistryEntry {
    pub kind: ElementKind,
    /// Author-assigned stable identifier (via `test_id = …`).
    pub test_id: Option<&'static str>,
    /// Mount-time label snapshot (button/text content). Authoritative
    /// for static sources; a fallback for reactive ones.
    pub label: Option<String>,
    /// Live-label recompute for reactive text/labels. The registry
    /// entry is built once at mount and never re-registered when a
    /// bound signal changes, so a cached string would go stale. The
    /// closure reads UNTRACKED so a
    /// robot query never subscribes anything.
    pub label_fn: Option<Rc<dyn Fn() -> Option<String>>>,
    pub actions: ElementActions,
    pub parent: Option<ElementId>,
    /// Kept sorted by [`order`](Self::order), so a pre-order walk is
    /// document order.
    pub children: Vec<ElementId>,
    /// Sibling sort key — see [`OrderKey`].
    pub order: OrderKey,
    /// Drawn somewhere other than inside its parent: a top-level element
    /// of a portal's content (a menu panel, a sheet, a flyout), which the
    /// backend draws on the window. It stays the opener's child in the
    /// tree, but nothing above it clips or scrolls it: the visibility and
    /// `scroll_into_view` walks stop here.
    pub detached: bool,
}

/// Where an entry sits among its siblings.
///
/// Registration order is NOT document order once reactive regions are
/// involved: a branch that swaps in (or a keyed row added) later registers
/// after every sibling that follows it on screen. And slot ids are recycled
/// through a free list, so id order is meaningless too — which is how
/// `find_all_elements(kind: "TextInput")` came back with a form step's
/// fields shuffled.
///
/// So: an ordinary registration takes `(seq, 0)` from a monotonic counter
/// (realize order, which IS document order within one synchronous mount).
/// A region reserves one `seq` when it is created, and everything it
/// realizes directly under its parent — at mount or any time later — takes
/// `(that seq, n)` with `n` counting up. The region's content therefore
/// always sorts after the siblings before the region and before the ones
/// after it. Within one region, content added later sorts after content
/// added earlier, so a keyed row inserted mid-list lists at the end of
/// that list (the scene does not report row positions to the registry).
pub type OrderKey = (u64, u64);

impl RegistryEntry {
    fn current_label(&self) -> Option<String> {
        match &self.label_fn {
            Some(f) => f(),
            None => self.label.clone(),
        }
    }
}

pub(crate) struct Registry {
    entries: Vec<Option<RegistryEntry>>,
    free: Vec<u32>,
    /// test_id → element id. LAST-WINS on duplicates (module docs);
    /// `find_all` scans entries instead of reading this 1:1 index.
    by_test_id: HashMap<&'static str, ElementId>,
}

impl Registry {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            free: Vec::new(),
            by_test_id: HashMap::new(),
        }
    }

    fn insert(&mut self, entry: RegistryEntry) -> ElementId {
        let id = if let Some(idx) = self.free.pop() {
            self.entries[idx as usize] = Some(entry);
            ElementId(idx)
        } else {
            let idx = self.entries.len() as u32;
            self.entries.push(Some(entry));
            ElementId(idx)
        };
        if let Some(Some(entry)) = self.entries.get(id.0 as usize) {
            if let Some(test_id) = entry.test_id {
                self.by_test_id.insert(test_id, id);
            }
        }
        id
    }

    fn remove(&mut self, id: ElementId) {
        if let Some(entry) = self.entries.get_mut(id.0 as usize).and_then(|s| s.take()) {
            // Only drop the index mapping if it still points at US — a
            // later duplicate registration (last-wins) may own it now,
            // and removing the older entry must not orphan the live one.
            if let Some(test_id) = entry.test_id {
                if self.by_test_id.get(test_id) == Some(&id) {
                    self.by_test_id.remove(test_id);
                }
            }
            // Unlink from a still-live parent so a recycled slot can't
            // later be walked as this parent's child (stale-child-id
            // guard).
            if let Some(pid) = entry.parent {
                if let Some(Some(parent)) = self.entries.get_mut(pid.0 as usize) {
                    parent.children.retain(|c| *c != id);
                }
            }
            self.free.push(id.0);
        }
    }

    fn get(&self, id: ElementId) -> Option<&RegistryEntry> {
        self.entries.get(id.0 as usize).and_then(|s| s.as_ref())
    }

    fn entries(&self) -> impl Iterator<Item = (ElementId, &RegistryEntry)> {
        self.entries.iter().enumerate().filter_map(|(i, e)| e.as_ref().map(|e| (ElementId(i as u32), e)))
    }

    /// Insert `child` into `parent`'s children at its [`OrderKey`]
    /// position (after any equal key, so ties keep registration order).
    fn link_child(&mut self, parent: ElementId, child: ElementId, order: OrderKey) {
        let keys: Vec<OrderKey> = match self.get(parent) {
            Some(p) => p
                .children
                .iter()
                .map(|c| self.get(*c).map_or((0, 0), |e| e.order))
                .collect(),
            None => return,
        };
        let at = keys.partition_point(|k| *k <= order);
        if let Some(Some(p)) = self.entries.get_mut(parent.0 as usize) {
            p.children.insert(at, child);
        }
    }

    /// Roots — entries with no live parent (none, or a dead/recycled
    /// slot: the orphan rule) — in [`OrderKey`] order.
    fn roots(&self) -> Vec<ElementId> {
        let mut roots: Vec<(OrderKey, ElementId)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let entry = slot.as_ref()?;
                let is_root = match entry.parent {
                    None => true,
                    Some(pid) => self.get(pid).is_none(),
                };
                is_root.then_some((entry.order, ElementId(i as u32)))
            })
            .collect();
        roots.sort_by_key(|(order, _)| *order);
        roots.into_iter().map(|(_, id)| id).collect()
    }

    /// Every live entry in document order: roots in order, each followed
    /// by its subtree, pre-order. What `find` / `find_all` / `count` walk,
    /// so "the first match" and "all matches" read top-to-bottom the way
    /// the screen does rather than in slot-id order.
    fn document_order(&self) -> Vec<ElementId> {
        let mut out = Vec::with_capacity(self.entries.len());
        let mut stack: Vec<ElementId> = self.roots().into_iter().rev().collect();
        while let Some(id) = stack.pop() {
            let Some(entry) = self.get(id) else { continue };
            out.push(id);
            stack.extend(entry.children.iter().rev().copied());
        }
        out
    }
}

/// One parent-stack frame: the element registrations nest under, and —
/// for a frame a region pushed — the region's sibling order (see
/// [`OrderKey`]).
#[derive(Clone)]
struct Frame {
    id: Option<ElementId>,
    region: Option<Rc<RegionOrder>>,
    /// Registrations under this frame are drawn elsewhere
    /// ([`RegistryEntry::detached`]); pushed by [`drawn_elsewhere`].
    detached: bool,
}

struct RegionOrder {
    base: u64,
    next: std::cell::Cell<u64>,
}

runtime_tls::thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::new());
    /// Parent stack: [`register_mount`] pushes before the handler
    /// realizes children; the guard pops on drop. Children peek the top
    /// to set `parent`. Regions push a frame around each realize of their
    /// content (see [`CapturedParent`]).
    static PARENT_STACK: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
    /// Source of [`OrderKey`] sequence numbers.
    static ORDER_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
    static REGION_HOOK_INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn next_seq() -> u64 {
    ORDER_SEQ.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

/// Teach the scene's regions to carry the parent stack into their later
/// realizations (`runtime_scene::install_region_context`). Installed on
/// the first registration of the thread: nothing can be parented before
/// that, so a region created earlier loses nothing.
fn ensure_region_hook() {
    if REGION_HOOK_INSTALLED.with(|c| c.replace(true)) {
        return;
    }
    runtime_scene::install_region_context(|| {
        let captured = capture_parent();
        Rc::new(move |f: &mut dyn FnMut()| with_captured_parent(&captured, f))
    });
}

/// Monotonic change-revision for a live-update push channel — same
/// contract as `runtime_shared::robot::current_revision` (the bridge
/// coalesces bursts; register/deregister bump it).
static ROBOT_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn bump_revision() {
    ROBOT_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A component registered or deregistered: the component tree a
/// subscribed inspector shows changed, so push a revision the same way an
/// element (de)registration does.
pub(crate) fn bump_component_revision() {
    bump_revision();
}

/// The element a registration would currently be parented under — the top of
/// the parent stack, or `None` at a realize root.
pub fn current_parent() -> Option<ElementId> {
    PARENT_STACK.with(|s| s.borrow().last().and_then(|f| f.id))
}

/// A registration position captured at mount, to be re-entered by content
/// realized LATER, outside the mount's dynamic scope.
///
/// # Why this exists
///
/// Content realized after the mount that owns it — a reactive region's
/// branch swap or new keyed row, a parked late-bound item, a `lazy` body
/// once its chunk arrives — is no longer inside the dynamic scope the
/// parent stack tracks, so everything it mounted registered as a detached
/// registry ROOT: `get_parent` reported nothing, a `test_id`-scoped query
/// saw an empty subtree (the `idea-ui-docs` `page-content` anchor had 0
/// children while a detached 399-element root held the page), and
/// `find_all` ordered the content by recycled slot id. Scene regions
/// capture one of these through `runtime_scene::install_region_context`;
/// `lazy` captures one directly.
#[derive(Clone)]
pub struct CapturedParent(Frame);

/// Capture the current registration position (see [`CapturedParent`]).
/// Reserves the sibling-order slot the captured content will sort into.
pub fn capture_parent() -> CapturedParent {
    CapturedParent(Frame {
        id: current_parent(),
        region: Some(Rc::new(RegionOrder {
            base: next_seq(),
            next: std::cell::Cell::new(1),
        })),
        // Content a region swaps in at the top of a portal is still
        // drawn on the window.
        detached: PARENT_STACK.with(|s| s.borrow().last().is_some_and(|f| f.detached)),
    })
}

/// Run `f` with what it registers marked drawn elsewhere — a portal's
/// content (see [`RegistryEntry::detached`]). The elements stay under the
/// current parent in the tree.
pub(crate) fn drawn_elsewhere<R>(f: impl FnOnce() -> R) -> R {
    let frame = PARENT_STACK.with(|s| {
        let top = s.borrow().last().cloned();
        Frame { detached: true, ..top.unwrap_or(Frame { id: None, region: None, detached: true }) }
    });
    PARENT_STACK.with(|s| s.borrow_mut().push(frame));
    let depth = PARENT_STACK.with(|s| s.borrow().len());
    let out = f();
    PARENT_STACK.with(|s| {
        let mut stack = s.borrow_mut();
        debug_assert_eq!(stack.len(), depth, "robot parent stack out of order around a portal's content");
        stack.pop();
    });
    out
}

/// The elements registered directly under `id`, in order.
pub(crate) fn children_of(id: ElementId) -> Vec<ElementId> {
    REGISTRY.with(|r| r.borrow().get(id).map(|e| e.children.clone()).unwrap_or_default())
}

/// Move `ids` (with their subtrees) to the captured position, after
/// whatever is already registered there. For content mounted before the
/// place it is shown in existed: a navigator's first screen is built
/// before the layout holding its outlet.
pub(crate) fn move_under(ids: &[ElementId], to: &CapturedParent) {
    let Frame { id: new_parent, region, .. } = &to.0;
    REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        for &id in ids {
            let Some(old) = reg.get(id).map(|e| e.parent) else { continue };
            if let Some(Some(p)) = old.and_then(|p| reg.entries.get_mut(p.0 as usize)) {
                p.children.retain(|c| *c != id);
            }
            let order = match region {
                Some(region) => {
                    let n = region.next.get();
                    region.next.set(n + 1);
                    (region.base, n)
                }
                None => (next_seq(), 0),
            };
            if let Some(Some(e)) = reg.entries.get_mut(id.0 as usize) {
                e.parent = *new_parent;
                e.order = order;
            }
            if let Some(pid) = new_parent {
                reg.link_child(*pid, id, order);
            }
        }
    });
    bump_revision();
}

/// Run `f` registering at the captured position.
pub fn with_captured_parent<R>(captured: &CapturedParent, f: impl FnOnce() -> R) -> R {
    PARENT_STACK.with(|s| s.borrow_mut().push(captured.0.clone()));
    let depth = PARENT_STACK.with(|s| s.borrow().len());
    let out = f();
    PARENT_STACK.with(|s| {
        let mut stack = s.borrow_mut();
        debug_assert_eq!(
            stack.len(),
            depth,
            "robot parent stack out of order — a captured parent must nest LIFO with \
             the `RegisteredPrim` guards realized inside it"
        );
        stack.pop();
    });
    out
}

/// The current change-revision (read by a bridge/relay transport).
pub fn current_revision() -> u64 {
    ROBOT_REVISION.load(std::sync::atomic::Ordering::Relaxed)
}

// =============================================================================
// Mount-time registration (the shared helper every handler calls)
// =============================================================================

/// RAII guard returned by [`register_mount`]: keeps the element on the
/// parent stack while the mount handler realizes children, pops on
/// drop (end of the handler body — children are realized by then).
pub(crate) struct RegisteredPrim {
    id: ElementId,
    /// `false` for a mount inside the framework's own highlight layer
    /// (see [`crate::robot_highlight`]): nothing was registered and
    /// nothing was pushed, so there's nothing to pop.
    live: bool,
}

impl RegisteredPrim {
    /// The registry id this mount received — read by the navigator
    /// handlers (nav-registry `element_id`) and tests.
    pub(crate) fn id(&self) -> ElementId {
        self.id
    }
}

impl Drop for RegisteredPrim {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        PARENT_STACK.with(|s| {
            let mut stack = s.borrow_mut();
            debug_assert_eq!(
                stack.last().and_then(|f| f.id),
                Some(self.id),
                "robot parent stack out of order — guards must drop LIFO"
            );
            stack.pop();
        });
    }
}

/// Register a just-created primitive node. Call AFTER the backend node
/// exists and BEFORE `realize_children_into` (children link to the top
/// of the parent stack). Wires:
///
/// - the registry entry (kind / test_id / label / actions),
/// - geometry + native-introspection closures over [`IntrospectionOps`]
///   (evaluated lazily at query time — registration itself makes no
///   geometry calls, so scene-parity op goldens are unaffected),
/// - `note_introspection_root` (primitive-boundary marker for native
///   introspection walks — a data-only default no-op),
/// - teardown deregistration via an `on_teardown` probe (dies with the
///   realized subtree),
/// - the parent-stack push (popped by the returned guard's drop).
pub(crate) fn register_mount<H: IntrospectionOps>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    kind: ElementKind,
    test_id: Option<&'static str>,
    label: Option<String>,
    label_fn: Option<Rc<dyn Fn() -> Option<String>>>,
    actions: MountActions,
) -> RegisteredPrim {
    // The highlight layer is the framework's own drawing, not the app's:
    // it must not appear in snapshots, element counts or the Inspector.
    if crate::robot_highlight::registration_suppressed() {
        return RegisteredPrim { id: ElementId(u32::MAX), live: false };
    }
    let MountActions {
        click,
        set_text,
        set_toggle,
        set_slider,
        focus,
        blur,
        set_scroll,
        scroll_axis,
        slider_drag,
        state,
        keys,
    } = actions;
    // Geometry / introspection closures: borrow the backend at QUERY
    // time (never held across the query's own callback).
    let frame = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().frame(&n)) as Rc<dyn Fn() -> Option<ViewportRect>>
    };
    let absolute_frame = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().absolute_frame(&n)) as Rc<dyn Fn() -> Option<ViewportRect>>
    };
    let device_frame = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().device_frame(&n)) as Rc<dyn Fn() -> Option<ViewportRect>>
    };
    // Absent where the backend has no read, so `Robot::introspect_native`
    // tells "this backend can't" (`ActionNotAvailable`) from "not laid out yet"
    // (`Ok(None)`). A check of the widget skips on the first and retries on the
    // second: a backend that claims the read and stops answering fails.
    let introspect = backend.borrow().supports_native_introspection().then(|| {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().introspect_native(&n))
            as Rc<dyn Fn() -> Option<runtime_shared::introspect::NativeNode>>
    });
    let shown = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().is_shown(&n)) as Rc<dyn Fn() -> Option<bool>>
    };
    let clip = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().content_clip(&n)) as Rc<dyn Fn() -> Option<ContentClip>>
    };
    let hit = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move |x, y| b.borrow().hit_contains(&n, x, y)) as Rc<dyn Fn(f32, f32) -> Option<bool>>
    };
    let native_pointer = {
        let b = backend.clone();
        let n = node.clone();
        // The borrow ends with this expression: the injection runs later.
        Rc::new(move |steps: &[PointerStep]| b.borrow().native_pointer(&n, steps))
            as Rc<dyn Fn(&[PointerStep]) -> Option<NativeInjection>>
    };
    let native_key = key::native_key_closure(backend, node);
    let move_focus = key::move_focus_closure(backend, node);
    let scroll_port = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move |t: ScrollTarget| b.borrow().scroll_port(&n, t)) as Rc<dyn Fn(ScrollTarget) -> Option<ScrollPort>>
    };
    let set_scroll_port = {
        let b = backend.clone();
        let n = node.clone();
        // The borrow ends with this expression: the injection runs later.
        Rc::new(move |t: ScrollTarget, x: f32, y: f32| b.borrow().set_scroll_offset(&n, t, x, y))
            as Rc<dyn Fn(ScrollTarget, f32, f32) -> Option<NativeInjection>>
    };
    let native_keyboard = (kind == ElementKind::TextInput).then(|| {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().keyboard_kind(&n))
            as Rc<dyn Fn() -> Option<runtime_shared::primitives::text_input::KeyboardKind>>
    });
    let computed_style = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().computed_style(&n)) as Rc<dyn Fn() -> Option<ResolvedStyle>>
    };
    backend.borrow().note_introspection_root(node);
    ensure_region_hook();

    let (parent, order, detached) = PARENT_STACK.with(|s| match s.borrow().last() {
        Some(Frame { id, region: Some(region), detached }) => {
            let n = region.next.get();
            region.next.set(n + 1);
            (*id, (region.base, n), *detached)
        }
        Some(Frame { id, region: None, detached }) => (*id, (next_seq(), 0), *detached),
        None => (None, (next_seq(), 0), false),
    });
    let resize_window = parent.is_none().then(|| {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move |w: f32, h: f32| b.borrow().resize_window(&n, w, h))
            as Rc<dyn Fn(f32, f32) -> Option<NativeInjection>>
    });
    let id = REGISTRY.with(|r| {
        r.borrow_mut().insert(RegistryEntry {
            kind,
            test_id,
            label,
            label_fn,
            actions: ElementActions {
                click,
                set_text,
                set_toggle,
                set_slider,
                focus,
                blur,
                set_scroll,
                scroll_axis,
                frame: Some(frame),
                absolute_frame: Some(absolute_frame),
                device_frame: Some(device_frame),
                introspect,
                touch: None,
                slider_drag,
                state,
                interaction: None,
                set_interaction: None,
                shown: Some(shown),
                clip: Some(clip),
                hit: Some(hit),
                native_pointer: Some(native_pointer),
                resize_window,
                native_key: Some(native_key),
                move_focus: Some(move_focus),
                keys,
                scroll_port: Some(scroll_port),
                set_scroll_port: Some(set_scroll_port),
                native_keyboard,
                computed_style: Some(computed_style),
                style_rules: None,
            },
            parent,
            children: Vec::new(),
            order,
            detached,
        })
    });
    if let Some(pid) = parent {
        REGISTRY.with(|r| r.borrow_mut().link_child(pid, id, order));
    }
    // Element↔component link: every component whose realize hook is
    // entered and unconsumed (`robot_methods` module docs) renders as
    // THIS element — the first one registered inside its bracket. More
    // than one when a component's root is another component's element;
    // taken outermost first, which is the order the links record.
    for instance in crate::robot_methods::take_pending_component_links() {
        crate::robot_methods::link_component_element(instance, id.0);
    }
    bump_revision();

    // Deregister when the realized subtree tears down (branch swap,
    // keyed removal, unmount). Without this, every torn-down branch
    // leaks a phantom entry — the classic stale-registry bug.
    crate::style_attach::on_teardown(move || {
        REGISTRY.with(|r| r.borrow_mut().remove(id));
        bump_revision();
    });

    PARENT_STACK.with(|s| s.borrow_mut().push(Frame { id: Some(id), region: None, detached: false }));
    RegisteredPrim { id, live: true }
}

/// Attach the touch handler a mount installed AFTER registering (a view's
/// `on_touch` goes on once its children and style are in). `handler` must
/// be the exact, scope-guarded handler the backend received.
pub(crate) fn attach_touch_action(prim: &RegisteredPrim, handler: runtime_shared::TouchHandler) {
    if !prim.live {
        return;
    }
    REGISTRY.with(|r| {
        if let Some(Some(entry)) = r.borrow_mut().entries.get_mut(prim.id.0 as usize) {
            entry.actions.touch = Some(handler);
        }
    });
}

/// Link the interaction cell the handler's own `attach_style` just made (if
/// it made one: an event-driven backend and a sheet with state variants) to
/// the element, and the style mirror the robot's `get_style` reads
/// (`style_attach::style_mirror`). Call right after that `attach_style`,
/// inside the same branch, so no other node's cells can be picked up.
pub(crate) fn link_interaction(prim: &RegisteredPrim) {
    let (cell, set_interaction) = crate::style_attach::interaction_mirror::take();
    let rules = crate::style_attach::style_mirror::take();
    if !prim.live || (cell.is_none() && rules.is_none()) {
        return;
    }
    REGISTRY.with(|r| {
        if let Some(Some(entry)) = r.borrow_mut().entries.get_mut(prim.id.0 as usize) {
            if cell.is_some() {
                entry.actions.interaction = cell;
            }
            if set_interaction.is_some() {
                entry.actions.set_interaction = set_interaction;
            }
            if rules.is_some() {
                entry.actions.style_rules = rules;
            }
        }
    });
}

// =============================================================================
// Navigator registry — the `list_navigators` / back-stack snapshot
// surface (slab + free list + "current" marker + prune-on-read)
// =============================================================================

/// Stable id for a navigator in the registry. Cheap to copy.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NavId(pub u32);

/// The per-snapshot payload a navigator's mount handler computes on
/// demand (`None` ⇒ the navigator's shared state is gone — dead `Weak`,
/// pruned). Signal reads inside
/// the closure are untracked; the bridge runs them world-entered.
pub struct NavSnapshotData {
    pub active_route: String,
    pub active_path: String,
    /// The active screen's query (its `ScreenState`). Rides beside the
    /// path, never inside it: `active_path` stays path-only, as everywhere
    /// in the navigator substrate.
    pub query: runtime_shared::primitives::navigator::QueryParams,
    pub depth: usize,
    pub can_go_back: bool,
    pub base: String,
    /// Back-stack `(route, path)` pairs, root-first, current last.
    /// A swap navigator (depth-less) reports its single active entry.
    pub stack: Vec<(String, String)>,
}

/// A read-only snapshot of one registered navigator — the field set
/// `nav_snapshot_json` serializes.
pub struct NavSnapshot {
    pub nav_id: u32,
    pub element_id: Option<u32>,
    /// Kind carrier: the builder's `.nav_label(...)` (the SDK's
    /// presentation label), else the vocabulary builder name
    /// (`"swap_navigator"` / `"stack_navigator"`).
    pub type_name: &'static str,
    pub active_route: String,
    pub active_path: String,
    /// The active screen's query — see [`NavSnapshotData::query`].
    pub query: runtime_shared::primitives::navigator::QueryParams,
    pub depth: usize,
    pub can_go_back: bool,
    /// `true` for the most-recently-dispatched-against navigator.
    pub is_current: bool,
    pub base: String,
    pub stack: Vec<(String, String)>,
    /// Accepts [`navigate`] commands.
    pub controllable: bool,
}

/// What the bridge's `navigate` verb asks a navigator to do. Paths are
/// FULL paths (base included, like `active_path`), optionally with a
/// `?query`, and resolve the way a deep link does: the screen whose
/// pattern matches the longest prefix wins, and any remainder is left for
/// a navigator nested in that screen. A path is an error only when it
/// falls outside the navigator's base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavAction {
    Push(String),
    Replace(String),
    Reset(String),
    Pop,
}

/// A navigator's robot control: resolve and dispatch a [`NavAction`]
/// exactly as an in-app link or back press would. Installed by the mount
/// handler once its dispatch exists.
pub(crate) type NavControl = Rc<dyn Fn(NavAction) -> Result<(), String>>;

struct NavRegEntry {
    type_name: &'static str,
    element_id: Option<u32>,
    snapshot: Rc<dyn Fn() -> Option<NavSnapshotData>>,
    control: Option<NavControl>,
}

#[derive(Default)]
struct NavRegistry {
    /// Slab; index = `NavId`. `None` = freed slot (LIFO recycled).
    entries: Vec<Option<NavRegEntry>>,
    free: Vec<u32>,
    /// Most-recently-dispatched-against navigator — "current".
    active: Option<NavId>,
}

runtime_tls::thread_local! {
    static NAV_REGISTRY: RefCell<NavRegistry> = RefCell::new(NavRegistry::default());
}

/// Register a freshly-mounted navigator. The mount handler supplies the
/// kind carrier, its element-registry id, and an on-demand snapshot
/// closure (typically over a `Weak` of the navigator's shared state).
/// If no navigator is yet current, this one becomes current (cold-start
/// root).
pub(crate) fn register_navigator(
    type_name: &'static str,
    element_id: Option<u32>,
    snapshot: Rc<dyn Fn() -> Option<NavSnapshotData>>,
) -> NavId {
    NAV_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        let entry = NavRegEntry {
            type_name,
            element_id,
            snapshot,
            control: None,
        };
        let id = if let Some(idx) = reg.free.pop() {
            reg.entries[idx as usize] = Some(entry);
            NavId(idx)
        } else {
            let idx = reg.entries.len() as u32;
            reg.entries.push(Some(entry));
            NavId(idx)
        };
        if reg.active.is_none() {
            reg.active = Some(id);
        }
        id
    })
}

/// Install `id`'s robot control (see [`NavControl`]).
pub(crate) fn set_navigator_control(id: NavId, control: NavControl) {
    NAV_REGISTRY.with(|r| {
        if let Some(Some(entry)) = r.borrow_mut().entries.get_mut(id.0 as usize) {
            entry.control = Some(control);
        }
    });
}

/// Drive navigator `id` (the bridge's `navigate` verb). The dispatch
/// stages its writes; this settles them so a query on the next line sees
/// the new screen — the `invoke_method` action contract.
pub fn navigate(id: NavId, action: NavAction) -> Result<(), String> {
    let control = NAV_REGISTRY.with(|r| {
        let reg = r.borrow();
        let entry = reg
            .entries
            .get(id.0 as usize)
            .and_then(|s| s.as_ref())
            .ok_or_else(|| format!("no navigator with id {}", id.0))?;
        entry
            .control
            .clone()
            .ok_or_else(|| format!("navigator {} ({}) takes no robot commands", id.0, entry.type_name))
    })?;
    // Run after the registry borrow drops: the dispatch marks this
    // navigator active, which borrows the registry again.
    control(action)?;
    settle();
    Ok(())
}

/// Remove a navigator (its subtree tore down). Clears `active` if it
/// pointed here. No-op for an unknown id.
pub(crate) fn deregister_navigator(id: NavId) {
    NAV_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        if let Some(slot) = reg.entries.get_mut(id.0 as usize) {
            if slot.take().is_some() {
                reg.free.push(id.0);
            }
        }
        if reg.active == Some(id) {
            reg.active = None;
        }
    });
}

/// Mark a navigator as the current one (last dispatched-against).
/// Called from the handlers' dispatch closures; bumps the live-update
/// revision.
pub(crate) fn mark_active_navigator(id: NavId) {
    NAV_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        if reg
            .entries
            .get(id.0 as usize)
            .map(|s| s.is_some())
            .unwrap_or(false)
        {
            reg.active = Some(id);
        }
    });
    bump_revision();
}

/// Snapshot every live navigator. Entries whose snapshot closure
/// reports dead shared state are skipped and pruned (dead-`Weak`
/// prune). Closures run AFTER the registry borrow drops —
/// they read signals and could re-enter. Callers wanting label-accurate
/// signal reads run this entered (the bridge does).
pub fn all_navigators() -> Vec<NavSnapshot> {
    type Collected = (u32, &'static str, Option<u32>, Rc<dyn Fn() -> Option<NavSnapshotData>>, bool, bool);
    let collected: Vec<Collected> =
        NAV_REGISTRY.with(|r| {
            let reg = r.borrow();
            let active = reg.active;
            reg.entries
                .iter()
                .enumerate()
                .filter_map(|(i, slot)| {
                    let entry = slot.as_ref()?;
                    Some((
                        i as u32,
                        entry.type_name,
                        entry.element_id,
                        entry.snapshot.clone(),
                        active == Some(NavId(i as u32)),
                        entry.control.is_some(),
                    ))
                })
                .collect()
        });
    let mut out = Vec::with_capacity(collected.len());
    let mut dead = Vec::new();
    for (id, type_name, element_id, snapshot, is_current, controllable) in collected {
        match snapshot() {
            Some(data) => out.push(NavSnapshot {
                nav_id: id,
                element_id,
                type_name,
                active_route: data.active_route,
                active_path: data.active_path,
                query: data.query,
                depth: data.depth,
                can_go_back: data.can_go_back,
                is_current,
                base: data.base,
                stack: data.stack,
                controllable,
            }),
            None => dead.push(NavId(id)),
        }
    }
    for id in dead {
        deregister_navigator(id);
    }
    out
}

/// Snapshot one navigator by id, or `None` if unknown/dead.
pub fn navigator_snapshot(id: NavId) -> Option<NavSnapshot> {
    all_navigators().into_iter().find(|s| s.nav_id == id.0)
}

// =============================================================================
// Public query/control API (mirrors `runtime_shared::robot::Robot`)
// =============================================================================

/// A snapshot of a mounted element's properties.
#[derive(Clone, Debug)]
pub struct Element {
    pub id: ElementId,
    pub kind: ElementKind,
    pub test_id: Option<&'static str>,
    pub label: Option<String>,
}

impl Element {
    fn from_registry(id: ElementId, entry: &RegistryEntry) -> Self {
        Self {
            id,
            kind: entry.kind,
            test_id: entry.test_id,
            label: entry.current_label(),
        }
    }
}

/// Criteria for finding elements.
///
/// The leaf criteria combine: [`And`](Query::And) requires all of its
/// parts (the bridge's `{kind, label, …}` object is one), and
/// [`Within`](Query::Within) scopes a query to a container's subtree.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Query {
    /// Match by author-assigned test ID (exact).
    TestId(String),
    /// Match by label text (exact).
    Label(String),
    /// Match by label text (contains substring).
    LabelContains(String),
    /// Match by primitive kind.
    Kind(ElementKind),
    /// Match the element with this id (an [`Element::id`] from an earlier
    /// query) while it stays mounted.
    Id(ElementId),
    /// Match an element every one of these matches. Empty matches all.
    And(Vec<Query>),
    /// Match an element `query` matches that is a DESCENDANT of an element
    /// `container` matches (never the container itself) — of the `nth`
    /// container in document order when `nth` is set, of any of them
    /// otherwise.
    Within {
        container: Box<Query>,
        nth: Option<usize>,
        query: Box<Query>,
    },
    /// Match all elements.
    All,
}

impl Query {
    /// `self` AND `other`.
    pub fn and(self, other: Query) -> Self {
        match self {
            Query::And(mut parts) => {
                parts.push(other);
                Query::And(parts)
            }
            Query::All => other,
            first => Query::And(vec![first, other]),
        }
    }

    /// `self`, among the descendants of what `container` matches.
    pub fn within(self, container: Query) -> Self {
        Query::Within { container: Box::new(container), nth: None, query: Box::new(self) }
    }

    pub fn id(id: ElementId) -> Self {
        Self::Id(id)
    }

    pub fn test_id(id: impl Into<String>) -> Self {
        Self::TestId(id.into())
    }

    pub fn label(text: impl Into<String>) -> Self {
        Self::Label(text.into())
    }

    pub fn label_contains(text: impl Into<String>) -> Self {
        Self::LabelContains(text.into())
    }

    pub fn kind(kind: ElementKind) -> Self {
        Self::Kind(kind)
    }
}

/// The app's viewport as [`Robot::viewport`] reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Logical px.
    pub width: f32,
    /// Logical px.
    pub height: f32,
    /// The bucket `width` falls in under the installed breakpoint table.
    pub breakpoint: runtime_shared::Breakpoint,
}

/// Why [`Robot::set_viewport`] can't resize on this platform.
pub const SET_VIEWPORT_UNSUPPORTED: &str = "set_viewport is unsupported on this platform: the app cannot \
     resize its own window here. On web, run under `idealyst test --web`, whose runner resizes the \
     page for robot-test's `set_viewport`; on iOS and Android, run on a device or simulator of the \
     size you need";

/// Errors returned by robot control operations.
#[derive(Clone, Debug, PartialEq)]
pub enum RobotError {
    /// The element has been unmounted since it was queried.
    ElementGone,
    /// The requested action is not available on this element kind.
    ActionNotAvailable(&'static str),
    /// No element matched the query.
    NotFound,
    /// The element is disabled, so it refuses the action — as the control
    /// refuses a user.
    Disabled(&'static str),
    /// A native action needs the element visible.
    NotVisible(&'static str),
    /// A native action's point is outside the window: a user couldn't
    /// reach it without scrolling.
    OutsideWindow { action: &'static str, x: f32, y: f32 },
    /// Something else is on top at the action's point and would take a
    /// real tap — an overlay, a scrim, a sibling drawn later. `by`
    /// describes the topmost registered element there, when one is.
    Covered { action: &'static str, x: f32, y: f32, by: Option<String> },
    /// This backend can't synthesize native input for the action.
    Unsupported(&'static str),
    /// The platform refused the synthesized input.
    Native(String),
}

impl std::fmt::Display for RobotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ElementGone => write!(f, "element has been unmounted"),
            Self::ActionNotAvailable(action) => {
                write!(f, "action '{action}' not available on this element")
            }
            Self::NotFound => write!(f, "no element matched the query"),
            Self::Disabled(action) => write!(f, "action '{action}' refused: the element is disabled"),
            Self::NotVisible(action) => write!(f, "action '{action}' refused: the element is not visible"),
            Self::OutsideWindow { action, x, y } => write!(
                f,
                "action '{action}' refused: the point ({x}, {y}) is outside the window (scroll the element into view)"
            ),
            Self::Covered { action, x, y, by } => {
                write!(f, "action '{action}' refused: another element covers the point ({x}, {y})")?;
                match by {
                    Some(by) => write!(f, " — {by} would receive the tap"),
                    None => Ok(()),
                }
            }
            Self::Unsupported(action) => {
                write!(f, "action '{action}' unsupported: this backend has no native input")
            }
            Self::Native(msg) => write!(f, "the platform refused the input: {msg}"),
        }
    }
}

impl std::error::Error for RobotError {}

// `Eq` by hand: the `f32` point a native action refused at is always a
// finite window coordinate (never NaN), so equality is reflexive.
impl Eq for RobotError {}

/// Refuse `action` on a disabled element (its state says so).
fn refuse_if_disabled(actions: &ElementActions, action: &'static str) -> Result<(), RobotError> {
    match &actions.state {
        Some(state) if state().is_disabled() => Err(RobotError::Disabled(action)),
        _ => Ok(()),
    }
}

/// A scroller [`Robot::scroll_into_view`] moved, and the offset it moved it
/// to (logical px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollMove {
    /// The scroller's element — with [`page`](Self::page), the app's root
    /// element, whose host scroller moved.
    pub scroll_view: ElementId,
    /// The scroller above the app's tree moved (the web page,
    /// [`ScrollTarget::Host`]), not the element's own.
    pub page: bool,
    pub x: f32,
    pub y: f32,
}

/// Where [`Robot::style`] read the style from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleSource {
    /// The toolkit's resolved style ([`IntrospectionOps::computed_style`]).
    Native,
    /// The rules the framework resolved and handed the backend.
    Framework,
}

impl StyleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            StyleSource::Native => "native",
            StyleSource::Framework => "framework",
        }
    }
}

/// The CSS weight of a [`runtime_shared::FontWeight`].
fn css_weight(w: runtime_shared::FontWeight) -> u16 {
    use runtime_shared::FontWeight as W;
    match w {
        W::Thin => 100,
        W::ExtraLight => 200,
        W::Light => 300,
        W::Normal => 400,
        W::Medium => 500,
        W::SemiBold => 600,
        W::Bold => 700,
        W::ExtraBold => 800,
        W::Black => 900,
    }
}

/// A framework-resolved style as the robot reports it: tokens resolved
/// against the active theme, colors as sRGB floats, lengths in px (a
/// percent or `auto` length has no px value and is left out).
pub fn resolved_from_rules(r: &runtime_shared::StyleRules) -> ResolvedStyle {
    use runtime_shared::{Length, TextAlign};
    let color = |c: &Option<runtime_shared::Tokenized<runtime_shared::Color>>| {
        c.as_ref().and_then(|c| runtime_shared::color::parse(&c.resolve().0).ok()).map(|c| c.to_srgb_f32())
    };
    let px = |l: &Option<runtime_shared::Tokenized<Length>>| match l.as_ref().map(|l| l.resolve()) {
        Some(Length::Px(v)) => Some(v),
        _ => None,
    };
    ResolvedStyle {
        color: color(&r.color),
        background: color(&r.background),
        opacity: r.opacity.as_ref().map(|o| o.resolve()),
        font_size: px(&r.font_size),
        font_weight: r.font_weight.map(css_weight),
        italic: r.font_style.map(|s| s == runtime_shared::FontStyle::Italic),
        font_family: r.font_family.as_ref().map(|f| match f {
            runtime_shared::FontFamily::System(name) => name.clone(),
            runtime_shared::FontFamily::Typeface(t) => t.family_name.to_string(),
        }),
        underline: r.underline,
        line_through: r.strikethrough,
        text_align: r.text_align.map(|a| {
            match a {
                TextAlign::Left => "left",
                TextAlign::Right => "right",
                TextAlign::Center => "center",
                TextAlign::Justify => "justify",
            }
            .to_string()
        }),
        border_width: r.border_top_width.as_ref().map(|w| w.resolve()),
        border_color: color(&r.border_top_color),
        corner_radius: px(&r.border_top_left_radius),
    }
}

/// `get_style`'s JSON: `{"source": …, "style": {…}}`, absent fields
/// omitted; colors `[r, g, b, a]` in `0..=1`; `text_decoration` is
/// `"none"`, `"underline"`, `"line-through"` or `"underline line-through"`
/// when either line is known.
pub fn style_json(source: StyleSource, s: &ResolvedStyle) -> String {
    use runtime_shared::__serde_json as serde_json;
    let mut m = serde_json::Map::new();
    let mut put = |k: &str, v: Option<serde_json::Value>| {
        if let Some(v) = v {
            m.insert(k.into(), v);
        }
    };
    put("color", s.color.map(|c| serde_json::json!(c)));
    put("background", s.background.map(|c| serde_json::json!(c)));
    put("opacity", s.opacity.map(|v| serde_json::json!(v)));
    put("font_size", s.font_size.map(|v| serde_json::json!(v)));
    put("font_weight", s.font_weight.map(|v| serde_json::json!(v)));
    put("font_style", s.italic.map(|i| serde_json::json!(if i { "italic" } else { "normal" })));
    put("font_family", s.font_family.clone().map(serde_json::Value::String));
    let decoration = match (s.underline, s.line_through) {
        (None, None) => None,
        (u, l) => Some(match (u == Some(true), l == Some(true)) {
            (false, false) => "none",
            (true, false) => "underline",
            (false, true) => "line-through",
            (true, true) => "underline line-through",
        }),
    };
    put("text_decoration", decoration.map(|d| serde_json::json!(d)));
    put("text_align", s.text_align.clone().map(serde_json::Value::String));
    put("border_width", s.border_width.map(|v| serde_json::json!(v)));
    put("border_color", s.border_color.map(|c| serde_json::json!(c)));
    put("corner_radius", s.corner_radius.map(|v| serde_json::json!(v)));
    serde_json::json!({ "source": source.as_str(), "style": serde_json::Value::Object(m) }).to_string()
}

/// The least scroll along one axis that brings `[start, start + len)`
/// inside `[port, port + port_len)`: `0` when it is already inside;
/// aligned to the start when it is longer than the port.
fn reveal_shift(start: f32, len: f32, port: f32, port_len: f32) -> f32 {
    let (end, port_end) = (start + len, port + port_len);
    if start >= port && end <= port_end {
        0.0
    } else if len > port_len || start < port {
        start - port
    } else {
        end - port_end
    }
}

/// The overlap of two rects, `None` when they don't overlap.
fn overlap(a: ViewportRect, b: ViewportRect) -> Option<ViewportRect> {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = ((a.x + a.width).min(b.x + b.width), (a.y + a.height).min(b.y + b.height));
    (x1 > x0 && y1 > y0).then(|| ViewportRect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
}

/// How [`Robot::tap_native`] reached the element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapMechanism {
    /// Real platform input, through the toolkit's event path.
    Native,
    /// The backend's hit test confirmed the point, then the press ran as
    /// `click` runs it, on the deepest element the hit test found there
    /// (a backend that can't synthesize input).
    HitTest,
}

impl TapMechanism {
    pub fn as_str(self) -> &'static str {
        match self {
            TapMechanism::Native => "native",
            TapMechanism::HitTest => "hit_test",
        }
    }
}

/// The deepest registered element the backend's hit test puts under
/// `(x, y)`, described for an error message.
fn topmost_at(x: f32, y: f32) -> Option<String> {
    let candidates: Vec<(ElementId, Rc<dyn Fn(f32, f32) -> Option<bool>>)> = REGISTRY.with(|r| {
        r.borrow().entries().filter_map(|(id, e)| e.actions.hit.clone().map(|h| (id, h))).collect()
    });
    let depth = |id: ElementId| {
        let mut d = 0;
        let mut cur = Some(id);
        while let Some(c) = cur {
            cur = REGISTRY.with(|r| r.borrow().get(c).and_then(|e| e.parent));
            d += 1;
        }
        d
    };
    let (id, _) = candidates
        .into_iter()
        .filter(|(_, hit)| hit(x, y) == Some(true))
        .map(|(id, _)| (id, depth(id)))
        .max_by_key(|&(_, d)| d)?;
    REGISTRY.with(|r| {
        let reg = r.borrow();
        let e = reg.get(id)?;
        Some(match (e.test_id, e.current_label()) {
            (Some(t), _) => format!("{:?} test_id={t:?}", e.kind),
            (None, Some(l)) => format!("{:?} {l:?}", e.kind),
            (None, None) => format!("{:?} #{}", e.kind, id.0),
        })
    })
}

/// The element a real tap at `(x, y)` lands on, given that the backend's
/// hit test puts the point inside `root`: the deepest element of `root`'s
/// subtree (`root` itself included) whose hit test contains the point —
/// later siblings win a tie, as they draw on top. What the hit-test tier
/// of [`Robot::tap_native`] presses, so a test that addresses a wrapper
/// reaches the pressable under its center, as a finger (and the web's
/// `elementFromPoint`) does.
fn deepest_hit_in(root: ElementId, x: f32, y: f32) -> ElementId {
    // Closures cloned out first: a hit test borrows the backend, never
    // with the registry borrowed.
    let candidates: Vec<(ElementId, usize, Rc<dyn Fn(f32, f32) -> Option<bool>>)> = REGISTRY.with(|r| {
        let reg = r.borrow();
        let mut out = Vec::new();
        let mut stack = vec![(root, 0usize)];
        while let Some((id, depth)) = stack.pop() {
            let Some(entry) = reg.get(id) else { continue };
            if let Some(hit) = entry.actions.hit.clone() {
                out.push((id, depth, hit));
            }
            stack.extend(entry.children.iter().rev().map(|&c| (c, depth + 1)));
        }
        out
    });
    candidates
        .into_iter()
        .filter(|(_, _, hit)| hit(x, y) == Some(true))
        .map(|(id, depth, _)| (id, depth))
        .max_by_key(|&(_, depth)| depth)
        .map_or(root, |(id, _)| id)
}

/// Element `id`'s [`Visibility`]; `None` once it has unmounted.
fn visibility_of(id: ElementId) -> Option<Visibility> {
    visibility_and_frame(id).map(|(v, _)| v)
}

/// Element `id`'s [`Visibility`] and the part of its box left on screen
/// ([`Robot::visible_frame`]); `None` once it has unmounted.
///
/// The clipping ancestors are the element's REGISTRY ancestors, so a
/// virtualized row — realized detached, in its own native cell — is cut by
/// its list (`handlers::virtualizer` links rows under the list's element).
fn visibility_and_frame(id: ElementId) -> Option<(Visibility, Option<ViewportRect>)> {
    type Ancestor = (Rc<dyn Fn() -> Option<ContentClip>>, Rc<dyn Fn() -> Option<ViewportRect>>);
    // Closures cloned out first: they borrow the backend, never with the
    // registry borrowed.
    let (shown, frame, ancestors) = REGISTRY.with(|r| {
        let reg = r.borrow();
        let e = reg.get(id)?;
        // Innermost first: `Visibility::clipped` reads the order.
        // Up to the first element drawn elsewhere: a portal's content is
        // not inside the boxes of the element that opened it.
        let mut ancestors: Vec<Ancestor> = Vec::new();
        let mut cur = (!e.detached).then_some(e.parent).flatten();
        while let Some(pid) = cur {
            let Some(p) = reg.get(pid) else { break };
            if let (Some(clip), Some(frame)) = (p.actions.clip.clone(), p.actions.absolute_frame.clone()) {
                ancestors.push((clip, frame));
            }
            cur = (!p.detached).then_some(p.parent).flatten();
        }
        Some((e.actions.shown.clone(), e.actions.absolute_frame.clone(), ancestors))
    })?;
    let frame = frame.and_then(|f| f());
    let clips: Vec<(ContentClip, ViewportRect)> = if frame.is_some() {
        ancestors
            .iter()
            .filter_map(|(clip, rect)| match clip()? {
                ContentClip::Visible => None,
                kind => Some((kind, rect()?)),
            })
            .collect()
    } else {
        Vec::new()
    };
    let window = runtime_shared::viewport::viewport_size().get();
    let window = (window.width > 0.0 && window.height > 0.0).then_some((window.width, window.height));
    Some(Visibility::clipped(shown.and_then(|f| f()), frame, &clips, window))
}

/// The registered state of element `id`, if it has any.
fn state_of(id: ElementId) -> Option<ElementState> {
    let (f, interaction, native_keyboard) = REGISTRY.with(|r| {
        r.borrow()
            .get(id)
            .map(|e| (e.actions.state.clone(), e.actions.interaction.clone(), e.actions.native_keyboard.clone()))
            .unwrap_or_default()
    });
    let mut state = match (f, &interaction) {
        (Some(f), _) => f(),
        (None, Some(_)) => ElementState::default(),
        (None, None) => return None,
    };
    if let Some(cell) = interaction {
        let bits = cell.get();
        state.hovered = Some(bits.contains(runtime_shared::StateBits::HOVERED));
        state.pressed = Some(bits.contains(runtime_shared::StateBits::PRESSED));
    }
    // Only a field that takes a keyboard kind (a `text_input`, not a
    // `text_area`) reports the native one.
    if state.keyboard.is_some() {
        state.native_keyboard = native_keyboard.and_then(|f| f());
    }
    Some(state)
}

runtime_tls::thread_local! {
    /// Set by a platform boot that learns an external driver owns the app
    /// another way than the environment (web: a page global).
    static TEST_DRIVEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether an external test driver (`idealyst test`) owns this app — so an
/// in-app self-test should stand down rather than race the driver's suite
/// for the same state. `IDEALYST_TEST_DRIVER` in the environment on native
/// (the dev tooling forwards it to Android, iOS and remote Windows apps,
/// which inherit nothing); on web the boot reads the page global the dev
/// server injects ([`set_test_driven`]).
pub fn is_test_driven() -> bool {
    TEST_DRIVEN.with(|t| t.get())
        || (!cfg!(target_arch = "wasm32") && std::env::var_os("IDEALYST_TEST_DRIVER").is_some())
}

/// The running app's platform as the robot names it to a test driver
/// (the `get_platform` verb): `web`, `ios`, `android`, `macos`, `linux`,
/// `windows` — the spellings of `idealyst test --<platform>` and of
/// `#[robot_test(platforms = …)]` — or `tvos`, `androidtv`, `roku`, a
/// backend's own `Custom` name, `unknown` when none was declared. Read
/// from [`runtime_shared::host::platform`], which every boot seeds from
/// its backend's `AppEnvOps::platform`.
pub fn platform_id() -> &'static str {
    use runtime_shared::Platform;
    match runtime_shared::host::platform() {
        Platform::Web => "web",
        Platform::Ios => "ios",
        Platform::Android => "android",
        Platform::MacOs => "macos",
        Platform::TvOs => "tvos",
        Platform::AndroidTv => "androidtv",
        Platform::Roku => "roku",
        Platform::Custom("") => "unknown",
        Platform::Custom(name) => name,
    }
}

/// Mark the app as owned by an external test driver (see [`is_test_driven`]).
pub fn set_test_driven(on: bool) {
    TEST_DRIVEN.with(|t| t.set(on));
}

/// A recursive snapshot of the mounted tree.
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub id: ElementId,
    pub kind: ElementKind,
    pub test_id: Option<&'static str>,
    pub label: Option<String>,
    pub children: Vec<TreeNode>,
}

/// A [`Query`] with its [`Within`](Query::Within) containers resolved to
/// element ids once, so a scan doesn't re-run the container query per
/// candidate.
enum Matcher<'q> {
    Leaf(&'q Query),
    And(Vec<Matcher<'q>>),
    Within(std::collections::HashSet<ElementId>, Box<Matcher<'q>>),
}

impl<'q> Matcher<'q> {
    fn compile(reg: &Registry, query: &'q Query) -> Self {
        match query {
            Query::And(parts) => Matcher::And(parts.iter().map(|q| Matcher::compile(reg, q)).collect()),
            Query::Within { container, nth, query } => {
                let found = matches_in_order(reg, &Matcher::compile(reg, container));
                let containers = match nth {
                    Some(n) => found.into_iter().nth(*n).into_iter().collect(),
                    None => found.into_iter().collect(),
                };
                Matcher::Within(containers, Box::new(Matcher::compile(reg, query)))
            }
            leaf => Matcher::Leaf(leaf),
        }
    }

    fn matches(&self, reg: &Registry, id: ElementId, entry: &RegistryEntry) -> bool {
        match self {
            Matcher::Leaf(query) => match query {
                Query::TestId(t) => entry.test_id == Some(t.as_str()),
                Query::Label(text) => entry.current_label().as_deref() == Some(text.as_str()),
                Query::LabelContains(text) => entry
                    .current_label()
                    .is_some_and(|l| l.contains(text.as_str())),
                Query::Kind(kind) => entry.kind == *kind,
                Query::Id(want) => id == *want,
                Query::All => true,
                // Compiled into the other arms.
                Query::And(_) | Query::Within { .. } => unreachable!("compound query compiled as a leaf"),
            },
            // Cheap criteria first is the caller's ordering; a label read
            // (a `label_fn` call) only happens for candidates still in.
            Matcher::And(parts) => parts.iter().all(|m| m.matches(reg, id, entry)),
            Matcher::Within(containers, inner) => {
                !containers.is_empty()
                    && has_ancestor_in(reg, entry, containers)
                    && inner.matches(reg, id, entry)
            }
        }
    }
}

/// Whether some proper ancestor of `entry` is in `set`.
fn has_ancestor_in(reg: &Registry, entry: &RegistryEntry, set: &std::collections::HashSet<ElementId>) -> bool {
    let mut cur = entry.parent;
    while let Some(pid) = cur {
        if set.contains(&pid) {
            return true;
        }
        cur = reg.get(pid).and_then(|p| p.parent);
    }
    false
}

/// Every element `m` matches, in document order.
fn matches_in_order(reg: &Registry, m: &Matcher<'_>) -> Vec<ElementId> {
    reg.document_order()
        .into_iter()
        .filter(|id| reg.get(*id).is_some_and(|e| m.matches(reg, *id, e)))
        .collect()
}

/// The programmatic controller for a running UI. Thread-local
/// (same thread as the mount); a ZST indexing the thread-local
/// registry, cheap to construct.
///
/// API mirrors `runtime_shared::robot::Robot`'s element surface so the
/// bridge's verb handlers adapt mechanically.
#[derive(Default)]
pub struct Robot;

impl Robot {
    pub fn new() -> Self {
        Self
    }

    /// Find the first element matching `query`. For `TestId`, resolves
    /// via the last-wins index (duplicate policy — see module docs).
    /// Runs entered (label queries resolve `label_fn` — driver env).
    pub fn find(&self, query: Query) -> Option<Element> {
        entered(|| self.find_inner(query))
    }

    fn find_inner(&self, query: Query) -> Option<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            match query {
                Query::TestId(ref id) => {
                    let eid = reg.by_test_id.get(id.as_str()).copied()?;
                    reg.get(eid).map(|e| Element::from_registry(eid, e))
                }
                ref query => {
                    let m = Matcher::compile(&reg, query);
                    reg.document_order().into_iter().find_map(|id| {
                        let entry = reg.get(id)?;
                        m.matches(&reg, id, entry).then(|| Element::from_registry(id, entry))
                    })
                }
            }
        })
    }

    /// Find all elements matching `query`. For `TestId`, scans ALL
    /// entries (not the 1:1 index) so duplicates are all returned.
    /// Runs entered (label queries resolve `label_fn` — driver env).
    pub fn find_all(&self, query: Query) -> Vec<Element> {
        entered(|| self.find_all_inner(query))
    }

    fn find_all_inner(&self, query: Query) -> Vec<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            let m = Matcher::compile(&reg, &query);
            matches_in_order(&reg, &m)
                .into_iter()
                .filter_map(|id| reg.get(id).map(|entry| Element::from_registry(id, entry)))
                .collect()
        })
    }

    /// Count mounted elements, optionally filtered by kind.
    pub fn count(&self, kind: Option<ElementKind>) -> usize {
        match kind {
            Some(k) => self.find_all(Query::Kind(k)).len(),
            None => self.find_all(Query::All).len(),
        }
    }

    /// Tap the element, the way a finger at its center would.
    ///
    /// The press goes to the nearest element at or above `element` that
    /// can take one — the same deepest-first, bubble-up rule real touches
    /// follow (`runtime_shared::touch` module docs):
    ///
    /// - a `pressable` / `button` / `link` runs its press callback;
    /// - a view (or extension node) with an `on_touch` handler receives a
    ///   synthesized `Began` → `Ended` at the element's center; if it
    ///   ignores the `Began`, the tap bubbles on to the next ancestor.
    ///
    /// The second case is what makes tap recognizers reachable: an idea-ui
    /// Table row, for one, is a `tap_with_press` on each cell's `on_touch`,
    /// so without it `click` would report "not available" for everything
    /// in it.
    pub fn click(&self, element: &Element) -> Result<(), RobotError> {
        for (id, actions) in self.press_chain(element.id)? {
            if let Some(f) = actions.click.clone() {
                refuse_if_disabled(&actions, "click")?;
                f();
                settle();
                return Ok(());
            }
            if let Some(handler) = actions.touch {
                let point = self.center_in_window(element.id);
                let mut stream = SyntheticTouch::new(handler, self.window_origin(id));
                if stream.deliver(TouchPhase::Began, point) {
                    stream.deliver(TouchPhase::Ended, point);
                    return Ok(());
                }
            }
        }
        Err(RobotError::ActionNotAvailable("click"))
    }

    /// Tap `element` with real platform input: the backend's
    /// [`native_pointer`](IntrospectionOps::native_pointer) presses and
    /// lifts at the element's center through the toolkit's own event path,
    /// so hit testing, overlays, gesture recognizers and the native
    /// disabled state all take part — what [`click`](Self::click), which
    /// calls the element's handler directly, cannot test.
    ///
    /// Refuses, as a user would fail, when the element is not visible,
    /// its center is outside the window, or the backend's hit test
    /// ([`hit_contains`](IntrospectionOps::hit_contains)) finds something
    /// else on top. A backend with a hit test but no input injection taps
    /// through the hit test: the point is checked, then the press runs as
    /// [`click`](Self::click) runs it ([`TapMechanism::HitTest`]) — on the
    /// deepest element the hit test puts under the point, not on `element`
    /// itself, so tapping a wrapper presses the pressable a finger at its
    /// center would land on, as real input does. A point over nothing
    /// pressable is a tap that lands and runs nothing, not an error, on
    /// either tier.
    /// With neither, [`RobotError::Unsupported`].
    pub fn tap_native(&self, element: &Element) -> Result<TapMechanism, RobotError> {
        const ACTION: &str = "tap_native";
        let (x, y) = self.native_point(element.id, None, ACTION)?;
        let hit = self.check_hit(element.id, x, y, ACTION)?;
        let steps = [
            PointerStep { phase: PointerStepPhase::Down, x, y },
            PointerStep { phase: PointerStepPhase::Up, x, y },
        ];
        if self.inject(element.id, &steps)? {
            return Ok(TapMechanism::Native);
        }
        if hit == Some(true) {
            let target = deepest_hit_in(element.id, x, y);
            let target = REGISTRY
                .with(|r| r.borrow().get(target).map(|e| Element::from_registry(target, e)))
                .ok_or(RobotError::ElementGone)?;
            // Nothing pressable under the point is not a failure: a finger
            // there lands and nothing runs, exactly what the native-input
            // tier reports for the same tap. A disabled control still refuses.
            match self.click(&target) {
                Ok(()) | Err(RobotError::ActionNotAvailable(_)) => {}
                Err(e) => return Err(e),
            }
            return Ok(TapMechanism::HitTest);
        }
        Err(RobotError::Unsupported(ACTION))
    }

    /// Move a real platform pointer over `element`'s center with no button
    /// held, through the toolkit's own event path — what lights `on_hover`
    /// and the `state hovered` style. Moving it over another element is
    /// how a test leaves this one. Checked as [`tap_native`](Self::tap_native)
    /// checks its point. No fallback: a backend without hover injection
    /// (touch platforms) answers [`RobotError::Unsupported`].
    pub fn hover_native(&self, element: &Element) -> Result<(), RobotError> {
        const ACTION: &str = "hover_native";
        let (x, y) = self.native_point(element.id, None, ACTION)?;
        self.check_hit(element.id, x, y, ACTION)?;
        if self.inject(element.id, &[PointerStep { phase: PointerStepPhase::Hover, x, y }])? {
            Ok(())
        } else {
            Err(RobotError::Unsupported(ACTION))
        }
    }

    /// Press the primary button (or a finger) down on `element`'s center
    /// with real platform input, and keep it down: what lights the
    /// `state pressed` style, and — held — what a long press is
    /// ([`release_native`](Self::release_native) lifts it, after as much
    /// real time as the test waits). Split in two so a test can check the
    /// pressed state in between; a backend that posts its events (macOS)
    /// delivers them on the next run-loop turn, so poll for the state.
    /// Checked as [`tap_native`](Self::tap_native) checks its point.
    ///
    /// A backend with a hit test but no input injection (iOS) presses
    /// through the hit test, as `tap_native` taps: on the deepest element
    /// under the point, a `Began` into its touch handlers — the path the
    /// framework's long-press recognizer and the dnd SDK's hold pickup
    /// arm their timers on — held until the release delivers `Ended`; a
    /// pressable there runs its press on the release. Every element from
    /// that one up to the root goes `pressed` for the hold, through the
    /// setter the backend's own press tracking drives: UIKit hands a touch
    /// to the recognizers of the hit view and of each of its ancestors, and
    /// nothing else in this tier would light the state.
    pub fn press_native(&self, element: &Element) -> Result<TapMechanism, RobotError> {
        const ACTION: &str = "press_native";
        let (x, y) = self.native_point(element.id, None, ACTION)?;
        let hit = self.check_hit(element.id, x, y, ACTION)?;
        if self.inject(element.id, &[PointerStep { phase: PointerStepPhase::Down, x, y }])? {
            return Ok(TapMechanism::Native);
        }
        if hit != Some(true) {
            return Err(RobotError::Unsupported(ACTION));
        }
        let target = deepest_hit_in(element.id, x, y);
        let point = TouchPoint::new(x, y);
        let mut held = HeldPress::Nothing;
        for (id, actions) in self.press_chain(target)? {
            if actions.click.is_some() {
                refuse_if_disabled(&actions, ACTION)?;
                held = HeldPress::Click(id);
                break;
            }
            let Some(handler) = actions.touch else { continue };
            let mut stream = SyntheticTouch::new(handler, self.window_origin(id));
            if stream.deliver(TouchPhase::Began, point) {
                held = HeldPress::Stream(stream);
                break;
            }
        }
        // A press still held from before is abandoned: its stream ends here
        // rather than leaving a handler waiting for an `Ended` forever.
        let previous = HELD_PRESS.with(|h| h.replace(held));
        if let HeldPress::Stream(mut stream) = previous {
            stream.catch_up();
            stream.deliver(TouchPhase::Cancelled, point);
        }
        unlight_held_press();
        let lit: Vec<_> = self
            .press_chain(target)?
            .into_iter()
            .filter_map(|(_, actions)| actions.set_interaction)
            .collect();
        for set in &lit {
            set(runtime_shared::StateBits::PRESSED, true);
        }
        HELD_LIT.with(|h| *h.borrow_mut() = lit);
        settle();
        Ok(TapMechanism::HitTest)
    }

    /// Lift the press [`press_native`](Self::press_native) started, at
    /// `element`'s center. Not hit-checked: the release goes wherever the
    /// pointer is, as a real one does. Through the hit-test tier, the held
    /// stream gets its `Ended` stamped with the real time that passed since
    /// the press, so duration-reading recognizers see the hold.
    pub fn release_native(&self, element: &Element) -> Result<TapMechanism, RobotError> {
        const ACTION: &str = "release_native";
        let (x, y) = self.native_point(element.id, None, ACTION)?;
        if self.inject(element.id, &[PointerStep { phase: PointerStepPhase::Up, x, y }])? {
            return Ok(TapMechanism::Native);
        }
        unlight_held_press();
        match HELD_PRESS.with(|h| h.replace(HeldPress::Nothing)) {
            HeldPress::Stream(mut stream) => {
                stream.catch_up();
                stream.deliver(TouchPhase::Ended, TouchPoint::new(x, y));
                Ok(TapMechanism::HitTest)
            }
            HeldPress::Click(id) => {
                let target = REGISTRY
                    .with(|r| r.borrow().get(id).map(|e| Element::from_registry(id, e)))
                    .ok_or(RobotError::ElementGone)?;
                self.click(&target)?;
                Ok(TapMechanism::HitTest)
            }
            HeldPress::Nothing => {
                let hit = REGISTRY.with(|r| r.borrow().get(element.id).and_then(|e| e.actions.hit.clone()));
                match hit.and_then(|h| h(x, y)) {
                    Some(_) => Ok(TapMechanism::HitTest),
                    None => Err(RobotError::Unsupported(ACTION)),
                }
            }
        }
    }

    /// Click the secondary (right) mouse button on `element`'s center
    /// with real platform input — a context click, what opens a
    /// right-click menu. It reaches touch handlers as a
    /// `PointerButton::Secondary` `Began` (Began-only, the secondary-press
    /// contract), through the toolkit's own event path: on web the page's
    /// `pointerdown` / `mousedown` / `contextmenu` with `button: 2`.
    /// Checked as [`tap_native`](Self::tap_native) checks its point.
    ///
    /// A backend whose injection has no secondary button (a touch
    /// platform) or no injection at all, but a hit test, delivers the
    /// secondary `Began` through the hit test: into the touch handlers of
    /// the deepest element under the point, bubbling to an ancestor that
    /// ignores it, stopping at a pressable — which swallows a secondary
    /// press as it does on every backend. On a phone the platform's own
    /// context gesture is a long press: hold [`press_native`](Self::press_native)
    /// for it instead.
    pub fn context_click_native(&self, element: &Element) -> Result<TapMechanism, RobotError> {
        const ACTION: &str = "context_click";
        let (x, y) = self.native_point(element.id, None, ACTION)?;
        let hit = self.check_hit(element.id, x, y, ACTION)?;
        if self.inject(element.id, &[PointerStep { phase: PointerStepPhase::SecondaryClick, x, y }])? {
            return Ok(TapMechanism::Native);
        }
        if hit != Some(true) {
            return Err(RobotError::Unsupported(ACTION));
        }
        let target = deepest_hit_in(element.id, x, y);
        let point = TouchPoint::new(x, y);
        for (id, actions) in self.press_chain(target)? {
            let Some(handler) = actions.touch else {
                if actions.click.is_some() {
                    break;
                }
                continue;
            };
            let mut stream = SyntheticTouch::new(handler, self.window_origin(id));
            stream.button = runtime_shared::PointerButton::Secondary;
            if stream.deliver(TouchPhase::Began, point) {
                break;
            }
        }
        Ok(TapMechanism::HitTest)
    }

    /// Drag with real platform input along `points` — relative to
    /// `element`'s top-left, logical px, as [`drag`](Self::drag) takes
    /// them: down at the first, moving through the rest, up at the last.
    /// The first point is checked as [`tap_native`](Self::tap_native)
    /// checks its center. No hit-test fallback: a backend without input
    /// injection answers [`RobotError::Unsupported`] (use `drag`).
    pub fn drag_native(&self, element: &Element, points: &[(f32, f32)]) -> Result<(), RobotError> {
        const ACTION: &str = "drag_native";
        let (&first, rest) = points.split_first().ok_or(RobotError::ActionNotAvailable("drag_native (no points)"))?;
        let (x0, y0) = self.native_point(element.id, Some(first), ACTION)?;
        self.check_hit(element.id, x0, y0, ACTION)?;
        let origin = self.window_origin(element.id);
        let mut steps = vec![PointerStep { phase: PointerStepPhase::Down, x: x0, y: y0 }];
        for &(x, y) in rest {
            steps.push(PointerStep { phase: PointerStepPhase::Move, x: origin.x + x, y: origin.y + y });
        }
        let last = steps.last().map(|s| (s.x, s.y)).unwrap_or((x0, y0));
        steps.push(PointerStep { phase: PointerStepPhase::Up, x: last.0, y: last.1 });
        if self.inject(element.id, &steps)? {
            Ok(())
        } else {
            Err(RobotError::Unsupported(ACTION))
        }
    }

    /// The window point a native action on `id` starts at: `offset` from
    /// its top-left, or its center. Refuses an invisible element and a
    /// point outside the window.
    fn native_point(&self, id: ElementId, offset: Option<(f32, f32)>, action: &'static str) -> Result<(f32, f32), RobotError> {
        let vis = visibility_of(id).ok_or(RobotError::ElementGone)?;
        if !vis.visible {
            return Err(RobotError::NotVisible(action));
        }
        let rect = REGISTRY
            .with(|r| r.borrow().get(id).and_then(|e| e.actions.absolute_frame.clone()))
            .and_then(|f| f())
            .ok_or(RobotError::Unsupported(action))?;
        let (x, y) = match offset {
            Some((dx, dy)) => (rect.x + dx, rect.y + dy),
            None => (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0),
        };
        let window = runtime_shared::viewport::viewport_size().get();
        if window.width > 0.0 && window.height > 0.0 && !(x >= 0.0 && y >= 0.0 && x < window.width && y < window.height) {
            return Err(RobotError::OutsideWindow { action, x, y });
        }
        Ok((x, y))
    }

    /// The backend's hit test at `(x, y)` for `id`: `Err(Covered)` when
    /// something else would take the tap.
    fn check_hit(&self, id: ElementId, x: f32, y: f32, action: &'static str) -> Result<Option<bool>, RobotError> {
        let hit = REGISTRY.with(|r| r.borrow().get(id).and_then(|e| e.actions.hit.clone()));
        let answer = hit.and_then(|h| h(x, y));
        if answer == Some(false) {
            return Err(RobotError::Covered { action, x, y, by: topmost_at(x, y) });
        }
        Ok(answer)
    }

    /// Run the backend's injection for `steps`; `false` when it has none.
    fn inject(&self, id: ElementId, steps: &[PointerStep]) -> Result<bool, RobotError> {
        let prepare = REGISTRY
            .with(|r| r.borrow().get(id).and_then(|e| e.actions.native_pointer.clone()))
            .ok_or(RobotError::ElementGone)?;
        let Some(injection) = prepare(steps) else { return Ok(false) };
        injection().map_err(RobotError::Native)?;
        settle();
        Ok(true)
    }

    /// Drag a single pointer along `points` — coordinates relative to
    /// `element`'s top-left, in logical px — as `Began` at the first
    /// point, `Moved` through the rest, `Ended` at the last. Delivered
    /// through the touch handlers exactly as a platform would deliver
    /// them (bubbling `Began` from `element` to the first handler that
    /// consumes it, which then owns the whole stream), so signature pads,
    /// pan recognizers, sliders and drag-to-reorder all see a real
    /// gesture. One point is a press-and-release in place.
    ///
    /// Events are spaced [`SYNTHETIC_FRAME_NS`] apart, and staged writes
    /// settle after each one, as a real event loop flushes between
    /// pointer events. The whole path is delivered inside this one call,
    /// so no real time passes between events. To press and hold before
    /// moving — a long-press pickup, the dnd SDK's default on phones and
    /// tablets — use [`drag_with_hold`](Self::drag_with_hold).
    ///
    /// On a `slider` (a native widget with no touch handler) each point's
    /// `x` across the element's width sets the value — `0` is `min`, the
    /// right edge is `max`, `y` is ignored — through the same step-snapped,
    /// disabled-gated change callback the native control reports to, and
    /// the native thumb moves with it. That needs the slider's laid-out
    /// frame; a disabled slider refuses with an error.
    pub fn drag(&self, element: &Element, points: &[(f32, f32)]) -> Result<(), RobotError> {
        self.drag_with_hold(element, points, 0)
    }

    /// [`drag`](Self::drag), pressing and holding at the first point for
    /// `hold_ms` before the path continues: after `Began`, a stationary
    /// `Moved` stamped `hold_ms` later.
    ///
    /// No real time passes (the call is synchronous, so app timers cannot
    /// run); the hold is carried by the events' timestamps. Long-press
    /// recognizers that read the event clock as well as their timer see a
    /// real hold — the dnd SDK's `DragRecognizer` does, so a long-press
    /// drag-to-reorder picks up exactly as a held finger would on iOS and
    /// Android. `hold_ms = 0` is [`drag`](Self::drag). Ignored on a slider.
    pub fn drag_with_hold(
        &self,
        element: &Element,
        points: &[(f32, f32)],
        hold_ms: u64,
    ) -> Result<(), RobotError> {
        let (&first, rest) = points
            .split_first()
            .ok_or(RobotError::ActionNotAvailable("drag (no points)"))?;
        let chain = self.press_chain(element.id)?;
        if let Some(slide) = chain.first().and_then(|(_, a)| a.slider_drag.clone()) {
            return self.drag_slider(element.id, &slide, points);
        }
        let origin = self.window_origin(element.id);
        let to_window = |(x, y): (f32, f32)| TouchPoint::new(origin.x + x, origin.y + y);
        for (id, actions) in chain {
            let Some(handler) = actions.touch else { continue };
            let mut stream = SyntheticTouch::new(handler, self.window_origin(id));
            if !stream.deliver(TouchPhase::Began, to_window(first)) {
                continue;
            }
            if hold_ms > 0 {
                stream.hold(hold_ms, to_window(first));
            }
            let last = rest.last().copied().unwrap_or(first);
            if let Some((_, moves)) = rest.split_last() {
                for &p in moves {
                    stream.deliver(TouchPhase::Moved, to_window(p));
                }
                stream.deliver(TouchPhase::Moved, to_window(last));
            }
            stream.deliver(TouchPhase::Ended, to_window(last));
            return Ok(());
        }
        Err(RobotError::ActionNotAvailable("drag"))
    }

    /// [`drag`](Self::drag) on a slider: each point's `x` over the track
    /// width becomes a value, one change per point, settled in between.
    fn drag_slider(
        &self,
        id: ElementId,
        slide: &Rc<dyn Fn(f32) -> bool>,
        points: &[(f32, f32)],
    ) -> Result<(), RobotError> {
        let width = REGISTRY
            .with(|r| r.borrow().get(id).and_then(|e| e.actions.absolute_frame.clone()))
            .and_then(|f| f())
            .map(|r| r.width)
            .filter(|w| *w > 0.0)
            .ok_or(RobotError::ActionNotAvailable(
                "drag (the slider has no laid-out frame to map points onto)",
            ))?;
        for &(x, _) in points {
            if !slide(x / width) {
                return Err(RobotError::ActionNotAvailable("drag (the slider is disabled)"));
            }
            settle();
        }
        Ok(())
    }

    /// `id` and its ancestors, innermost first, with their actions cloned
    /// out so no registry borrow is held while author code runs.
    fn press_chain(&self, id: ElementId) -> Result<Vec<(ElementId, ElementActions)>, RobotError> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            let mut chain = Vec::new();
            let mut cur = Some(id);
            while let Some(c) = cur {
                let Some(entry) = reg.get(c) else {
                    if c == id {
                        return Err(RobotError::ElementGone);
                    }
                    break;
                };
                chain.push((c, entry.actions.clone()));
                cur = entry.parent;
            }
            Ok(chain)
        })
    }

    /// The element's top-left in window coordinates; the origin when it
    /// has no layout (headless hosts), which keeps element-relative
    /// points meaningful for a handler on the element itself.
    fn window_origin(&self, id: ElementId) -> TouchPoint {
        let rect = REGISTRY
            .with(|r| r.borrow().get(id).and_then(|e| e.actions.absolute_frame.clone()))
            .and_then(|f| f());
        rect.map_or(TouchPoint::ZERO, |r| TouchPoint::new(r.x, r.y))
    }

    fn center_in_window(&self, id: ElementId) -> TouchPoint {
        let rect = REGISTRY
            .with(|r| r.borrow().get(id).and_then(|e| e.actions.absolute_frame.clone()))
            .and_then(|f| f());
        rect.map_or(TouchPoint::ZERO, |r| {
            TouchPoint::new(r.x + r.width / 2.0, r.y + r.height / 2.0)
        })
    }

    /// Replace a text input's value (paste-like).
    pub fn type_text(&self, element: &Element, text: &str) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a
                .set_text
                .clone()
                .ok_or(RobotError::ActionNotAvailable("type_text"))?;
            refuse_if_disabled(a, "type_text")?;
            f(text.to_string());
            Ok(())
        })
    }

    /// Set a toggle's value.
    pub fn set_toggle(&self, element: &Element, value: bool) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a
                .set_toggle
                .clone()
                .ok_or(RobotError::ActionNotAvailable("set_toggle"))?;
            refuse_if_disabled(a, "set_toggle")?;
            f(value);
            Ok(())
        })
    }

    /// Set a slider's value.
    pub fn set_slider(&self, element: &Element, value: f32) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a
                .set_slider
                .clone()
                .ok_or(RobotError::ActionNotAvailable("set_slider"))?;
            refuse_if_disabled(a, "set_slider")?;
            f(value);
            Ok(())
        })
    }

    /// Focus an element (text inputs).
    pub fn focus(&self, element: &Element) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a.focus.clone().ok_or(RobotError::ActionNotAvailable("focus"))?;
            refuse_if_disabled(a, "focus")?;
            f();
            Ok(())
        })
    }

    /// Blur (unfocus) an element.
    pub fn blur(&self, element: &Element) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a.blur.clone().ok_or(RobotError::ActionNotAvailable("blur"))?;
            f();
            Ok(())
        })
    }

    /// Set a scroll view's offset (logical px/points).
    pub fn set_scroll(&self, element: &Element, x: f32, y: f32) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a
                .set_scroll
                .clone()
                .ok_or(RobotError::ActionNotAvailable("set_scroll"))?;
            f(x, y);
            Ok(())
        })
    }

    /// Scroll `element` into view, as Playwright's `scrollIntoViewIfNeeded`
    /// does: each scroller it sits in, innermost first, moves the least
    /// that shows the whole element inside that scroller — and inside the
    /// window, for the outermost one. An element larger than a scroller is
    /// aligned to its top/left. A scroller moves only along the axes it
    /// scrolls, and never past its range. Already visible: nothing moves.
    /// Returns the scrollers it moved, with their new offsets.
    ///
    /// The scrollers are every registered ancestor the toolkit scrolls —
    /// a `scroll_view`, and any other element whose native object scrolls
    /// ([`IntrospectionOps::scroll_port`]: a list's own scroller, a web
    /// element whose CSS `overflow` scrolls) — then the scroller above the
    /// app's tree, where there is one ([`ScrollTarget::Host`]: the web page,
    /// whose document scrolls when the app's root outgrows the window). A
    /// `scroll_view` moves through its own `set_scroll` route, any other
    /// through [`IntrospectionOps::set_scroll_offset`].
    ///
    /// Works the same on every backend because it reads only what each
    /// reports — window frames ([`IntrospectionOps::absolute_frame`]) and
    /// the scrollers' offsets and ranges. The shifts are computed from ONE
    /// read of the frames, predicting that scrolling a view by `d` moves
    /// everything inside it by `-d`, rather than re-reading frames between
    /// scrolls: a backend may settle a scroll's frames after the call
    /// returns (a deferred layout pass), and a re-read there would see the
    /// old frames and scroll again.
    ///
    /// Only what is mounted can be scrolled to: a virtualized list's rows
    /// beyond its window aren't elements yet.
    pub fn scroll_into_view(&self, element: &Element) -> Result<Vec<ScrollMove>, RobotError> {
        const ACTION: &str = "scroll_into_view";
        type PortFn = Rc<dyn Fn(ScrollTarget) -> Option<ScrollPort>>;
        type SetPortFn = Rc<dyn Fn(ScrollTarget, f32, f32) -> Option<NativeInjection>>;
        /// A registered ancestor (or, `page`, the root's host scroller)
        /// that may scroll, with its closures cloned out of the registry.
        struct Candidate {
            id: ElementId,
            page: bool,
            set_scroll: Option<Rc<dyn Fn(f32, f32)>>,
            axis: Option<runtime_shared::primitives::scroll_view::ScrollAxis>,
            frame: Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
            port: Option<PortFn>,
            set_port: Option<SetPortFn>,
        }
        // Closures cloned out under one borrow; none runs with it held.
        let (frame, candidates) = REGISTRY.with(|r| {
            let reg = r.borrow();
            let entry = reg.get(element.id).ok_or(RobotError::ElementGone)?;
            let mut out: Vec<Candidate> = Vec::new();
            // Up to the first element drawn elsewhere (a portal's
            // content): the scrollers around its opener don't move it, and
            // the page candidate is whatever scrolls around IT.
            let mut root = (element.id, entry);
            let mut cur = (!entry.detached).then_some(entry.parent).flatten();
            while let Some(pid) = cur {
                let Some(p) = reg.get(pid) else { break };
                out.push(Candidate {
                    id: pid,
                    page: false,
                    set_scroll: p.actions.set_scroll.clone(),
                    axis: p.actions.scroll_axis,
                    frame: p.actions.absolute_frame.clone(),
                    port: p.actions.scroll_port.clone(),
                    set_port: p.actions.set_scroll_port.clone(),
                });
                root = (pid, p);
                cur = (!p.detached).then_some(p.parent).flatten();
            }
            out.push(Candidate {
                id: root.0,
                page: true,
                set_scroll: None,
                axis: None,
                frame: None,
                port: root.1.actions.scroll_port.clone(),
                set_port: root.1.actions.set_scroll_port.clone(),
            });
            Ok::<_, RobotError>((entry.actions.absolute_frame.clone(), out))
        })?;
        // No laid-out box (not laid out yet, or a backend that doesn't
        // report one): nothing to scroll to.
        let mut target = frame.and_then(|f| f()).ok_or(RobotError::ActionNotAvailable(ACTION))?;
        let window = runtime_shared::viewport::viewport_size().get();
        let window = (window.width > 0.0 && window.height > 0.0)
            .then_some(ViewportRect { x: 0.0, y: 0.0, width: window.width, height: window.height });

        /// A candidate that scrolls, read once.
        struct Live {
            candidate: Candidate,
            port: ViewportRect,
            offset: (f32, f32),
            /// Per axis: `Some(max)` when the toolkit reports the range,
            /// `None` when only the declared axis is known (no clamp at
            /// the far end — the toolkit clamps).
            range: (Option<f32>, Option<f32>),
        }
        let mut scrollers: Vec<Live> = Vec::new();
        for c in candidates {
            let which = if c.page { ScrollTarget::Host } else { ScrollTarget::Node };
            let live = c.port.as_ref().and_then(|f| f(which));
            let (port, offset, range) = match live {
                Some(l) => {
                    let Some(port) = l.port.or_else(|| c.frame.as_ref().and_then(|f| f())) else { continue };
                    // An axis with no range doesn't scroll: a `scroll_view`
                    // that grew to its content, or the cross axis.
                    let declared = c.axis.unwrap_or_default();
                    let x = (l.max_x > 0.0 && (c.axis.is_none() || declared.scrolls_x())).then_some(l.max_x);
                    let y = (l.max_y > 0.0 && (c.axis.is_none() || declared.scrolls_y())).then_some(l.max_y);
                    if x.is_none() && y.is_none() {
                        continue;
                    }
                    (port, (l.x, l.y), (x.or(Some(0.0)), y.or(Some(0.0))))
                }
                // A `scroll_view` on a backend that reports no scroll port:
                // its declared axes, its own box, its registered offset.
                None if c.set_scroll.is_some() => {
                    let Some(port) = c.frame.as_ref().and_then(|f| f()) else { continue };
                    let axis = c.axis.unwrap_or_default();
                    let offset = state_of(c.id).and_then(|s| s.scroll).map_or((0.0, 0.0), |(x, y)| (x as f32, y as f32));
                    let range = (
                        if axis.scrolls_x() { None } else { Some(0.0) },
                        if axis.scrolls_y() { None } else { Some(0.0) },
                    );
                    (port, offset, range)
                }
                None => continue,
            };
            scrollers.push(Live { candidate: c, port, offset, range });
        }
        let outermost = scrollers.len().saturating_sub(1);
        let mut moved = Vec::new();
        for (i, live) in scrollers.into_iter().enumerate() {
            // Inner scrollers are judged by their own box (an outer scroll
            // brings them on screen); the outermost also by the window it
            // is clipped to.
            let port = match window {
                Some(w) if i == outermost => overlap(live.port, w).unwrap_or(live.port),
                _ => live.port,
            };
            let (x0, y0) = live.offset;
            let shift = |range: Option<f32>, start, len, port_start, port_len, at: f32| -> f32 {
                if range == Some(0.0) {
                    return at;
                }
                let to = (at + reveal_shift(start, len, port_start, port_len)).max(0.0);
                // The far end is the toolkit's to clamp when the range is
                // unknown.
                range.map_or(to, |max| to.min(max))
            };
            let x1 = shift(live.range.0, target.x, target.width, port.x, port.width, x0);
            let y1 = shift(live.range.1, target.y, target.height, port.y, port.height, y0);
            if x1 == x0 && y1 == y0 {
                continue;
            }
            let c = &live.candidate;
            if let Some(set_scroll) = &c.set_scroll {
                set_scroll(x1, y1);
            } else {
                let which = if c.page { ScrollTarget::Host } else { ScrollTarget::Node };
                let Some(injection) = c.set_port.as_ref().and_then(|f| f(which, x1, y1)) else { continue };
                injection().map_err(RobotError::Native)?;
            }
            target.x -= x1 - x0;
            target.y -= y1 - y0;
            moved.push(ScrollMove { scroll_view: c.id, page: c.page, x: x1, y: y1 });
        }
        if !moved.is_empty() {
            settle();
        }
        Ok(moved)
    }

    /// The element's rect in its parent's coordinate system.
    pub fn frame(&self, element: &Element) -> Result<Option<ViewportRect>, RobotError> {
        self.geometry(element, |a| a.frame.clone(), "frame")
    }

    /// The element's rect in viewport/window coordinates.
    pub fn absolute_frame(
        &self,
        element: &Element,
    ) -> Result<Option<ViewportRect>, RobotError> {
        self.geometry(element, |a| a.absolute_frame.clone(), "absolute_frame")
    }

    /// The element's rect in physical device-screen pixels.
    pub fn device_frame(
        &self,
        element: &Element,
    ) -> Result<Option<ViewportRect>, RobotError> {
        self.geometry(element, |a| a.device_frame.clone(), "device_frame")
    }

    /// The element's platform-native render tree (parity surface). `Ok(None)`:
    /// not laid out yet. [`RobotError::ActionNotAvailable`]: this backend has no
    /// read ([`IntrospectionOps::supports_native_introspection`]).
    pub fn introspect_native(
        &self,
        element: &Element,
    ) -> Result<Option<runtime_shared::introspect::NativeNode>, RobotError> {
        let cb = REGISTRY.with(|r| {
            r.borrow()
                .get(element.id)
                .map(|e| e.actions.introspect.clone())
                .ok_or(RobotError::ElementGone)
        })?;
        match cb {
            Some(cb) => Ok(cb()),
            None => Err(RobotError::ActionNotAvailable("introspect_native")),
        }
    }

    /// Flat list of all mounted elements.
    pub fn elements(&self) -> Vec<Element> {
        self.find_all(Query::All)
    }

    /// Recursive snapshot. An entry whose parent is no longer live (a
    /// dead/recycled slot) surfaces as a ROOT rather than vanishing —
    /// the orphan rule (content mounted late inside
    /// driver effects registers with a dead or absent parent and must
    /// stay reachable). Runs entered (labels resolve — driver env).
    pub fn snapshot(&self) -> Vec<TreeNode> {
        entered(|| self.snapshot_inner())
    }

    fn snapshot_inner(&self) -> Vec<TreeNode> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            reg.roots()
                .iter()
                .filter_map(|&id| Self::build_tree_node(&reg, id))
                .collect()
        })
    }

    /// Direct children of an element. Runs entered (labels resolve).
    pub fn children_of(&self, element: &Element) -> Vec<Element> {
        entered(|| self.children_of_inner(element))
    }

    fn children_of_inner(&self, element: &Element) -> Vec<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            reg.get(element.id)
                .map(|entry| {
                    entry
                        .children
                        .iter()
                        .filter_map(|&cid| reg.get(cid).map(|e| Element::from_registry(cid, e)))
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// Parent of an element. Runs entered (labels resolve).
    pub fn parent_of(&self, element: &Element) -> Option<Element> {
        entered(|| self.parent_of_inner(element))
    }

    fn parent_of_inner(&self, element: &Element) -> Option<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            let pid = reg.get(element.id)?.parent?;
            reg.get(pid).map(|e| Element::from_registry(pid, e))
        })
    }

    /// Clear every robot registry (elements, component methods, watched
    /// signals, navigators) — test isolation; registries are
    /// thread-local, so parallel `#[test]` threads never share entries.
    /// The app's viewport now: its logical size and the breakpoint bucket
    /// that size falls in — what the app's breakpoint UI sees.
    pub fn viewport(&self) -> Viewport {
        let (size, breakpoint) = entered(crate::viewport::current_untracked);
        Viewport { width: size.width, height: size.height, breakpoint }
    }

    /// Resize the app's window so its viewport is `width × height` logical
    /// px, where the app can size its own window (the desktop backends,
    /// through [`IntrospectionOps::resize_window`]); otherwise
    /// `Err(`[`SET_VIEWPORT_UNSUPPORTED`]`)`. The new size reaches
    /// [`viewport`](Self::viewport) through the backend's own resize path
    /// — synchronously on some toolkits, on the next configure on others —
    /// so a caller that needs it waits for it.
    pub fn set_viewport(&self, width: f32, height: f32) -> Result<(), String> {
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return Err(format!("set_viewport needs a positive width and height, got {width} × {height}"));
        }
        // The first root that came from a backend: the app's main content.
        let resize = REGISTRY.with(|r| {
            let reg = r.borrow();
            reg.roots().into_iter().find_map(|id| reg.get(id)?.actions.resize_window.clone())
        });
        let injection = resize
            .and_then(|prepare| prepare(width, height))
            .ok_or_else(|| SET_VIEWPORT_UNSUPPORTED.to_string())?;
        injection().map_err(|e| format!("the platform refused the resize: {e}"))?;
        settle();
        Ok(())
    }

    pub fn reset(&self) {
        REGISTRY.with(|r| *r.borrow_mut() = Registry::new());
        PARENT_STACK.with(|s| s.borrow_mut().clear());
        crate::robot_methods::reset();
        crate::robot_watch::reset();
        NAV_REGISTRY.with(|r| *r.borrow_mut() = NavRegistry::default());
        HELD_PRESS.with(|h| *h.borrow_mut() = HeldPress::Nothing);
        HELD_LIT.with(|h| h.borrow_mut().clear());
    }

    fn build_tree_node(reg: &Registry, id: ElementId) -> Option<TreeNode> {
        let entry = reg.get(id)?;
        let children = entry
            .children
            .iter()
            .filter_map(|&cid| Self::build_tree_node(reg, cid))
            .collect();
        Some(TreeNode {
            id,
            kind: entry.kind,
            test_id: entry.test_id,
            label: entry.current_label(),
            children,
        })
    }

    fn geometry(
        &self,
        element: &Element,
        pick: impl FnOnce(&ElementActions) -> Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
        name: &'static str,
    ) -> Result<Option<ViewportRect>, RobotError> {
        let cb = REGISTRY.with(|r| {
            r.borrow()
                .get(element.id)
                .map(|e| pick(&e.actions))
                .ok_or(RobotError::ElementGone)
        })?;
        match cb {
            Some(cb) => Ok(cb()),
            None => Err(RobotError::ActionNotAvailable(name)),
        }
    }

    /// Clone the actions out, DROP the registry borrow, then invoke —
    /// an action (e.g. a click handler) may mount/unmount subtrees,
    /// which re-enters the registry; a held borrow would abort with
    /// "RefCell already borrowed".
    ///
    /// The author callback runs OUTSIDE the world (its handle-routed
    /// writes stage, exactly like a real platform event), then
    /// [`settle`] commits the staged work synchronously so a query on
    /// the next line observes the post-action tree — the driver-env
    /// contract (see `DriverEnv`).
    /// The element's own state ([`ElementState`]); `None` for a primitive
    /// with nothing to report (a view, a text, an image).
    pub fn state(&self, element: &Element) -> Result<Option<ElementState>, RobotError> {
        if REGISTRY.with(|r| r.borrow().get(element.id).is_none()) {
            return Err(RobotError::ElementGone);
        }
        Ok(state_of(element.id))
    }

    /// Whether the element can be seen, and whether it is on screen
    /// ([`Visibility`]).
    pub fn visibility(&self, element: &Element) -> Result<Visibility, RobotError> {
        visibility_of(element.id).ok_or(RobotError::ElementGone)
    }

    /// The element's resolved visual style (the `get_style` verb) and where
    /// it was read: [`StyleSource::Native`] — the toolkit's own resolution
    /// ([`IntrospectionOps::computed_style`]: the web's `getComputedStyle`,
    /// where CSS decides state, breakpoint, inheritance and preminted
    /// classes) — else [`StyleSource::Framework`]: the rules the framework
    /// resolved and last handed the backend, which on every native backend
    /// IS what it applied. `None`: no style to report (the element has no
    /// style and the toolkit offers no read).
    pub fn style(&self, element: &Element) -> Result<Option<(StyleSource, ResolvedStyle)>, RobotError> {
        let (computed, rules) = REGISTRY.with(|r| {
            let reg = r.borrow();
            let e = reg.get(element.id).ok_or(RobotError::ElementGone)?;
            Ok::<_, RobotError>((e.actions.computed_style.clone(), e.actions.style_rules.clone()))
        })?;
        if let Some(style) = computed.and_then(|f| f()) {
            return Ok(Some((StyleSource::Native, style)));
        }
        let rules = rules.and_then(|cell| cell.borrow().clone());
        Ok(rules.map(|r| (StyleSource::Framework, resolved_from_rules(&r))))
    }

    /// The part of the element's box a user could see, in window
    /// coordinates: its [`absolute_frame`](Self::absolute_frame) cut by the
    /// window and by every registered ancestor that clips its content
    /// ([`IntrospectionOps::content_clip`] — `overflow: hidden`, a scroller,
    /// a list), up to a portal: a portal's content is drawn on the window,
    /// so the ancestors of the element that opened it don't cut it. Zero-sized when nothing is left or the element is not
    /// [visible](Visibility::visible); `None` when its frame is unknown.
    pub fn visible_frame(&self, element: &Element) -> Result<Option<ViewportRect>, RobotError> {
        visibility_and_frame(element.id).map(|(_, f)| f).ok_or(RobotError::ElementGone)
    }

    fn with_actions(
        &self,
        id: ElementId,
        f: impl FnOnce(&ElementActions) -> Result<(), RobotError>,
    ) -> Result<(), RobotError> {
        let actions = REGISTRY.with(|r| {
            r.borrow()
                .get(id)
                .map(|e| e.actions.clone())
                .ok_or(RobotError::ElementGone)
        })?;
        let out = f(&actions);
        settle();
        out
    }
}

// =============================================================================
// Synthetic touch streams (`click` on touch handlers, `drag`)
// =============================================================================

use runtime_shared::{TouchEvent, TouchId, TouchPhase, TouchPoint};

/// Spacing between synthesized touch events: one 60 Hz frame, so velocity-
/// and duration-based recognizers see a plausible human gesture (a tap
/// lands well inside `TapRecognizer`'s max duration; a drag has finite
/// velocity).
pub const SYNTHETIC_FRAME_NS: u64 = 16_666_667;

/// The robot's pointer id. Platform ids are small per-finger counters, so
/// the top of the range cannot collide with a live finger.
const ROBOT_TOUCH_ID: TouchId = TouchId(u64::MAX);

/// What the hit-test tier of [`Robot::press_native`] is holding until
/// [`Robot::release_native`].
enum HeldPress {
    Nothing,
    /// A touch stream whose `Began` a handler consumed; `Ended` on release.
    Stream(SyntheticTouch),
    /// A pressable under the point: its press runs on release.
    Click(ElementId),
}

runtime_tls::thread_local! {
    /// The hit-test tier's press, held between the `press_native` and
    /// `release_native` verbs — real time (a long press) passes between them.
    static HELD_PRESS: RefCell<HeldPress> = const { RefCell::new(HeldPress::Nothing) };
    /// The state setters that press lit `pressed` on.
    static HELD_LIT: RefCell<Vec<Rc<dyn Fn(runtime_shared::StateBits, bool)>>> = const { RefCell::new(Vec::new()) };
}

/// Clear `pressed` on whatever the hit-test tier's held press lit.
fn unlight_held_press() {
    let lit = HELD_LIT.with(|h| std::mem::take(&mut *h.borrow_mut()));
    if lit.is_empty() {
        return;
    }
    for set in &lit {
        set(runtime_shared::StateBits::PRESSED, false);
    }
    settle();
}

/// One synthesized pointer stream into one handler.
struct SyntheticTouch {
    handler: runtime_shared::TouchHandler,
    /// The handler's own view origin, in window coordinates —
    /// `TouchEvent::position` is local to the view that installed it.
    origin: TouchPoint,
    timestamp_ns: u64,
    /// `Began`'s timestamp — what a hold is measured from.
    began_ns: u64,
    /// The button the stream reports (`runtime_shared::pointer_button`)
    /// before each dispatch: `Primary`, or `Secondary` for a context click.
    button: runtime_shared::PointerButton,
}

impl SyntheticTouch {
    fn new(handler: runtime_shared::TouchHandler, origin: TouchPoint) -> Self {
        Self {
            handler,
            origin,
            timestamp_ns: runtime_shared::time::now_micros().saturating_mul(1_000),
            began_ns: 0,
            button: runtime_shared::PointerButton::Primary,
        }
    }

    /// Bring the event clock up to now, for a stream held across real
    /// time ([`Robot::press_native`]'s hit-test tier): its next event is
    /// stamped when it is delivered, so the hold reads as long as it was.
    fn catch_up(&mut self) {
        let now = runtime_shared::time::now_micros().saturating_mul(1_000);
        self.timestamp_ns = self.timestamp_ns.max(now);
    }

    /// Hold still at `window` until `hold_ms` after `Began`: one stationary
    /// `Moved` stamped there (a real held finger sends none; recognizers
    /// that also read the event clock — see [`Robot::drag_with_hold`] —
    /// take it as the hold). Later events continue a frame apart.
    fn hold(&mut self, hold_ms: u64, window: TouchPoint) {
        let held_until = self.began_ns + hold_ms.saturating_mul(1_000_000);
        self.timestamp_ns = self.timestamp_ns.max(held_until);
        self.deliver(TouchPhase::Moved, window);
    }

    /// Deliver one event at `window` and settle. Returns whether the
    /// handler consumed it.
    fn deliver(&mut self, phase: TouchPhase, window: TouchPoint) -> bool {
        let event = TouchEvent {
            id: ROBOT_TOUCH_ID,
            phase,
            position: TouchPoint::new(window.x - self.origin.x, window.y - self.origin.y),
            window_position: window,
            timestamp_ns: self.timestamp_ns,
            force: None,
        };
        if phase == TouchPhase::Began {
            self.began_ns = self.timestamp_ns;
        }
        self.timestamp_ns += SYNTHETIC_FRAME_NS;
        // An unmodified press of the stream's button, as backends report
        // before each dispatch.
        runtime_shared::set_pointer_modifiers(runtime_shared::PointerModifiers::default());
        runtime_shared::set_pointer_button(self.button);
        let consumed = (self.handler)(&event).consumed;
        settle();
        consumed
    }
}

// =============================================================================
// Driver environment — the transport/harness seam (conformance wave)
// =============================================================================

/// The per-boot environment a NEW-core host installs so foreign drivers
/// (the bridge/relay verb loop, the in-process `robot-e2e` harness) can
/// use [`Robot`] without knowing the backend:
///
/// - `enter`: run a closure with the app's [`runtime_world::World`]
///   entered. Label-resolving queries ([`RegistryEntry::label_fn`]) read
///   world signals, and world signal reads need the ambient world —
///   drivers run outside it (relay `onmessage`, scheduled harness
///   steps). `World::enter` nests, so wrapping an already-entered caller
///   is harmless.
/// - `settle`: synchronously commit staged reactive work (the web
///   host's `flush_sync`). Robot actions invoke the RAW author callback
///   (pre dispatch-site flush glue), so without an explicit settle the
///   staged writes would only commit on the next flush microtask —
///   breaking the harness's synchronous `act → assert` contract.
///
/// [`Robot`] consults the installed env itself (queries run entered,
/// actions settle after dispatch), so callers stay core-agnostic.
/// Without an installed env both hooks are identity — unit tests that
/// drive the registry directly need no world.
struct DriverEnv {
    enter: Rc<dyn Fn(&mut dyn FnMut())>,
    settle: Rc<dyn Fn()>,
}

runtime_tls::thread_local! {
    static DRIVER_ENV: RefCell<Option<DriverEnv>> = const { RefCell::new(None) };
}

/// Install the driver environment (see [`DriverEnv`] docs above).
/// Called once by the host boot (e.g.
/// `backend_web::newcore::start`). Re-installing replaces the prior env.
pub fn install_driver_env(
    enter: impl Fn(&mut dyn FnMut()) + 'static,
    settle: impl Fn() + 'static,
) {
    DRIVER_ENV.with(|slot| {
        *slot.borrow_mut() = Some(DriverEnv {
            enter: Rc::new(enter),
            settle: Rc::new(settle),
        })
    });
}

/// Remove the installed driver environment (host teardown / tests).
pub fn clear_driver_env() {
    DRIVER_ENV.with(|slot| *slot.borrow_mut() = None);
}

/// Run `f` with the app's world entered when an env is installed
/// (identity otherwise). The env `Rc` is cloned out before the call so
/// `f` may itself touch the env slot. `World::enter` nests, so wrapping
/// an already-entered caller is harmless (crate-internal read paths —
/// watch reads, nav snapshots — self-wrap through this).
pub(crate) fn entered<R>(f: impl FnOnce() -> R) -> R {
    let enter = DRIVER_ENV.with(|slot| slot.borrow().as_ref().map(|e| e.enter.clone()));
    match enter {
        Some(enter) => {
            let mut f = Some(f);
            let mut out: Option<R> = None;
            enter(&mut || {
                if let Some(f) = f.take() {
                    out = Some(f());
                }
            });
            out.expect("robot driver env `enter` must invoke the closure it is given")
        }
        None => f(),
    }
}

/// Synchronously commit staged reactive work via the installed env
/// (no-op without one). Robot actions call this after dispatching the
/// author callback; harnesses may call it after writing signals
/// directly.
pub fn settle() {
    let settle = DRIVER_ENV.with(|slot| slot.borrow().as_ref().map(|e| e.settle.clone()));
    if let Some(settle) = settle {
        settle();
    }
}

// =============================================================================
// Bridge verb dispatch — the transport half, against THIS registry (same
// verbs and response JSON as `runtime_shared::robot::bridge::dispatch`)
// =============================================================================

/// JSON verb dispatch for bridge/relay transports. Mirrors the element
/// surface of `runtime_shared::robot::bridge::dispatch` verb-for-verb and
/// byte-for-byte on the wire (same `cmd` names, same argument shapes,
/// same response JSON), so the MCP server / relay / `robot-test` client
/// drive an app unchanged.
///
/// Also served here: `list_components` / `invoke_method` (the
/// `#[method]` registry), `list_watched_signals` / `read_signal` (the
/// watch registry), `list_navigators` / `get_navigator_state` (the
/// nav registry), `list_functions` / `call_function` / `poll_function`
/// (`robot_fn`), `open_link` (the inbound-link ingress) and
/// `get_viewport` / `set_viewport` ([`Robot::viewport`] /
/// [`Robot::set_viewport`]), `scroll_into_view`
/// ([`Robot::scroll_into_view`]) and `get_platform` ([`platform_id`]).
/// Verbs the
/// registry doesn't own at all (`get_logs`,
/// `screenshot`, custom commands) return `unknown command: …` — the
/// transport falls back to its platform path for those (the web relay
/// client routes them to `runtime_shared::robot::bridge::dispatch`,
/// whose log/custom machinery is registry-independent).
pub mod bridge {
    use super::*;
    use runtime_shared::__serde_json as serde_json;

    /// `open_link`'s bound on in-call re-routing: one round per nested
    /// navigator level that mounts synchronously after its parent's
    /// commit. Each round must move the shown chain or the loop stops, so
    /// the bound only guards against a navigator tree that keeps moving
    /// without converging.
    const OPEN_LINK_SYNC_ROUNDS: usize = 16;

    /// Invoke one bridge verb against the vocabulary registry. Must run on
    /// the UI thread (registry + driver env are thread-local). Queries
    /// run inside the installed driver env's `enter`; actions settle.
    pub fn invoke_command(cmd: &str, args: &serde_json::Value) -> Result<String, String> {
        let robot = Robot::new();
        match cmd {
            "ping" => Ok("\"pong\"".into()),
            "find_element" => {
                let query = parse_query(args)?;
                match robot.find(query) {
                    Some(el) => Ok(element_json(&el)),
                    None => Ok("null".into()),
                }
            }
            "find_all_elements" => {
                let query = parse_query(args)?;
                let els: Vec<String> = robot.find_all(query).iter().map(element_json).collect();
                Ok(format!("[{}]", els.join(",")))
            }
            "click" => {
                let el = resolve_element(args)?;
                robot.click(&el).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "tap_native" => {
                let el = resolve_element(args)?;
                let how = robot.tap_native(&el).map_err(|e| e.to_string())?;
                Ok(format!("{{\"mechanism\":\"{}\"}}", how.as_str()))
            }
            "press_native" => {
                let el = resolve_element(args)?;
                let how = robot.press_native(&el).map_err(|e| e.to_string())?;
                Ok(format!("{{\"mechanism\":\"{}\"}}", how.as_str()))
            }
            "release_native" => {
                let el = resolve_element(args)?;
                let how = robot.release_native(&el).map_err(|e| e.to_string())?;
                Ok(format!("{{\"mechanism\":\"{}\"}}", how.as_str()))
            }
            "context_click" => {
                let el = resolve_element(args)?;
                let how = robot.context_click_native(&el).map_err(|e| e.to_string())?;
                Ok(format!("{{\"mechanism\":\"{}\"}}", how.as_str()))
            }
            "hover_native" => {
                let el = resolve_element(args)?;
                robot.hover_native(&el).map_err(|e| e.to_string())?;
                Ok("{\"mechanism\":\"native\"}".into())
            }
            "drag_native" => {
                let el = resolve_element(args)?;
                let points = parse_points(args)?;
                robot.drag_native(&el, &points).map_err(|e| e.to_string())?;
                Ok("{\"mechanism\":\"native\"}".into())
            }
            "key" => {
                let chord = key::chord_from_args(args)?;
                // An element is optional: with one, focus it first.
                let el = match args["element_id"] {
                    serde_json::Value::Null => None,
                    _ => Some(resolve_element(args)?),
                };
                let how = robot.press_key(el.as_ref(), &chord).map_err(|e| e.to_string())?;
                Ok(format!("{{\"mechanism\":\"{}\"}}", how.as_str()))
            }
            "drag" => {
                let el = resolve_element(args)?;
                let points = parse_points(args)?;
                let hold_ms = match &args["hold_ms"] {
                    serde_json::Value::Null => 0,
                    v => v.as_u64().ok_or(format!("'hold_ms' must be a whole number of ms, got {v}"))?,
                };
                robot
                    .drag_with_hold(&el, &points, hold_ms)
                    .map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "type_text" => {
                let el = resolve_element(args)?;
                let text = args["text"].as_str().ok_or("missing 'text' argument")?;
                robot.type_text(&el, text).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "set_toggle" => {
                let el = resolve_element(args)?;
                let value = args["value"].as_bool().ok_or("missing 'value' argument")?;
                robot.set_toggle(&el, value).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "set_slider" => {
                let el = resolve_element(args)?;
                let value = args["value"].as_f64().ok_or("missing 'value' argument")? as f32;
                robot.set_slider(&el, value).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "set_scroll" => {
                let el = resolve_element(args)?;
                let x = args["x"].as_f64().unwrap_or(0.0) as f32;
                let y = args["y"].as_f64().unwrap_or(0.0) as f32;
                robot.set_scroll(&el, x, y).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "scroll_into_view" => {
                let el = resolve_element(args)?;
                let moved = robot.scroll_into_view(&el).map_err(|e| e.to_string())?;
                let scrolled: Vec<serde_json::Value> = moved
                    .iter()
                    .map(|m| {
                        let mut v = serde_json::json!({ "element_id": m.scroll_view.0, "x": m.x, "y": m.y });
                        if m.page {
                            v["page"] = serde_json::Value::Bool(true);
                        }
                        v
                    })
                    .collect();
                Ok(serde_json::json!({ "scrolled": scrolled }).to_string())
            }
            "get_platform" => Ok(format!("{{\"platform\":{}}}", serde_json::to_string(platform_id()).unwrap())),
            "focus" => {
                let el = resolve_element(args)?;
                robot.focus(&el).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "get_style" => {
                let el = resolve_element(args)?;
                Ok(match robot.style(&el).map_err(|e| e.to_string())? {
                    Some((source, style)) => style_json(source, &style),
                    None => "{\"source\":null,\"style\":{}}".into(),
                })
            }
            "get_state" => {
                let el = resolve_element(args)?;
                Ok(match robot.state(&el).map_err(|e| e.to_string())? {
                    Some(state) => state.to_json(),
                    None => "{}".into(),
                })
            }
            "blur" => {
                let el = resolve_element(args)?;
                robot.blur(&el).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "get_snapshot" => {
                let tree = robot.snapshot();
                let index = crate::robot_methods::element_component_index();
                let nodes: Vec<String> = tree.iter().map(|n| tree_node_json(n, &index)).collect();
                Ok(format!("[{}]", nodes.join(",")))
            }
            "get_children" => {
                let el = resolve_element(args)?;
                let children: Vec<String> =
                    robot.children_of(&el).iter().map(element_json).collect();
                Ok(format!("[{}]", children.join(",")))
            }
            "get_parent" => {
                let el = resolve_element(args)?;
                match robot.parent_of(&el) {
                    Some(p) => Ok(element_json(&p)),
                    None => Ok("null".into()),
                }
            }
            "get_viewport" => {
                let v = robot.viewport();
                Ok(format!(
                    "{{\"width\":{},\"height\":{},\"breakpoint\":\"{}\"}}",
                    v.width,
                    v.height,
                    breakpoint_name(v.breakpoint),
                ))
            }
            "set_viewport" => {
                let dim = |key: &str| {
                    args[key].as_f64().map(|v| v as f32).ok_or_else(|| format!("missing '{key}' argument"))
                };
                robot.set_viewport(dim("width")?, dim("height")?)?;
                Ok("\"ok\"".into())
            }
            "count_elements" => {
                // Same criteria as `find_all_elements`, so a count always
                // agrees with the list it counts (and an unknown kind errors
                // instead of counting everything).
                let query = parse_query(args)?;
                Ok(robot.find_all(query).len().to_string())
            }
            "get_frame" => {
                let el = resolve_element(args)?;
                rect_json(robot.frame(&el).map_err(|e| e.to_string())?)
            }
            "get_absolute_frame" => {
                let el = resolve_element(args)?;
                rect_json(robot.absolute_frame(&el).map_err(|e| e.to_string())?)
            }
            "highlight_element" => {
                let el = resolve_element(args)?;
                crate::robot_highlight::highlight(el.id)
            }
            "clear_highlight" => {
                crate::robot_highlight::clear();
                Ok("\"ok\"".into())
            }
            "get_device_frame" => {
                let el = resolve_element(args)?;
                rect_json(robot.device_frame(&el).map_err(|e| e.to_string())?)
            }
            "introspect_native" => {
                let el = resolve_element(args)?;
                // A backend with no read answers `null`, as an element not laid
                // out yet does: both mean "no data" to a parity capture.
                match robot.introspect_native(&el) {
                    Ok(Some(node)) => serde_json::to_string(&node)
                        .map_err(|e| format!("failed to serialize native tree: {e}")),
                    Ok(None) | Err(RobotError::ActionNotAvailable(_)) => Ok("null".into()),
                    Err(e) => Err(e.to_string()),
                }
            }
            "introspect_subtree" => {
                // BATCHED `introspect_native`: one call returns the native node
                // for `element_id` and every descendant, keyed by element id.
                //
                // The per-element verb costs a full bridge round trip each, and
                // the round trip is bounded below by the bridge's 16 ms poll —
                // so a cross-platform parity capture of a 400-element screen
                // spent ~800 round trips and over ten seconds PER PLATFORM,
                // which put a 49-route sweep past ten minutes. Reading the whole
                // subtree in one call collapses that to one round trip per
                // screen per platform.
                //
                // Elements the backend cannot introspect are omitted rather than
                // emitted as `null`: absence already means "no data" to every
                // caller, and skipping them keeps the payload to what is
                // actually comparable.
                let el = resolve_element(args)?;
                let mut out: Vec<String> = Vec::new();
                let mut stack = vec![el];
                while let Some(current) = stack.pop() {
                    for child in robot.children_of(&current) {
                        stack.push(child);
                    }
                    match robot.introspect_native(&current) {
                        Ok(Some(node)) => {
                            let json = serde_json::to_string(&node).map_err(|e| {
                                format!("failed to serialize native tree: {e}")
                            })?;
                            out.push(format!("\"{}\":{}", current.id.0, json));
                        }
                        Ok(None) | Err(RobotError::ActionNotAvailable(_)) => continue,
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Ok(format!("{{{}}}", out.join(",")))
            }
            "list_components" => {
                // Same JSON shape as runtime-shared's `list_components`
                // (instance_id / name / element_id / methods[{name,args}]).
                let snaps = crate::robot_methods::list_components();
                let entries: Vec<String> = snaps
                    .iter()
                    .map(|s| {
                        let methods: Vec<String> = s
                            .methods
                            .iter()
                            .map(|(name, args)| {
                                let args_json: Vec<String> = args
                                    .iter()
                                    .map(|(arg_name, arg_type)| {
                                        format!(
                                            "{{\"name\":{},\"type\":{}}}",
                                            serde_json::to_string(arg_name).unwrap(),
                                            serde_json::to_string(arg_type).unwrap(),
                                        )
                                    })
                                    .collect();
                                format!(
                                    "{{\"name\":{},\"args\":[{}]}}",
                                    serde_json::to_string(name).unwrap(),
                                    args_json.join(",")
                                )
                            })
                            .collect();
                        let element_id = match s.element_id {
                            Some(eid) => eid.0.to_string(),
                            None => "null".to_string(),
                        };
                        format!(
                            "{{\"instance_id\":{},\"name\":{},\"file\":{},\"line\":{},\"element_id\":{},\"methods\":[{}]}}",
                            s.id.0,
                            serde_json::to_string(s.name).unwrap(),
                            serde_json::to_string(s.file).unwrap(),
                            s.line,
                            element_id,
                            methods.join(",")
                        )
                    })
                    .collect();
                Ok(format!("[{}]", entries.join(",")))
            }
            "get_component" => {
                let instance = crate::robot_methods::ComponentInstanceId(
                    args["instance_id"].as_u64().ok_or("missing 'instance_id' argument")? as u32,
                );
                let Some(snap) = crate::robot_methods::list_components()
                    .into_iter()
                    .find(|c| c.id == instance)
                else {
                    return Ok("null".into());
                };
                // A `Live` prop's reader is author code reading signals:
                // run it entered, like a label query.
                let props = entered(|| crate::robot_methods::component_props(instance)).unwrap_or_default();
                Ok(serde_json::json!({
                    "instance_id": snap.id.0,
                    "name": snap.name,
                    "file": snap.file,
                    "line": snap.line,
                    "element_id": snap.element_id.map(|e| e.0),
                    "methods": snap.methods.iter().map(|(name, args)| serde_json::json!({
                        "name": name,
                        "args": args.iter().map(|(n, t)| serde_json::json!({ "name": n, "type": t })).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                    "props": props.iter().map(|p| serde_json::json!({
                        "name": p.name,
                        "type": p.ty,
                        "mode": p.mode.as_str(),
                        "value": p.value,
                    })).collect::<Vec<_>>(),
                })
                .to_string())
            }
            "invoke_method" => {
                let instance_id = args["instance_id"]
                    .as_u64()
                    .ok_or("missing 'instance_id' argument")? as u32;
                let method = args["method"]
                    .as_str()
                    .ok_or("missing 'method' argument")?;
                let method_args = args
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                // Author code: runs OUTSIDE the world, stages writes, and
                // settles (inside `invoke_method` itself — action contract).
                crate::robot_methods::invoke_method(
                    ComponentInstanceId(instance_id),
                    method,
                    &method_args,
                )?;
                Ok("\"ok\"".into())
            }
            "list_watched_signals" => {
                // Reads run entered inside `list_watched` (driver env).
                let items: Vec<String> = crate::robot_watch::list_watched()
                    .iter()
                    .map(|s| {
                        format!(
                            "{{\"id\":{},\"name\":{},\"value\":{},\"writes\":{},\"changed_ago_ms\":{},\"writable\":{}}}",
                            s.id,
                            serde_json::to_string(&s.name).unwrap_or_else(|_| "\"\"".into()),
                            // `Value`'s Display emits valid compact JSON.
                            s.value,
                            s.writes,
                            s.changed_ago_ms.map(|ms| ms.to_string()).unwrap_or_else(|| "null".into()),
                            s.writable,
                        )
                    })
                    .collect();
                Ok(format!("[{}]", items.join(",")))
            }
            "get_signal_history" => {
                let id = crate::robot_watch::resolve_watched(
                    args["id"].as_u64().map(|i| i as u32),
                    args["name"].as_str(),
                )
                .ok_or("get_signal_history requires the 'id' or 'name' of a watched signal")?;
                match crate::robot_watch::watched_history(id) {
                    Some((name, writes, points)) => Ok(serde_json::json!({
                        "id": id,
                        "name": name,
                        "writes": writes,
                        "history": points.iter().map(|p| serde_json::json!({
                            "ago_ms": p.ago_ms,
                            "value": p.value,
                        })).collect::<Vec<_>>(),
                    })
                    .to_string()),
                    None => Ok("null".into()),
                }
            }
            "write_signal" => {
                let id = crate::robot_watch::resolve_watched(
                    args["id"].as_u64().map(|i| i as u32),
                    args["name"].as_str(),
                )
                .ok_or("write_signal requires the 'id' or 'name' of a watched signal")?;
                let value = args.get("value").ok_or("write_signal requires a 'value' argument")?;
                crate::robot_watch::write_watched(id, value)?;
                Ok("true".into())
            }
            "read_signal" => {
                let value = if let Some(name) = args["name"].as_str() {
                    crate::robot_watch::read_watched_by_name(name)
                } else if let Some(id) = args["id"].as_u64() {
                    crate::robot_watch::read_watched_by_id(id as u32)
                } else {
                    return Err("read_signal requires a 'name' or 'id' argument".into());
                };
                Ok(value.map(|v| v.to_string()).unwrap_or_else(|| "null".into()))
            }
            "list_navigators" => {
                // Snapshot closures read nav signals — run entered.
                let items: Vec<String> = entered(all_navigators)
                    .iter()
                    .map(nav_snapshot_json)
                    .collect();
                Ok(format!("[{}]", items.join(",")))
            }
            "navigate" => {
                let nav_id = args["nav_id"].as_u64().ok_or("missing 'nav_id' argument")? as u32;
                let path = || {
                    args["path"]
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "navigate requires a 'path' for push/replace/reset".to_string())
                };
                let action = match args["action"].as_str().ok_or("missing 'action' argument")? {
                    "push" => NavAction::Push(path()?),
                    "replace" => NavAction::Replace(path()?),
                    "reset" => NavAction::Reset(path()?),
                    "pop" => NavAction::Pop,
                    other => return Err(format!("unknown navigate action '{other}' (push|replace|reset|pop)")),
                };
                navigate(NavId(nav_id), action)?;
                Ok("true".into())
            }
            "get_navigator_state" => {
                let nav_id =
                    args["nav_id"].as_u64().ok_or("missing 'nav_id' argument")? as u32;
                match entered(|| navigator_snapshot(NavId(nav_id))) {
                    Some(s) => Ok(nav_snapshot_json(&s)),
                    None => Ok("null".into()),
                }
            }
            "open_link" => {
                let url = args["url"].as_str().ok_or("missing 'url' argument")?;
                let target = runtime_shared::inbound_link::route_path(url);
                // What a host does when the OS hands the running app a
                // link: observers, interceptors, then the navigators — and
                // the flush it owes (`settle`). `resume` is a client's
                // follow-up for the SAME link: observers and interceptors
                // already had it, so only the router runs again.
                let routed = if args["resume"].as_bool().unwrap_or(false) {
                    runtime_shared::inbound_link::route(url)
                } else {
                    runtime_shared::inbound_link::deliver(url)
                };
                settle();
                // The router moves only navigators that exist. One the
                // commit mounted outside the link's scope (a screen that
                // builds its nested navigator in a later reactive pass)
                // is live now, on its configured initial: keep routing
                // while that moves the shown chain closer. A navigator
                // behind an async load (a lazy chunk, a data fetch) is
                // not up until the event loop runs — the answer says how
                // far the link got, and the client resumes it.
                let mut progress = entered(|| crate::handlers::nav_link_progress(&target));
                if routed {
                    for _ in 0..OPEN_LINK_SYNC_ROUNDS {
                        if progress.1 || !runtime_shared::inbound_link::route(url) {
                            break;
                        }
                        settle();
                        let next = entered(|| crate::handlers::nav_link_progress(&target));
                        let moved = next != progress;
                        progress = next;
                        if !moved {
                            break;
                        }
                    }
                }
                let (reached, complete) = progress;
                Ok(serde_json::json!({ "reached": reached, "complete": complete, "routed": routed })
                    .to_string())
            }
            "list_functions" => Ok(crate::robot_fn::list_json()),
            "call_function" => {
                let name = args["name"].as_str().ok_or("missing 'name' argument")?;
                let fn_args = args
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                // Author code: runs outside the world and settles, like
                // `invoke_method`.
                crate::robot_fn::call_json(name, &fn_args)
            }
            "poll_function" => {
                let call = args["call"].as_u64().ok_or("missing 'call' argument")?;
                crate::robot_fn::poll_json(call)
            }
            _ => Err(format!("unknown command: {cmd}")),
        }
    }

    /// `points`: an array of `[x, y]` pairs.
    fn parse_points(args: &serde_json::Value) -> Result<Vec<(f32, f32)>, String> {
        args["points"]
            .as_array()
            .ok_or("missing 'points' argument (an array of [x, y] pairs)")?
            .iter()
            .map(|p| match (p[0].as_f64(), p[1].as_f64()) {
                (Some(x), Some(y)) => Ok((x as f32, y as f32)),
                _ => Err(format!("each point must be [x, y], got {p}")),
            })
            .collect()
    }

    /// runtime-shared's bridge `nav_snapshot_json` plus `controllable`,
    /// `query` and `href`.
    ///
    /// `query` is the active screen's query as a JSON object of decoded
    /// values (a repeated key reports its first value, as
    /// `QueryParams::get` does); `href` is `active_path` with the encoded
    /// query appended — the URL web's address bar shows for this
    /// navigator's screen. `active_path` itself stays path-only.
    fn nav_snapshot_json(s: &NavSnapshot) -> String {
        let stack: Vec<String> = s
            .stack
            .iter()
            .map(|(route, path)| {
                format!(
                    "{{\"route\":{},\"path\":{}}}",
                    serde_json::to_string(route).unwrap_or_else(|_| "\"\"".into()),
                    serde_json::to_string(path).unwrap_or_else(|_| "\"\"".into()),
                )
            })
            .collect();
        let element_id = match s.element_id {
            Some(id) => id.to_string(),
            None => "null".into(),
        };
        let mut query = serde_json::Map::new();
        for (k, v) in s.query.iter() {
            query.entry(k.to_string()).or_insert_with(|| serde_json::Value::String(v.to_string()));
        }
        let href = runtime_shared::primitives::navigator::with_query(&s.active_path, &s.query);
        format!(
            "{{\"nav_id\":{},\"element_id\":{},\"type_name\":{},\"active_route\":{},\
               \"active_path\":{},\"query\":{},\"href\":{},\"depth\":{},\"can_go_back\":{},\
               \"is_current\":{},\"base\":{},\"stack\":[{}],\"controllable\":{}}}",
            s.nav_id,
            element_id,
            serde_json::to_string(s.type_name).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(&s.active_route).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(&s.active_path).unwrap_or_else(|_| "\"\"".into()),
            serde_json::Value::Object(query),
            serde_json::to_string(&href).unwrap_or_else(|_| "\"\"".into()),
            s.depth,
            s.can_go_back,
            s.is_current,
            serde_json::to_string(&s.base).unwrap_or_else(|_| "\"\"".into()),
            stack.join(","),
            s.controllable,
        )
    }

    /// The query object `find_element` / `find_all_elements` /
    /// `count_elements` take. Every field given must match (AND):
    ///
    /// - `test_id`: exact test id;
    /// - `kind`: the `ElementKind` name (`"TextInput"`);
    /// - `label`: exact label;
    /// - `label_contains`: a substring of the label, or an array of
    ///   substrings that must all appear;
    /// - `within`: scope to the descendants of a container — an element
    ///   id (a number), or a nested query object whose own optional `nth`
    ///   picks one of the containers it matches (document order).
    ///
    /// No field matches every element. Kind and test id are checked before
    /// the label, so the label is only read for candidates still in.
    fn parse_query(args: &serde_json::Value) -> Result<Query, String> {
        let mut parts = Vec::new();
        if let Some(kind_str) = args["kind"].as_str() {
            match parse_element_kind(kind_str) {
                Some(k) => parts.push(Query::Kind(k)),
                None => return Err(format!("unknown element kind: {kind_str}")),
            }
        }
        if let Some(id) = args["test_id"].as_str() {
            parts.push(Query::TestId(id.to_string()));
        }
        if let Some(label) = args["label"].as_str() {
            parts.push(Query::Label(label.to_string()));
        }
        match &args["label_contains"] {
            serde_json::Value::Null => {}
            serde_json::Value::String(sub) => parts.push(Query::LabelContains(sub.clone())),
            serde_json::Value::Array(subs) => {
                for sub in subs {
                    let sub = sub.as_str().ok_or("'label_contains' must be a string or an array of strings")?;
                    parts.push(Query::LabelContains(sub.to_string()));
                }
            }
            _ => return Err("'label_contains' must be a string or an array of strings".into()),
        }
        let query = match parts.len() {
            0 => Query::All,
            1 => parts.pop().expect("one part"),
            _ => Query::And(parts),
        };
        match &args["within"] {
            serde_json::Value::Null => Ok(query),
            serde_json::Value::Number(n) => {
                let id = n.as_u64().ok_or("'within' must be an element id or a query object")?;
                Ok(query.within(Query::Id(ElementId(id as u32))))
            }
            container @ serde_json::Value::Object(_) => Ok(Query::Within {
                container: Box::new(parse_query(container)?),
                nth: container["nth"].as_u64().map(|n| n as usize),
                query: Box::new(query),
            }),
            _ => Err("'within' must be an element id or a query object".into()),
        }
    }

    /// The lower-case name `get_viewport` reports (`"xs"` … `"xl"`), the
    /// same spelling as a stylesheet's `breakpoint md { … }`.
    fn breakpoint_name(bp: runtime_shared::Breakpoint) -> &'static str {
        use runtime_shared::Breakpoint;
        match bp {
            Breakpoint::Xs => "xs",
            Breakpoint::Sm => "sm",
            Breakpoint::Md => "md",
            Breakpoint::Lg => "lg",
            Breakpoint::Xl => "xl",
        }
    }

    fn parse_element_kind(s: &str) -> Option<ElementKind> {
        match s {
            "View" => Some(ElementKind::View),
            "Text" => Some(ElementKind::Text),
            "Button" => Some(ElementKind::Button),
            "Pressable" => Some(ElementKind::Pressable),
            "Image" => Some(ElementKind::Image),
            "Icon" => Some(ElementKind::Icon),
            "TextInput" => Some(ElementKind::TextInput),
            "Toggle" => Some(ElementKind::Toggle),
            "ScrollView" => Some(ElementKind::ScrollView),
            "Slider" => Some(ElementKind::Slider),
            "ActivityIndicator" => Some(ElementKind::ActivityIndicator),
            "Virtualizer" => Some(ElementKind::Virtualizer),
            "Graphics" => Some(ElementKind::Graphics),
            "Navigator" => Some(ElementKind::Navigator),
            "Link" => Some(ElementKind::Link),
            "Overlay" => Some(ElementKind::Overlay),
            "Presence" => Some(ElementKind::Presence),
            _ => None,
        }
    }

    fn resolve_element(args: &serde_json::Value) -> Result<Element, String> {
        let id = args["element_id"]
            .as_u64()
            .ok_or("missing 'element_id' argument")?;
        Ok(Element {
            id: ElementId(id as u32),
            kind: ElementKind::View,
            test_id: None,
            label: None,
        })
    }

    fn rect_json(rect: Option<ViewportRect>) -> Result<String, String> {
        Ok(match rect {
            Some(r) => format!(
                "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}}",
                r.x, r.y, r.width, r.height
            ),
            None => "null".into(),
        })
    }

    /// `,"visible":…[,"in_viewport":…][,"visible_frame":{…}][,"state":{…}]`
    /// — visibility for every element (`visible_frame` when its frame is
    /// known), `state` for one that has any.
    fn state_field(id: ElementId) -> String {
        let mut out = String::new();
        if let Some((v, frame)) = visibility_and_frame(id) {
            out.push_str(&format!(",\"visible\":{}", v.visible));
            if let Some(on) = v.in_viewport {
                out.push_str(&format!(",\"in_viewport\":{on}"));
            }
            if frame.is_some() {
                out.push_str(&format!(",\"visible_frame\":{}", rect_json(frame).unwrap_or_default()));
            }
        }
        if let Some(state) = state_of(id) {
            out.push_str(&format!(",\"state\":{}", state.to_json()));
        }
        out
    }

    fn element_json(el: &Element) -> String {
        format!(
            "{{\"id\":{},\"kind\":\"{:?}\",\"test_id\":{},\"label\":{}{}}}",
            el.id.0,
            el.kind,
            el.test_id
                .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "null".into()))
                .unwrap_or_else(|| "null".into()),
            el.label
                .as_deref()
                .map(|l| serde_json::to_string(l).unwrap_or_else(|_| "null".into()))
                .unwrap_or_else(|| "null".into()),
            state_field(el.id),
        )
    }

    /// One tree node. `components` lists the component instances this
    /// element is the root of, outermost first — omitted for an element
    /// no component links to, so a component-free tree keeps the plain
    /// shape.
    fn tree_node_json(
        node: &TreeNode,
        index: &std::collections::HashMap<u32, Vec<(crate::robot_methods::ComponentInstanceId, &'static str)>>,
    ) -> String {
        let children: Vec<String> =
            node.children.iter().map(|c| tree_node_json(c, index)).collect();
        let components = match index.get(&node.id.0) {
            Some(linked) if !linked.is_empty() => {
                let items: Vec<String> = linked
                    .iter()
                    .map(|(id, name)| {
                        format!(
                            "{{\"instance_id\":{},\"name\":{}}}",
                            id.0,
                            serde_json::to_string(name).unwrap()
                        )
                    })
                    .collect();
                format!(",\"components\":[{}]", items.join(","))
            }
            _ => String::new(),
        };
        format!(
            "{{\"id\":{},\"kind\":\"{:?}\",\"test_id\":{},\"label\":{}{}{},\"children\":[{}]}}",
            node.id.0,
            node.kind,
            node.test_id
                .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "null".into()))
                .unwrap_or_else(|| "null".into()),
            node.label
                .as_deref()
                .map(|l| serde_json::to_string(l).unwrap_or_else(|_| "null".into()))
                .unwrap_or_else(|| "null".into()),
            state_field(node.id),
            components,
            children.join(","),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::__serde_json::{self as serde_json, json};

    fn entry(test_id: &'static str) -> RegistryEntry {
        RegistryEntry {
            kind: ElementKind::Text,
            test_id: Some(test_id),
            label: None,
            label_fn: None,
            actions: ElementActions::default(),
            parent: None,
            children: Vec::new(),
            order: (0, 0),
            detached: false,
        }
    }

    /// Duplicate policy pin: `find` is last-wins via
    /// the index; `find_all` returns every holder.
    #[test]
    fn duplicate_test_id_last_wins_for_find_all_for_find_all() {
        let robot = Robot::new();
        robot.reset();
        let first = REGISTRY.with(|r| r.borrow_mut().insert(entry("dup")));
        let second = REGISTRY.with(|r| r.borrow_mut().insert(entry("dup")));
        assert_eq!(robot.find(Query::test_id("dup")).unwrap().id, second);
        assert_eq!(robot.find_all(Query::test_id("dup")).len(), 2);
        // Removing the OLDER duplicate must not orphan the index entry
        // the newer one owns.
        REGISTRY.with(|r| r.borrow_mut().remove(first));
        assert_eq!(robot.find(Query::test_id("dup")).unwrap().id, second);
        robot.reset();
    }

    /// The orphan rule: a dead parent surfaces its child as a root.
    #[test]
    fn snapshot_surfaces_orphan_with_dead_parent() {
        let robot = Robot::new();
        robot.reset();
        let parent = REGISTRY.with(|r| r.borrow_mut().insert(entry("p")));
        let child = REGISTRY.with(|r| {
            r.borrow_mut().insert(RegistryEntry {
                parent: Some(parent),
                ..entry("orphan-child")
            })
        });
        // Kill just the parent slot; the child keeps its stale parent id.
        REGISTRY.with(|r| {
            r.borrow_mut().entries[parent.0 as usize] = None;
        });
        let tree = robot.snapshot();
        assert!(
            tree.iter().any(|n| n.id == child && n.test_id == Some("orphan-child")),
            "orphan must surface as a root: {tree:?}"
        );
        robot.reset();
    }

    /// Bridge-dispatch round trip: the relay verb loop's exact path —
    /// `find_element` (by test_id) → `click` (by the returned
    /// element_id) → the author callback fired. Wire shapes mirror the
    /// old `runtime_shared::robot::bridge` responses.
    #[test]
    fn bridge_invoke_command_find_then_click() {
        use runtime_shared::__serde_json::json;
        use std::cell::Cell;

        let robot = Robot::new();
        robot.reset();
        let clicks: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let clicks_in = clicks.clone();
        let id = REGISTRY.with(|r| {
            r.borrow_mut().insert(RegistryEntry {
                kind: ElementKind::Button,
                test_id: Some("inc"),
                label: Some("Increment".into()),
                actions: ElementActions {
                    click: Some(Rc::new(move || clicks_in.set(clicks_in.get() + 1))),
                    ..ElementActions::default()
                },
                ..RegistryEntry::default()
            })
        });

        let found = bridge::invoke_command("find_element", &json!({"test_id": "inc"}))
            .expect("find_element");
        assert_eq!(
            found,
            format!(
                // `visible`: no toolkit answer and no frame — mounted is
                // all that's known (`Visibility::from_parts`).
                "{{\"id\":{},\"kind\":\"Button\",\"test_id\":\"inc\",\"label\":\"Increment\",\"visible\":true}}",
                id.0
            )
        );

        let ok = bridge::invoke_command("click", &json!({"element_id": id.0}))
            .expect("click dispatches");
        assert_eq!(ok, "\"ok\"");
        assert_eq!(clicks.get(), 1, "author callback fired through the verb loop");

        // Unknown verbs surface as the fallback marker the transport
        // keys on; owned verbs with bad args fail with the argument
        // name (never the fallback marker — the transport must not
        // route them to its platform fallback).
        let unknown = bridge::invoke_command("get_logs", &json!({})).unwrap_err();
        assert!(unknown.starts_with("unknown command:"), "{unknown}");
        let bad_args = bridge::invoke_command("invoke_method", &json!({})).unwrap_err();
        assert!(bad_args.contains("instance_id"), "{bad_args}");
        robot.reset();
    }

    /// `list_components` + `invoke_method` over the bridge — the exact
    /// verb path the MCP server / Inspector use.
    #[test]
    fn bridge_method_verbs_round_trip() {
        use runtime_shared::__serde_json::json;
        use std::cell::Cell;

        let robot = Robot::new();
        robot.reset();
        let hits: Rc<Cell<i32>> = Rc::new(Cell::new(0));
        let hits_in = hits.clone();
        let reg = crate::robot_methods::register_component(
            "MethodCounter",
            vec![crate::robot_methods::Method {
                name: "bump_by",
                args: &[("n", "i32")],
                invoke: Rc::new(move |args| {
                    let n = args["n"].as_i64().ok_or("arg 'n': missing")? as i32;
                    hits_in.set(hits_in.get() + n);
                    Ok(())
                }),
            }],
        );
        crate::robot_methods::link_component_element(reg.id(), 7);

        let list = bridge::invoke_command("list_components", &json!({})).expect("list");
        let expected = format!(
            "[{{\"instance_id\":{},\"name\":\"MethodCounter\",\"file\":\"\",\"line\":0,\"element_id\":7,\
             \"methods\":[{{\"name\":\"bump_by\",\"args\":[{{\"name\":\"n\",\"type\":\"i32\"}}]}}]}}]",
            reg.id().0
        );
        assert_eq!(list, expected, "old-bridge wire shape plus the additive file/line");

        let ok = bridge::invoke_command(
            "invoke_method",
            &json!({ "instance_id": reg.id().0, "method": "bump_by", "args": { "n": 4 } }),
        )
        .expect("invoke");
        assert_eq!(ok, "\"ok\"");
        assert_eq!(hits.get(), 4, "method ran through the verb loop");

        drop(reg);
        assert!(
            bridge::invoke_command(
                "invoke_method",
                &json!({ "instance_id": 999, "method": "bump_by", "args": {} }),
            )
            .is_err(),
            "unknown instance errors"
        );
        robot.reset();
    }

    /// `list_watched_signals` / `read_signal` expose a
    /// `watch_signal`-registered signal's live value over the bridge.
    #[test]
    fn bridge_watch_verbs_read_live_signal_value() {
        use runtime_shared::__serde_json::json;
        use runtime_world::World;

        let robot = Robot::new();
        robot.reset();
        let world = World::new();
        let sig = world.enter(|| {
            let s = runtime_world::signal(7i32);
            crate::robot_watch::watch_signal("bridge_counter", s);
            s.set(42);
            s
        });
        world.flush();

        // Bridge reads self-wrap through the driver env; none is
        // installed here, so give them the world via a real env.
        {
            let world_for_env = world.clone();
            install_driver_env(move |f| world_for_env.enter(|| f()), || {});
        }
        let out = bridge::invoke_command("read_signal", &json!({ "name": "bridge_counter" }))
            .expect("read_signal by name");
        assert_eq!(out, "\"42\"");
        let id = (world.enter(|| sig.raw_id()) & 0xffff_ffff) as u32;
        let by_id = bridge::invoke_command("read_signal", &json!({ "id": id })).unwrap();
        assert_eq!(by_id, "\"42\"");
        let list = bridge::invoke_command("list_watched_signals", &json!({})).unwrap();
        assert_eq!(
            list,
            format!(
                "[{{\"id\":{id},\"name\":\"bridge_counter\",\"value\":\"42\",\
                 \"writes\":1,\"changed_ago_ms\":null,\"writable\":false}}]"
            ),
            "old-bridge wire shape plus the additive history/writable fields \
             (no clock installed in this test, so the age is null)"
        );
        assert!(bridge::invoke_command("read_signal", &json!({})).is_err());
        clear_driver_env();
        robot.reset();
    }

    /// Nav registry: snapshot shape (wire-identical `nav_snapshot_json`),
    /// current-marking, and dead-state pruning.
    #[test]
    fn bridge_nav_verbs_snapshot_mark_active_and_prune() {
        use runtime_shared::__serde_json::json;
        use std::cell::Cell;

        let robot = Robot::new();
        robot.reset();
        let alive = Rc::new(Cell::new(true));
        let alive_in = alive.clone();
        let id = register_navigator(
            "stack_navigator",
            Some(3),
            Rc::new(move || {
                alive_in.get().then(|| NavSnapshotData {
                    active_route: "detail".into(),
                    active_path: "/detail".into(),
                    query: runtime_shared::primitives::navigator::QueryParams::parse(
                        "tag=%7E7781&tag=2&q=a%20b",
                    ),
                    depth: 2,
                    can_go_back: true,
                    base: String::new(),
                    stack: vec![
                        ("root".into(), "/".into()),
                        ("detail".into(), "/detail".into()),
                    ],
                })
            }),
        );

        let json_one =
            bridge::invoke_command("get_navigator_state", &json!({ "nav_id": id.0 })).unwrap();
        assert_eq!(
            json_one,
            format!(
                "{{\"nav_id\":{},\"element_id\":3,\"type_name\":\"stack_navigator\",\
                 \"active_route\":\"detail\",\"active_path\":\"/detail\",\
                 \"query\":{{\"q\":\"a b\",\"tag\":\"~7781\"}},\"href\":\"/detail?tag=~7781&tag=2&q=a+b\",\
                 \"depth\":2,\"can_go_back\":true,\"is_current\":true,\
                 \"base\":\"\",\"stack\":[{{\"route\":\"root\",\"path\":\"/\"}},\
                 {{\"route\":\"detail\",\"path\":\"/detail\"}}],\"controllable\":false}}",
                id.0
            ),
            "old-bridge wire shape plus the additive `query`, `href` and `controllable` \
             (query decoded, first value of a repeated key; href = path + encoded query)"
        );

        // A second navigator; marking it active flips is_current.
        let id2 = register_navigator(
            "swap_navigator",
            None,
            Rc::new(|| {
                Some(NavSnapshotData {
                    active_route: "home".into(),
                    active_path: "/".into(),
                    query: Default::default(),
                    depth: 1,
                    can_go_back: false,
                    base: String::new(),
                    stack: vec![("home".into(), "/".into())],
                })
            }),
        );
        mark_active_navigator(id2);
        let all = bridge::invoke_command("list_navigators", &json!({})).unwrap();
        use runtime_shared::__serde_json as serde_json;
        let v: serde_json::Value = serde_json::from_str(&all).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        let current: Vec<bool> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["is_current"].as_bool().unwrap())
            .collect();
        assert_eq!(current, vec![false, true], "mark_active moved the current bit");

        // Dead shared state prunes on read (old dead-Weak contract).
        alive.set(false);
        assert!(navigator_snapshot(id).is_none(), "dead snapshot pruned");
        let unknown =
            bridge::invoke_command("get_navigator_state", &json!({ "nav_id": id.0 })).unwrap();
        assert_eq!(unknown, "null");
        deregister_navigator(id2);
        assert!(all_navigators().is_empty());
        robot.reset();
    }

    /// The driver env wraps queries (`enter`) and actions (`settle`) —
    /// the transport contract. A fake env counts both.
    #[test]
    fn driver_env_enters_queries_and_settles_actions() {
        use std::cell::Cell;

        let robot = Robot::new();
        robot.reset();
        let enters: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let settles: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        {
            let enters = enters.clone();
            let settles = settles.clone();
            install_driver_env(
                move |f| {
                    enters.set(enters.get() + 1);
                    f();
                },
                move || settles.set(settles.get() + 1),
            );
        }
        let id = REGISTRY.with(|r| {
            r.borrow_mut().insert(RegistryEntry {
                kind: ElementKind::Button,
                test_id: Some("env-btn"),
                actions: ElementActions {
                    click: Some(Rc::new(|| {})),
                    ..ElementActions::default()
                },
                ..RegistryEntry::default()
            })
        });
        let el = robot.find(Query::test_id("env-btn")).expect("found");
        assert_eq!(el.id, id);
        assert_eq!(enters.get(), 1, "query ran inside env.enter");
        robot.click(&el).expect("click");
        assert_eq!(settles.get(), 1, "action settled staged work");
        clear_driver_env();
        robot.reset();
    }

    /// `get_viewport` reports the size every platform seam writes and the
    /// bucket it falls in under the installed table.
    #[test]
    fn get_viewport_reports_size_and_breakpoint() {
        Robot::new().reset();
        runtime_shared::set_viewport_size(runtime_shared::ViewportSize::new(390.0, 844.0));
        let v: serde_json::Value =
            serde_json::from_str(&bridge::invoke_command("get_viewport", &json!({})).unwrap()).unwrap();
        assert_eq!(v, json!({ "width": 390, "height": 844, "breakpoint": "xs" }));
        runtime_shared::set_viewport_size(runtime_shared::ViewportSize::new(1280.0, 800.0));
        let v: serde_json::Value =
            serde_json::from_str(&bridge::invoke_command("get_viewport", &json!({})).unwrap()).unwrap();
        assert_eq!(v["breakpoint"], "xl");
    }

    /// `set_viewport` resizes through the first root's window, outside any
    /// backend borrow, and the size lands where `get_viewport` reads it.
    #[test]
    fn set_viewport_resizes_the_first_roots_window() {
        let robot = Robot::new();
        robot.reset();
        let asked: Rc<RefCell<Vec<(f32, f32)>>> = Rc::default();
        let asked_in = asked.clone();
        REGISTRY.with(|r| {
            r.borrow_mut().insert(RegistryEntry {
                actions: ElementActions {
                    resize_window: Some(Rc::new(move |w, h| {
                        let asked = asked_in.clone();
                        Some(Box::new(move || {
                            asked.borrow_mut().push((w, h));
                            // What a backend's resize seam does.
                            runtime_shared::set_viewport_size(runtime_shared::ViewportSize::new(w, h));
                            Ok(())
                        }) as NativeInjection)
                    })),
                    ..ElementActions::default()
                },
                ..RegistryEntry::default()
            })
        });
        assert_eq!(bridge::invoke_command("set_viewport", &json!({ "width": 800, "height": 600 })).unwrap(), "\"ok\"");
        assert_eq!(*asked.borrow(), vec![(800.0, 600.0)]);
        assert_eq!(robot.viewport().breakpoint, runtime_shared::Breakpoint::Md);
        assert_eq!((robot.viewport().width, robot.viewport().height), (800.0, 600.0));

        let err = bridge::invoke_command("set_viewport", &json!({ "width": 800 })).unwrap_err();
        assert!(err.contains("'height'"), "{err}");
        let err = bridge::invoke_command("set_viewport", &json!({ "width": -1, "height": 600 })).unwrap_err();
        assert!(err.contains("positive"), "{err}");
        robot.reset();
    }

    /// A backend that can't size its own window (web, phones) answers
    /// `None`, and the verb says so and where to set the size instead.
    #[test]
    fn set_viewport_is_unsupported_without_a_resizable_window() {
        let robot = Robot::new();
        robot.reset();
        REGISTRY.with(|r| r.borrow_mut().insert(RegistryEntry::default()));
        let err = bridge::invoke_command("set_viewport", &json!({ "width": 800, "height": 600 })).unwrap_err();
        assert_eq!(err, SET_VIEWPORT_UNSUPPORTED);
        // A backend that has the hook but declines (`None`) reads the same.
        robot.reset();
        REGISTRY.with(|r| {
            r.borrow_mut().insert(RegistryEntry {
                actions: ElementActions { resize_window: Some(Rc::new(|_, _| None)), ..ElementActions::default() },
                ..RegistryEntry::default()
            })
        });
        let err = bridge::invoke_command("set_viewport", &json!({ "width": 800, "height": 600 })).unwrap_err();
        assert!(err.contains("unsupported on this platform"), "{err}");
        robot.reset();
    }
}

// Struct-update syntax for the test helper above.
#[cfg(test)]
impl Default for RegistryEntry {
    fn default() -> Self {
        Self {
            kind: ElementKind::View,
            test_id: None,
            label: None,
            label_fn: None,
            actions: ElementActions::default(),
            parent: None,
            children: Vec::new(),
            order: (0, 0),
            detached: false,
        }
    }
}
