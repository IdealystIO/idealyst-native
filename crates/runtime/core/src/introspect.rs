//! Platform-native render introspection.
//!
//! This module defines the data model for reading back **what a backend
//! actually rendered** for a primitive — the resolved geometry and visual
//! properties as the platform itself reports them — plus a backend-agnostic
//! tree-walk helper that assembles a primitive's native sub-hierarchy.
//!
//! # Why this exists
//!
//! The framework's promise is cross-platform parity: one author tree, native
//! output that looks the same on every backend. Screenshots prove *a* pixel
//! result but can't be diffed structurally. [`NativeNode`] gives a structured,
//! per-primitive read of the **platform's resolved state** — colors, corner
//! radii, fonts, frames — so two running apps (say web and macOS) can have
//! their trees captured over the robot bridge and compared key-by-key to find
//! parity drift.
//!
//! # The cardinal rule: read from the platform, never from author input
//!
//! Every value in a [`NativeNode`] MUST be read from the live native object
//! (a `CALayer`'s `backgroundColor`, the DOM's `getComputedStyle`, a resolved
//! `NSFont`) — **not** echoed back from the framework's own style structs.
//! Echoing author input would make the parity check tautological: it would
//! report "the styles we asked for", not "the styles the platform applied".
//! The whole point is to catch the cases where those two disagree.
//!
//! # Native sub-hierarchy
//!
//! A single framework primitive can be built from several native objects (an
//! `NSScrollView`'s clip + document views, a text field's internal editor).
//! [`NativeNode::children`] holds *those* platform sub-objects — but stops at
//! any descendant that is itself a framework element root (a sibling/child
//! primitive with its own registry entry and its own `introspect_native`).
//! [`collect_native_tree`] encodes that boundary walk once so both backends
//! share — and tests cover — the tricky pruning logic.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A node in the platform-native render tree, read back from a live native
/// object. Recursive: [`children`](NativeNode::children) holds the platform
/// sub-objects that compose this one primitive (not framework child elements).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeNode {
    /// The platform's own class/tag name for this object, read from the
    /// object itself — e.g. `"NSTextField"`, `"CALayer"`, `"div"`, `"input"`.
    pub class: String,
    /// Optional hint identifying which framework primitive (or sub-part of
    /// one) this native object backs — e.g. `"text_input"`,
    /// `"scroll_view.content"`. `None` for pure-platform internals the
    /// framework didn't name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Geometry in **logical px**, relative to the window/viewport (matches
    /// `Backend::absolute_frame` semantics so web `getBoundingClientRect` and
    /// macOS window-relative frames are directly comparable).
    pub frame: NativeRect,
    /// Resolved visual properties, normalized to canonical keys + units (see
    /// the `keys` module). A key is **absent** when the platform doesn't
    /// expose it for this object — absence is meaningful and distinct from a
    /// zero value, so the external diff must treat "missing" and "0" apart.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: BTreeMap<String, NativeValue>,
    /// Platform sub-objects composing this primitive. Empty for leaf
    /// primitives that map to a single native object.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NativeNode>,
}

impl NativeNode {
    /// A leaf node with a class + frame and no props/children yet — the
    /// shallow read a backend fills before [`collect_native_tree`] attaches
    /// the platform sub-hierarchy.
    pub fn leaf(class: impl Into<String>, frame: NativeRect) -> Self {
        Self {
            class: class.into(),
            role: None,
            frame,
            props: BTreeMap::new(),
            children: Vec::new(),
        }
    }

    /// Builder: set the framework role hint.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    /// Insert a canonical property, skipping `None` so callers can pipe an
    /// optional read straight through without a branch.
    pub fn set(&mut self, key: &str, value: Option<NativeValue>) {
        if let Some(v) = value {
            self.props.insert(key.to_string(), v);
        }
    }
}

/// A rectangle in logical pixels. Defined here (rather than reusing
/// `ViewportRect`) so the introspection model is self-contained and its
/// serialized shape is owned by this module.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A canonical, platform-normalized property value. Serializes as
/// `{ "type": "<variant>", "value": <payload> }` so the external diff can
/// compare typed values (with per-type tolerance) rather than parsing
/// stringly-typed platform encodings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum NativeValue {
    /// Straight (non-premultiplied) sRGB RGBA, each channel `0.0..=1.0`.
    Color([f32; 4]),
    /// A length in logical pixels.
    Length(f32),
    /// A unitless number (e.g. font weight 400, opacity 0.5).
    Number(f32),
    /// Text content the platform is actually displaying.
    Text(String),
    /// A boolean flag (e.g. `hidden`).
    Flag(bool),
}

/// Canonical property keys. Both backends populate the **same** keys from
/// their respective platform reads so the external diff compares by key. Add
/// new platform specifics as new canonical keys here rather than as free-form
/// strings — that keeps cross-platform diffs structured.
pub mod keys {
    /// Resolved background fill (`NativeValue::Color`).
    pub const BACKGROUND_COLOR: &str = "background_color";
    /// Resolved opacity, `0.0..=1.0` (`NativeValue::Number`).
    pub const OPACITY: &str = "opacity";
    /// Corner radius in logical px (`NativeValue::Length`).
    pub const CORNER_RADIUS: &str = "corner_radius";
    /// Border width in logical px (`NativeValue::Length`).
    pub const BORDER_WIDTH: &str = "border_width";
    /// Border color (`NativeValue::Color`).
    pub const BORDER_COLOR: &str = "border_color";
    /// Foreground/text color (`NativeValue::Color`).
    pub const TEXT_COLOR: &str = "text_color";
    /// Resolved font family name (`NativeValue::Text`).
    pub const FONT_FAMILY: &str = "font_family";
    /// Resolved font size in logical px (`NativeValue::Length`).
    pub const FONT_SIZE: &str = "font_size";
    /// Resolved numeric font weight, e.g. 400/700 (`NativeValue::Number`).
    pub const FONT_WEIGHT: &str = "font_weight";
    /// The actually-displayed text (`NativeValue::Text`).
    pub const TEXT: &str = "text";
    /// Shadow blur radius in logical px (`NativeValue::Length`).
    pub const SHADOW_RADIUS: &str = "shadow_radius";
    /// Shadow color (`NativeValue::Color`).
    pub const SHADOW_COLOR: &str = "shadow_color";
    /// Whether the platform considers the object hidden (`NativeValue::Flag`).
    pub const HIDDEN: &str = "hidden";
}

/// Assemble a primitive's native tree from a root platform handle.
///
/// Backend-agnostic so the boundary-pruning logic is written — and tested —
/// once. `H` is the backend's native handle type (an `NSView`, a DOM node).
///
/// - `read` does the shallow per-object read (class/role/frame/props) from the
///   live platform object. It MUST read from the platform, never author input.
/// - `children` enumerates a node's native sub-objects.
/// - `is_boundary` returns `true` for a node that is itself a **framework
///   element root** — a sibling/child primitive with its own registry entry.
///   The walk does not descend into boundaries: those are separate elements,
///   introspected on their own. The boundary check is applied to descendants
///   only; the `root` is always read (it is the element being introspected).
///
/// The result mirrors the platform's synthesized sub-hierarchy for exactly one
/// primitive, with sibling/child primitives pruned at the boundary.
pub fn collect_native_tree<H>(
    root: &H,
    read: &impl Fn(&H) -> NativeNode,
    children: &impl Fn(&H) -> Vec<H>,
    is_boundary: &impl Fn(&H) -> bool,
) -> NativeNode {
    let mut node = read(root);
    node.children = children(root)
        .into_iter()
        .filter(|c| !is_boundary(c))
        .map(|c| collect_native_tree(&c, read, children, is_boundary))
        .collect();
    node
}
