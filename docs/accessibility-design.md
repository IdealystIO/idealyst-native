# Accessibility Design — idealyst-native

Status: **shipped** — phases 1-6 + wire-protocol plumbing landed.
Owner: framework team. Reviewers: every backend maintainer + idea-ui.

This document defined (and now records) the cross-platform accessibility
(a11y) surface for idealyst-native. Before this work the framework
exposed only `Image.accessibilityLabel` and `Link`'s native role; every
other primitive shipped with **no a11y**. Retrofitting after more
primitives ship would have been exponentially more expensive — every new
`create_*` adds another retrofit point across every backend — so the
surface landed before the primitive count grew further.

## Implementation status

| Component | Status | Notes |
|---|---|---|
| `framework_core::accessibility` module | ✅ | `Role`, `AccessibilityProps`, `AccessibilityTraits`, `LiveRegionPriority`, `AccessibilityAction`, `AccessibilityTree`, `AccessibilityNode`, `AccessibilityRect`, `PrimitiveKind`, `default_role()` — 7 unit tests |
| `Backend` trait: `update_accessibility`, `announce_for_accessibility`, `dump_accessibility_tree` | ✅ | No-op defaults; every `create_*` takes `&AccessibilityProps` |
| `Primitive` variants carry `accessibility: AccessibilityProps` | ✅ | 19 variants (control-flow `When`/`Switch`/`Repeat` excluded) |
| Walker plumbs a11y through every primitive build | ✅ | |
| **Web backend** | ✅ | ARIA attributes, `aria-live` announcer, `update_accessibility` reapplies idempotently |
| **iOS-mobile backend** | ✅ | `UIAccessibility*` setters + `UIAccessibility.post(.announcement)`. State flags without UIKit traits (CHECKED, EXPANDED, MIXED) ride `accessibilityValue`. iOS 17+ uses `UIAccessibilitySpeechAttributeAnnouncementPriority` on an `NSAttributedString` (Polite→Default, Assertive→High); older iOS falls back to a plain `NSString`. Runtime-gated via `NSProcessInfo.isOperatingSystemAtLeastVersion:`. |
| **Android backend** | ✅ | `setContentDescription` / `setTooltipText` / `setAccessibilityLiveRegion` + `announceForAccessibility`. State flags without direct API ride a `contentDescription` tail (TalkBack still announces). |
| **macOS backend** | ✅ | `NSAccessibility*` setters + `postNotificationWithUserInfo` (Polite→Medium, Assertive→High). `BUSY` trait unmapped (no AppKit equivalent). |
| **Roku backend** | ✅ no-op | Roku SceneGraph has no AT API; props dropped at the `_a11y` boundary. Audit rule `backend-roku-a11y.md` nudges plumbing when an SDK exposes it. |
| **wgpu backend** | ✅ | Per-node `accessibility` storage; `dump_accessibility_tree` builds the parallel semantics tree; `pending_announcements` queue drained by the host shell. 5 dedicated tests. |
| **Wire protocol** | ✅ | `WireAccessibilityProps` + `WireRole` (`#[serde(other)] Unknown` for forward-compat) + `WireLiveRegionPriority` + `WireAccessibilityAction` (name + `HandlerId` trampoline). Every `Create*` carries a11y; new `UpdateAccessibility` + `AnnounceForAccessibility` commands; `WIRE_VERSION=4`. SceneModel snapshot replays per-node a11y so late-joining AAS clients see current state. AX action handlers cross the wire as reverse-channel `HandlerId`s — same trampoline mechanism as `on_click` / `on_change`. |

Documented gaps (intentional):
- Future `Role` variants surface as `WireRole::Unknown` on older replayers → `None` (no role override).
- wgpu's host-shell consumer crate that projects `AccessibilityTree` into the platform AX layer (winit / AT-SPI / shell-iOS) doesn't exist yet — design contract is recorded; the consumer is the next chunk of work.

See `.claude/audits/accessibility-completeness.md` for the audit that keeps the surface in lockstep across backends.

The design follows the framework's two governing rules:

- **Rule 3 (core stays minimal)** — a11y types live in
  `framework_core::accessibility` and the Backend trait grows the
  smallest possible surface. Per-role authoring helpers (e.g.
  `aria-current="page"` shortcuts, named "landmark" components) belong
  outside core, on top of these primitives.
- **Rule 7 (backend determines how things render, implementations
  converge in behavior)** — one `Role` taxonomy, one `AccessibilityTraits`
  bitfield, one `AccessibilityProps` struct. Each backend maps to its
  native AX system. No `is_simulator()`-style per-platform branches in
  framework code; no per-platform fudge factors on the public surface.

---

## 1. `Role` taxonomy

A single `Role` enum with one variant per semantic role authors think
in. ~35 variants. Defined in `framework_core::accessibility`.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Role {
    // Structural
    Button, Link, Image, Text, Header, List, ListItem, Group, Separator,

    // Input
    TextField, TextArea, Switch, Slider, Checkbox, RadioButton, RadioGroup,
    ComboBox, SearchField,

    // Disclosure / navigation
    Tab, TabList, TabPanel, NavigationLink, MenuItem, Menu, MenuBar, Toolbar,

    // Feedback
    Alert, Status, ProgressBar, Spinner,

    // Container / overlay
    Dialog, AlertDialog, Drawer, Popover, Tooltip, Region,
}
```

`#[non_exhaustive]` so adding a variant later isn't a breaking change.

### Per-platform mapping table

`UIAccessibilityTraits` and `NSAccessibility.Role` constants are
referenced in their Swift form for readability; the actual ObjC bridge
uses the underlying NSString constants
([Apple UIAccessibilityTraits](https://developer.apple.com/documentation/uikit/uiaccessibilitytraits),
[Apple NSAccessibility.Role](https://developer.apple.com/documentation/appkit/nsaccessibility/role),
[Android AccessibilityNodeInfo](https://developer.android.com/reference/android/view/accessibility/AccessibilityNodeInfo),
[WAI-ARIA 1.2](https://www.w3.org/TR/wai-aria-1.2/)).

| Role             | iOS (UIAccessibilityTraits)            | Android (className / extras)                       | Web (ARIA `role`) | macOS (NSAccessibility.Role)    |
|------------------|-----------------------------------------|----------------------------------------------------|-------------------|---------------------------------|
| `Button`         | `.button`                              | `android.widget.Button`                            | `button`          | `.button`                       |
| `Link`           | `.link`                                | `android.widget.TextView` + `RoleDescription`=link | `link` (or `<a>`) | `.link`                         |
| `Image`          | `.image`                               | `android.widget.ImageView`                         | `img`             | `.image`                        |
| `Text`           | `.staticText`                          | `android.widget.TextView`                          | (none; static)    | `.staticText`                   |
| `Header`         | `.header`                              | `setHeading(true)`                                 | `heading` (`<h*>`)| `.staticText`+`subrole=AXHeading` |
| `List`           | (none — container)                     | `android.widget.ListView` *(see note)*             | `list`            | `.list`                         |
| `ListItem`       | (none — content inside list)           | (no first-class role; relies on parent)            | `listitem`        | `.row`                          |
| `Group`          | (none)                                 | `android.view.ViewGroup`                           | `group`           | `.group`                        |
| `Separator`      | (none — visual)                        | (no first-class role; `Divider` heuristic)         | `separator`       | `.splitter`                     |
| `TextField`      | `.searchField` or none + `accessibilityValue` | `android.widget.EditText`                  | `textbox`         | `.textField`                    |
| `TextArea`       | (none — value reflects content)        | `android.widget.EditText` + `setMultiLine(true)`   | `textbox` + `aria-multiline=true` | `.textField` + `.textArea` subrole |
| `Switch`         | `.switchButton`                        | `android.widget.Switch` + `isCheckable`            | `switch`          | `.checkBox` + `subrole=AXSwitch` |
| `Slider`         | `.adjustable`                          | `android.widget.SeekBar`                           | `slider`          | `.slider`                       |
| `Checkbox`       | (none — combine with `.button` + `accessibilityValue`) | `android.widget.CheckBox` + `isCheckable` | `checkbox`        | `.checkBox`                     |
| `RadioButton`    | (none — `.button` + `accessibilityValue`) | `android.widget.RadioButton` + `isCheckable`     | `radio`           | `.radioButton`                  |
| `RadioGroup`     | (none)                                 | `android.widget.RadioGroup`                        | `radiogroup`      | `.radioGroup`                   |
| `ComboBox`       | (none)                                 | `android.widget.Spinner`                           | `combobox`        | `.popUpButton`                  |
| `SearchField`    | `.searchField`                         | `android.widget.EditText` + `setHintText`          | `searchbox`       | `.textField` + `subrole=AXSearchField` |
| `Tab`            | (none — leaf within tab bar)           | (no first-class role; parent + `selected`) [note]  | `tab`             | `.radioButton` + `subrole=AXTabButton` |
| `TabList`        | `.tabBar`                              | `android.widget.TabWidget`                         | `tablist`         | `.tabGroup`                     |
| `TabPanel`       | (none)                                 | (no first-class role)                              | `tabpanel`        | `.group`                        |
| `NavigationLink` | `.button` + `.link`                    | `android.widget.Button` + RoleDescription "link"   | `link`            | `.link`                         |
| `MenuItem`       | `.button`                              | `setRoleDescription("menu item")`                  | `menuitem`        | `.menuItem`                     |
| `Menu`           | (none — container)                     | (no first-class role)                              | `menu`            | `.menu`                         |
| `MenuBar`        | (none)                                 | (no first-class role)                              | `menubar`         | `.menuBar`                      |
| `Toolbar`        | (none)                                 | `setRoleDescription("toolbar")`                    | `toolbar`         | `.toolbar`                      |
| `Alert`          | (post `announcement` notification)     | `setLiveRegion(POLITE\|ASSERTIVE)`                 | `alert`           | `.staticText` + announce        |
| `Status`         | (announce on update)                   | `setLiveRegion(POLITE)`                            | `status`          | `.staticText` + announce        |
| `ProgressBar`    | `.updatesFrequently` + value          | `android.widget.ProgressBar`                       | `progressbar`     | `.progressIndicator`            |
| `Spinner`        | `.updatesFrequently` + `.image`        | `android.widget.ProgressBar` (indeterminate)       | `progressbar`+ `aria-valuetext` | `.busyIndicator`        |
| `Dialog`         | (post `screenChanged`)                 | `setPaneTitle(label)`                              | `dialog`          | `.window` + `subrole=AXDialog`  |
| `AlertDialog`    | (post `screenChanged`)                 | `setPaneTitle` + assertive live region             | `alertdialog`     | `.window` + `subrole=AXSystemDialog` |
| `Drawer`         | (none — same as `Dialog`)              | `setPaneTitle`                                     | `dialog`          | `.drawer`                       |
| `Popover`        | (none)                                 | `setPaneTitle`                                     | `dialog` + `aria-modal=false` | `.popover` (no first-class) |
| `Tooltip`        | (helper text via `accessibilityHint`)  | `setRoleDescription("tooltip")`                    | `tooltip`         | `.helpTag`                      |
| `Region`         | (none)                                 | (no first-class role)                              | `region`          | `.group`                        |

#### Awkward mappings (explicit calls)

- **`Tab`** — Android has no first-class Tab role. The framework relies
  on the parent `TabList` carrying `android.widget.TabWidget` and each
  child Tab carrying `setSelected(true|false)`. Same trick for `MenuItem`
  inside `Menu`.
- **`ListItem`** — neither iOS nor Android has a first-class role; the
  parent `List` (Android `ListView` / iOS table-or-collection-view) does
  the announcement. The framework still emits the ARIA `listitem` role on
  web for screen-reader navigation.
- **`Header`** — iOS has `.header`; web has `<h1>`-`<h6>`; macOS uses the
  `AXHeading` subrole on `staticText`; Android sets `setHeading(true)`.
  All converge on "screen reader treats this as a section landmark."
- **`Drawer` / `Popover`** — macOS has a first-class `drawer` role.
  Other platforms map to `Dialog` semantics; the visual chrome diverges
  but the AX behavior is the same (modal-ish, has a label, dismissable).
- **`Separator`** — visual on iOS / Android (no AX node). Web emits
  `<hr role="separator">`. macOS uses `.splitter`. None of these are
  focusable unless paired with `Slider`-like behavior, which we leave to
  a later `OrientableSeparator` if it ever becomes a real ask.

The set is deliberately smaller than ARIA's full list. Roles like
`grid`/`treegrid`/`feed` need framework primitives we don't have; adding
them to the enum without backing primitives would invite authors to set a
role we can't honor. We leave room (`#[non_exhaustive]`) and add the
variant in the same change that adds the primitive.

---

## 2. `AccessibilityTraits` bitfield

Orthogonal per-element flags that compose with `Role`. **Implementation
choice: `bitflags!` struct, not a struct of `bool`s.** Justification:

- The set is small (~12) and stable. We don't gain IDE auto-discovery
  by exploding into named fields; `traits.contains(...)` is already
  discoverable through the type's API.
- `AccessibilityProps` is constructed and **passed by reference into
  every `create_*` call on every node**. A 12-bool struct is 12 bytes
  padded to 16; a `bitflags!` u16 is 2 bytes. Multiplied by the node
  count on a screen, the difference is real.
- The natural client-side spelling is composition (`SELECTED | DISABLED`),
  which `bitflags!` handles directly. With a bool struct, every callsite
  becomes `AccessibilityTraits { selected: true, disabled: true, ..Default::default() }`.

```rust
bitflags::bitflags! {
    #[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct AccessibilityTraits: u16 {
        // Selection / activation
        const SELECTED   = 1 << 0;
        const DISABLED   = 1 << 1;
        const EXPANDED   = 1 << 2;
        const COLLAPSED  = 1 << 3;
        const CHECKED    = 1 << 4;
        const MIXED      = 1 << 5;   // tri-state checkbox

        // Status
        const BUSY       = 1 << 6;
        const REQUIRED   = 1 << 7;
        const READONLY   = 1 << 8;
        const INVALID    = 1 << 9;

        // iOS-specific behavioral hints (see note)
        const UPDATES_FREQUENTLY = 1 << 10;
    }
}
```

Excluded from the public bitfield (folded into `Role` instead):

- **`playsSound`** / **`startsMediaSession`** — semantically these are a
  property of *what the button does*, not of the button itself. Wire
  them through a dedicated `Role::MediaButton` variant or via a future
  audio-focused primitive; do not expose at the trait level.
- **`causesPageTurn`** — book-reader specific. If we ever ship a Reader
  primitive, it adds its own role; this trait is too narrow to belong
  on the cross-cutting surface.

### Per-platform trait mapping

| Trait                 | iOS                                       | Android (NodeInfo)               | Web (ARIA)        | macOS (NSAccessibility)             |
|-----------------------|-------------------------------------------|----------------------------------|-------------------|-------------------------------------|
| `SELECTED`            | `.selected`                              | `setSelected(true)`              | `aria-selected="true"` | `.setAccessibilitySelected(true)` |
| `DISABLED`            | `.notEnabled`                            | `setEnabled(false)`              | `aria-disabled="true"` | `setAccessibilityEnabled(false)` |
| `EXPANDED`            | accessibilityValue="expanded"             | `setExpanded(true)` API 33+ / `addAction(ACTION_COLLAPSE)` | `aria-expanded="true"`  | `isAccessibilityExpanded = true` |
| `COLLAPSED`           | accessibilityValue="collapsed"            | `addAction(ACTION_EXPAND)`       | `aria-expanded="false"` | `isAccessibilityExpanded = false` |
| `CHECKED`             | accessibilityValue="checked"             | `setChecked(true)`               | `aria-checked="true"`   | `isAccessibilityChecked = true`  |
| `MIXED`               | accessibilityValue="mixed"               | `setChecked(true)` + state desc  | `aria-checked="mixed"`  | `isAccessibilityChecked = mixed`  |
| `BUSY`                | post `.layoutChanged` after          | `setLiveRegion(POLITE)` + label  | `aria-busy="true"`      | post `AXAnnouncementRequested`     |
| `REQUIRED`            | accessibilityHint "required"             | `setRoleDescription` includes "required" | `aria-required="true"` | (no first-class; via label)    |
| `READONLY`            | (combine with notEnabled if non-interactive) | `addAction` removal           | `aria-readonly="true"`  | `isAccessibilityProtectedContent` |
| `INVALID`             | accessibilityValue includes "invalid"     | `setError(msg)`                  | `aria-invalid="true"`   | (no first-class; via label)        |
| `UPDATES_FREQUENTLY`  | `.updatesFrequently`                     | `setLiveRegion(POLITE)`          | `aria-live="polite"`    | post `AXAnnouncementRequested`     |

Where a platform has no first-class flag (e.g. `INVALID` on iOS/macOS),
the backend folds the state into a derived label/hint string — the
*observable behavior* (screen reader announces "invalid") converges.

---

## 3. `AccessibilityProps` struct

The shape every `create_*` receives. Owned by `framework_core::accessibility`.

```rust
#[derive(Clone, Debug, Default)]
pub struct AccessibilityProps {
    /// Author-supplied a11y name. `None` means "derive from the
    /// primitive's natural content" (Button's label, Text's content,
    /// Image's alt, etc.). The backend implements derivation; the
    /// walker doesn't pre-fill.
    pub label: Option<String>,

    /// Longer description ("Double tap to open menu"). Maps to
    /// `accessibilityHint` (iOS/macOS), `aria-describedby` (web),
    /// content-description tail on Android.
    pub hint: Option<String>,

    /// Author override of the inferred role. `None` means
    /// "backend uses the primitive's default role" (Button → Button,
    /// TextInput → TextField, etc.). The override exists for cases
    /// like a `Pressable` styled as a navigation link.
    pub role: Option<Role>,

    /// Orthogonal state flags. Default = empty.
    pub traits: AccessibilityTraits,

    /// Hide from the accessibility tree entirely. Maps to
    /// `accessibilityElementsHidden` (iOS), `importantForAccessibility =
    /// NO_HIDE_DESCENDANTS` (Android), `aria-hidden="true"` (web),
    /// `isAccessibilityElement = false` (macOS).
    pub hidden: bool,

    /// Live-region priority. `None` = not a live region. Other values
    /// hint platform-AX to announce updates polite-ly or assertive-ly.
    pub live_region: Option<LiveRegionPriority>,

    /// Custom AX actions (label + handler). Maps to
    /// `accessibilityCustomActions` (iOS/macOS), `addAction` with a
    /// labelled int (Android), `aria-keyshortcuts` / custom widget
    /// dispatch (web). Empty by default.
    pub actions: Vec<AccessibilityAction>,

    /// Stable identifier for AX-tooling tests (XCUI, UIAutomator,
    /// Selenium). Maps to `accessibilityIdentifier` (iOS),
    /// `setViewIdResourceName`-equivalent extras (Android), `id`
    /// attribute (web). **Separate from the framework's `test_id` for
    /// the Robot harness** — Robot ids are for in-process e2e; this
    /// `identifier` is for external AX-driven tooling.
    pub identifier: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LiveRegionPriority { Polite, Assertive }

#[derive(Clone)]
pub struct AccessibilityAction {
    pub name: String,
    pub handler: std::rc::Rc<dyn Fn()>,
}
```

### Design decisions, with rationale

**`label: Option<String>` rather than an enum.** Considered:

```rust
pub enum AccessibilityLabel { Auto, Static(String), DerivedFrom(Signal<String>) }
```

Rejected. The reactive case is already handled by the surrounding
reactive system — author code that wants a reactive label wraps the
prop builder in an `Effect` and re-calls `update_accessibility`. The
"use the primitive's visible text" case (Button without explicit label
falls back to its text) is the **backend's** job — it's the only side
that knows what the visible text is, and Flutter solves this the same
way ([SemanticsConfiguration](https://api.flutter.dev/flutter/semantics/SemanticsConfiguration-class.html)
absorbs child semantics when the explicit label is empty). Keeping the
prop a simple `Option<String>` matches Flutter's `SemanticsConfiguration.label`
field exactly and avoids re-inventing reactivity.

**`role: Option<Role>`, with backend-side inference when `None`.** Every
primitive has a hard-coded default role (`Button` → `Role::Button`,
`Pressable` → `Role::Button`, `Text` → `Role::Text`, …). The walker
passes the inferred default in `props.role`'s slot only when the author
didn't override; the backend then doesn't have to know which primitive
it's working with to find the right role — it just reads
`props.role.unwrap_or(default_for_primitive)`. Default-role table lives
in `framework_core::accessibility::default_role()` and is documented in
the primitive index.

**`actions: Vec<AccessibilityAction>` belongs in core.** Custom AX
actions are not a peripheral feature — VoiceOver / TalkBack expose them
as the rotor menu, and any moderately complex list ("delete row",
"archive row" without showing the buttons) needs them. Cost is one
`Vec` per primitive (typically empty); the indirection is a single
non-null check in the walker and the backend.

**No `accessible_children` field.** Considered: a manual child-override
that lets a group primitive re-flatten its visual children into a
different a11y hierarchy. Rejected at this layer — the same effect is
achievable with `hidden: true` on the unwanted children plus a
`Role::Group` on the visible parent. If a real ask appears (composite
custom controls that want sub-elements distinct from visual layout), it
goes in a follow-on RFC; we have no consuming primitive today.

**`identifier` is separate from `test_id` for Robot.** Robot is the
in-process e2e harness owned by the framework. `identifier` is the
external AX hook (XCUITest's `accessibilityIdentifier`, web's `id`,
Android's resource name). They serve different audiences and should not
share a field — if you change your test harness, you do not want every
e2e id leaking into your shipped accessibility tree.

---

## 4. `Backend` trait changes

The trait grows three new methods + an `AccessibilityProps` parameter on
every existing `create_*`. The new methods all have no-op defaults so
backends can land them incrementally.

### Existing signatures: `&AccessibilityProps` added

```rust
fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node;
fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node;
fn create_button(
    &mut self,
    label: &str,
    on_click: &crate::derive::Action,
    leading_icon: Option<&IconData>,
    trailing_icon: Option<&IconData>,
    a11y: &AccessibilityProps,
) -> Self::Node;
// ... and every other create_*: create_image, create_pressable,
// create_text_input, create_text_area, create_toggle, create_slider,
// create_scroll_view, create_video, create_activity_indicator,
// create_virtualizer, create_link, create_navigator, create_portal,
// create_external — all grow the parameter.
```

### New methods

```rust
/// Replace the a11y props on an already-created node. Called by the
/// walker's reactive a11y Effect when any field of the prop changes
/// (e.g. `traits` flips `SELECTED`, `label` updates).
///
/// Backends translate to per-attribute setter calls
/// (`accessibilityLabel = ...`, `setAttribute('aria-label', ...)`,
/// `setContentDescription(...)`).
fn update_accessibility(
    &mut self,
    node: &Self::Node,
    a11y: &AccessibilityProps,
) {
    // default: no-op (backends without a11y still render)
}

/// Post a one-shot announcement to the platform's a11y subsystem.
/// Independent of any node — used for transient feedback ("Form
/// submitted", "Loading complete") that doesn't have a stable
/// focus target.
///
/// - iOS: `UIAccessibility.post(notification: .announcement, argument: msg)`
/// - Android: `View.announceForAccessibility(msg)` on the root view,
///   priority controls `AccessibilityEvent.TYPE_ANNOUNCEMENT` vs
///   `setLiveRegion` on a hidden live-region node
/// - Web: write `msg` into a hidden `<div aria-live="polite|assertive">`
///   that the screen reader observes
/// - macOS: post `NSAccessibilityAnnouncementRequestedNotification`
///   with a `NSAccessibilityAnnouncementKey` + priority key
fn announce_for_accessibility(
    &mut self,
    msg: &str,
    priority: LiveRegionPriority,
) {
    // default: no-op
}

/// GPU-backend semantics-tree dump. Native widget backends return
/// `None`: their AX data lives on the native widget already, the
/// platform AX walker traverses it directly.
///
/// The wgpu / future canvas backends override to return their
/// parallel semantics tree. The host crate (winit shell, AppKit
/// app delegate) reads the tree on every layout commit and
/// projects it into the host platform's AX API.
fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
    None
}
```

### Ownership / borrowing decisions

**`&AccessibilityProps`, borrowed.** The walker has the prop in hand
from the `Primitive` it's expanding; it can lend it to `create_*` for
the duration of the call. No `Rc` needed at the trait boundary — each
backend either ignores the prop (no allocation), maps it to per-attribute
setter calls (no retention), or for backends that need the prop later
(web's reactive a11y binding, GPU's semantics-tree node) clones the
parts it needs into its own state. This matches how `&Rc<StyleRules>`
flows through `apply_style` today.

**`Primitive` enum grows an `accessibility: AccessibilityProps` field on
each variant — not a derived-from-other-props computation.** Reasoning:
the prop has fields (`hint`, `actions`, `identifier`, custom `role`)
that have no analog in the primitive-specific props; trying to compute
them post-hoc means inventing a parallel a11y-spec layer in core. Adding
the field directly is what every other prop system does (Flutter's
`SemanticsConfiguration`, React Native's `accessibilityProps`).

**Relationship to existing reactive updates.** When the author changes a
button's *visible* label (the existing `update_button_label` Effect),
the framework also re-fires `update_accessibility` if and only if
`props.label.is_none()` (i.e. the a11y label is derived from the
visible label). When `props.label.is_some()`, the a11y label is decoupled
and only changes when the *a11y prop itself* changes. Default-derivation
keeps the common case zero-config; explicit-prop is the escape hatch.

The walker has a single `Effect` per node that observes the
`AccessibilityProps` for that primitive and fires `update_accessibility`
when any field of the prop changes. Other primitive-specific update paths
(e.g. `update_text`) **do not** fire `update_accessibility` — they only
fire it when the *derived label* changes, by re-running the a11y prop
Effect.

---

## 5. GPU-backend semantics tree

The wgpu backend renders primitives as quads. There is no `UIView`,
`UIButton`, or `<div>` for the host platform's a11y walker to find. The
host needs a **parallel semantics tree** that mirrors the visual tree's
hierarchy + traversal order, then serializes it to the host platform's
AX API once per layout commit. Flutter solved exactly this problem with
its `SemanticsNode` /
[`SemanticsConfiguration`](https://api.flutter.dev/flutter/semantics/SemanticsConfiguration-class.html)
/ `SemanticsBinding` triple. Our design is a deliberate subset of theirs.

### Tree shape

```rust
pub struct AccessibilityTree {
    pub root: AccessibilityNode,
}

pub struct AccessibilityNode {
    /// Stable id assigned by the backend; the host uses it as the
    /// AX walker's element id (NSAccessibility children, iOS
    /// UIAccessibilityElement.accessibilityFrameInContainerSpace
    /// lookups, AT-SPI object refs on Linux).
    pub id: u64,

    /// Resolved a11y props for this node (label may be the derived
    /// fallback by this point — the backend has folded in the
    /// primitive's default-label rule).
    pub props: AccessibilityProps,

    /// Default role inferred from the primitive (so the host doesn't
    /// have to repeat the inference). Author-supplied `props.role`
    /// has already taken precedence if set.
    pub role: Role,

    /// Bounds in the wgpu surface's coordinate space (device-independent
    /// pixels, origin at top-left). The host converts to its
    /// platform's accessibility coordinate space.
    pub bounds: ViewportRect,

    /// Children in traversal order (top-to-bottom, left-to-right
    /// in LTR). Distinct from paint order so screen-reader nav goes
    /// the user-meaningful way even if z-ordering would suggest
    /// otherwise. Flutter splits these out the same way.
    pub children: Vec<AccessibilityNode>,
}
```

### When the host queries the tree

The host (the framework's wgpu-shell crate that owns the window + AX
glue) queries `dump_accessibility_tree()`:

1. **After layout, before paint** — the layout pass has produced new
   bounds and the wgpu backend has updated its internal semantics-node
   bounds to match. The host pulls the tree once, diffs against the
   previous version (by node id), and submits the diff to the platform
   AX API (`accessibilityChildren()` returning the new array; iOS
   `UIAccessibilityElement` array replacement).
2. **On AX-driven focus change** — when the platform AX walker requests
   the next element (VoiceOver swipe, TalkBack volume-key), the host
   asks the backend for the current tree to resolve the focus target
   by id and report bounds back.

Both query sites read the tree; the backend never *pushes* changes
unsolicited. The host already has the layout-commit and focus-event
hooks (it's the OS shell), so adding the pull at those two points is
natural.

### Focus / activation event flow

Platform AX walker → host shell → backend → framework → user code:

1. User triple-taps in VoiceOver to activate the focused element (id
   `42`).
2. macOS sends `accessibilityPerformPress` to the wgpu `NSView`; the
   host's `NSAccessibility` impl receives it.
3. Host calls into the wgpu backend: "fire activation for AX node 42."
4. Backend looks up the primitive id that owns node 42 (a map kept on
   the side of its semantics tree), translates to the framework's
   primitive-press event (the same one a touch handler would fire), and
   dispatches.
5. Framework runs the author's `on_click` closure inside the appropriate
   `Effect` scope.

The same flow handles slider adjustment (`accessibilityIncrement` →
slider's `on_change`), text-field editing (`accessibilityPerformSet:`
→ text input's `on_change`), and custom actions
(`accessibilityPerformAction:withIdentifier:` → matching entry from
`AccessibilityProps::actions`).

### What we adopt from Flutter and what we don't

Adopt:

- **Parallel semantics tree separate from render tree.** Same shape,
  same lifetime, same query model.
- **`SemanticsConfiguration`-shaped props.** Our `AccessibilityProps` is
  a deliberate near-clone (`label` / `value` / `hint`, role-as-flag-or-enum,
  state flags).
- **Traversal-order distinct from paint-order.** The backend computes
  traversal from the layout tree's natural reading order, ignoring
  z-index. This matters as soon as the GPU backend gets overlapping
  content (modals over content).

Reject:

- **`SemanticsAction`-as-bitfield.** Flutter packs actions into a
  bitfield of pre-defined slots (tap, longPress, increase, decrease,
  scrollUp, …). We use `Vec<AccessibilityAction>` with author-defined
  names because our action set isn't fixed — the framework doesn't know
  every action ahead of time. The standard ones (tap, scroll, increase)
  are derived from primitive type + state, not from the action list.
- **Per-RenderObject `markNeedsSemanticsUpdate`.** Flutter does
  fine-grained semantics diffing because their render tree is huge and
  per-frame. Ours commits tree dumps at layout boundaries (much rarer);
  the diff cost is acceptable as a full-tree compare.

### Native backends don't implement `dump_accessibility_tree`

iOS / Android / web / macOS native widgets carry a11y data on the
widget itself. `dump_accessibility_tree` returns `None` (the trait
default) on those backends and the host platform's AX walker reads from
the widget directly. The GPU backend is the only one that needs the
parallel tree; the trait method exists so we don't have to special-case
the GPU shell elsewhere.

---

## 6. `announce_for_accessibility` semantics

One author-facing call:

```rust
backend.announce_for_accessibility("Form submitted", LiveRegionPriority::Polite);
```

Per-backend translation:

| Backend | Call                                                                                       | Notes |
|---------|--------------------------------------------------------------------------------------------|-------|
| iOS     | `UIAccessibility.post(notification: .announcement, argument: msg)`                         | iOS 17+ posts an `NSAttributedString` carrying `UIAccessibilitySpeechAttributeAnnouncementPriority` (Polite→`UIAccessibilityPriorityDefault`, Assertive→`UIAccessibilityPriorityHigh`); older iOS falls back to a plain `NSString`. Runtime-gated via `NSProcessInfo.isOperatingSystemAtLeastVersion:` so a single binary serves both. ([source](https://developer.apple.com/documentation/uikit/uiaccessibility/notification)) |
| Android | `rootView.announceForAccessibility(msg)` for `Polite`. `Assertive` uses a hidden live-region view with `setLiveRegion(LIVE_REGION_ASSERTIVE)` + `sendAccessibilityEvent(TYPE_ANNOUNCEMENT)`. ([source](https://developer.android.com/reference/android/view/View#announceForAccessibility(java.lang.CharSequence))) |
| Web     | Write `msg` into a hidden `<div role="status" aria-live="polite">` (or `aria-live="assertive"`) appended to `<body>`. Clear after a short delay so re-announcements of the same string work. |
| macOS   | `NSAccessibility.post(element: window, notification: .announcementRequested, userInfo: [.announcement: msg, .priority: priority])` |
| wgpu    | Forwards through the host shell to the host platform's announcement API (iOS-shell uses the iOS branch, AppKit-shell uses the macOS branch). |
| Roku    | Roku SceneGraph has no programmatic announce API. **Document as no-op**, log once at debug level. |

A11y announcements **do not** fire from within reactive Effects
automatically — they're imperative. Authors call
`accessibility::announce(...)` (a small framework helper that resolves
the current backend and forwards) when their use case demands it.
Reactive announcement of state changes goes through `live_region` on the
node carrying the changing value; the backend re-announces on update.

---

## 7. Migration plan

Eight phases. Phases 1-3 can land in a single PR (no behavior change on
any backend). Phase 4 is the breaking-trait change. Phases 6a-6f land
sequentially per backend; only 6a (web) and 6b (iOS) gate the v1 launch.

| Phase | Scope | Files | Effort | Parallelizable |
|-------|-------|-------|--------|----------------|
| **1. Core types** | `Role`, `AccessibilityTraits`, `AccessibilityProps`, `AccessibilityAction`, `LiveRegionPriority`, `AccessibilityNode`, `AccessibilityTree`. Add `framework_core::accessibility` module. Unit tests for default values + `Role`/trait mapping helpers. | `crates/framework/core/src/accessibility.rs` (new), `crates/framework/core/src/lib.rs` (re-export). | ~1d | No (foundation) |
| **2. Backend trait additions** | Add `update_accessibility`, `announce_for_accessibility`, `dump_accessibility_tree` to `Backend` trait — **all with no-op defaults**. No existing-method changes yet. | `crates/framework/core/src/backend.rs` | ~0.5d | No (gates phases 4+) |
| **3. Primitive enum field** | Add `accessibility: AccessibilityProps` field to every `Primitive::*` variant. Builder methods (`.label(...)`, `.hint(...)`, `.role(...)`, `.traits(...)`, `.identifier(...)`, etc.) on every primitive's builder. Default `AccessibilityProps::default()` so existing call sites keep compiling. | `crates/framework/core/src/primitive.rs`, every builder in `crates/framework/core/src/primitives/*.rs`, macros in `crates/framework/macros/src/ui.rs` if needed for shorthand. | ~2d | Yes (per primitive) |
| **4. Walker plumbing** | Every `walker::build_*` function reads its primitive's `accessibility` field and passes it to the backend's `create_*` call. Every `create_*` call site updated to pass `&primitive.accessibility`. | `crates/framework/core/src/walker/*.rs` | ~1d | No (depends on phase 3) |
| **5. Breaking `Backend` trait change** | Add `a11y: &AccessibilityProps` parameter to every `create_*` method on the trait. All current backends update signatures with no behavior change (ignore the new param). | `crates/framework/core/src/backend.rs` + every backend's matching `impl Backend`. | ~1d | No (gates phases 6) |
| **6a. Web backend** | Apply `aria-label`, `aria-describedby`, `role`, `aria-*` state attrs in `create_*` and `update_accessibility`. Implement `announce_for_accessibility` via a hidden polite/assertive live region pair appended to `<body>` once. | `crates/backend/web/src/*.rs` | ~3d | Yes (parallel with 6b–6f) |
| **6b. iOS backend** | Set `accessibilityLabel`/`Hint`/`Traits`/`Value`/`Identifier` on each `UIView` in `create_*` and `update_accessibility`. Map `Role` → trait bag per the table. Wire `announce_for_accessibility` to `UIAccessibility.post(.announcement, …)`. | `crates/backend/ios/mobile/src/*.rs` | ~4d | Yes |
| **6c. Android backend** | Call `setContentDescription`/`setRoleDescription`/`setCheckable`/`setChecked`/`setEnabled`/`setLiveRegion` in `create_*` and `update_accessibility`. Map `Role` → `setClassName(...)`. Wire `announce_for_accessibility` to `rootView.announceForAccessibility(msg)` (Polite) or hidden live-region path (Assertive). | `crates/backend/android/mobile/src/*.rs` | ~4d | Yes |
| **6d. macOS backend** | NSAccessibility protocol overrides on every NSView subclass: `accessibilityLabel`, `accessibilityRole`, `accessibilityHelp` (= hint), `accessibilityChildren` override only when `accessible_children`-style behavior needed (currently never). Wire announcement to `NSAccessibility.post(.announcementRequested, …)`. | `crates/backend/macos/src/*.rs` | ~3d | Yes |
| **6e. Roku backend** | Roku SceneGraph has no first-class a11y model. Document `update_accessibility` and `announce_for_accessibility` as no-ops. Mark in CLAUDE.md and add a single-test that asserts the no-op compiles, so the trait change doesn't silently regress. | `crates/backend/roku/src/lib.rs` | ~0.5d | Yes |
| **6f. wgpu backend (GPU)** | Build the parallel semantics tree. Implement `create_*` to attach `AccessibilityProps` + bounds to a new `SemanticsNode` keyed by primitive id. Implement `update_accessibility` to patch the node. Implement `dump_accessibility_tree`. Host crate (wgpu-shell) writes the tree into the host platform AX layer (NSAccessibility on macOS, UIAccessibilityElement array on iOS, AT-SPI on Linux). | `crates/render/wgpu/src/*.rs` + new `crates/render/wgpu/src/accessibility.rs`. Host wgpu-shell crate(s). | ~7d | Yes (parallel with 6a-6e) |
| **7. End-to-end test coverage** | Per-backend tests: for each primitive, build it with non-default `AccessibilityProps` and assert the backend wrote the corresponding native attribute. Web uses jsdom; iOS/Android use snapshot tests on `accessibilityLabel`-equivalent string capture from the FFI layer; wgpu reads back via `dump_accessibility_tree`. | tests under each backend crate. | ~2d | Yes |
| **8. Docs + examples** | Add `accessibility.md` user-facing doc. Update `primitives.md` to list each primitive's default role. Audit-rule under `.claude/audits/` that flags new primitives missing default-role registration. | `docs/accessibility.md`, `docs/primitives.md`, `.claude/audits/*.md`. | ~1d | After phase 6 |

**Total**: ~22-28 engineer-days (single-stream). With phases 6a-6f
fanned out across backend maintainers in parallel, the critical path is
phase 1 → 2 → 3 → 4 → 5 → (6a + 6b) → 7 + 8, roughly 12-15 calendar
days.

**Phase ordering note.** Phases 1-5 land in a single PR with no
behavior change on any backend. Phase 6a + 6b are the v1 launch (web +
iOS get real a11y). 6c-6f land independently after. This sequencing
matches the framework's tradition of growing the Backend trait first
with no-op defaults, then filling in per-backend implementations.

---

## 8. Open design questions

Five items the team needs to resolve before phase-1 lands:

1. **Reading current focus programmatically.** Does author code need to
   query "what AX element is currently focused" and/or
   "programmatically move AX focus to element X"? VoiceOver / TalkBack
   support is uneven (iOS has `UIAccessibility.focusedElement`, web
   has `document.activeElement`-but-not-AX-focus, Android has nothing
   first-class). If we expose it, what's the API? `Ref<Handle>::focus_accessibility()`?
   Recommendation: defer until we have a consuming use case, document
   that authors can't read AX focus today.

2. **Focus management vs. `Ref<H>` system.** The framework's `Ref` system
   already exposes imperative ops like `.focus()` (text input). Should
   that *also* announce a focus-change to the AX layer, or are
   keyboard-focus and AX-focus separate concerns? On iOS they're
   already separate (keyboard focus vs. VoiceOver cursor); on web
   they're tied. Need a decision before `update_accessibility` lands.

3. **`AccessibilityProps: Clone` or borrowed-only?** Marked `Clone` in
   the strawman. The wgpu backend needs an owned copy to put in its
   semantics tree; other backends only need a borrowed pass-through.
   `Clone` costs us String + Vec clones per node on the wgpu path —
   non-negligible at 1k+ nodes. Alternative: backend-specific
   conversion to a leaner `AccessibilityNodeProps` form at tree-build
   time. Recommendation: ship `Clone` for now, profile, optimize
   later.

4. **Per-primitive default-role table location.** Should `default_role()`
   live as a method on `Primitive`, a free function in
   `framework_core::accessibility`, or be inlined into each `build_*`
   walker call? The macros / ui-layer may want to query it for
   compile-time validation ("you set `role: Role::Slider` on a
   `Button`; that's almost certainly wrong"). If we want that, the
   table needs to be a stable, documented API surface (own module).

5. **Live regions and the reactive system — composability.** When a
   `Status` primitive has `live_region: Some(Polite)` and the inner
   text reactively changes, the backend needs to be told both about
   the new text *and* that this constitutes a live-region update worth
   announcing. The naive design re-fires `update_accessibility` on
   every text change. That risks announcing every typo in a search
   field. Need a rule for "what change is announce-worthy" — likely
   "only when `AccessibilityProps.label` itself changes (computed or
   raw); not when the primitive's visible text changes." But that
   means the `update_text` path can't auto-derive the a11y label, or
   we re-announce on every keystroke. Resolve before phase 6 lands.

---

## References

- [Apple `UIAccessibilityTraits`](https://developer.apple.com/documentation/uikit/uiaccessibilitytraits)
- [Apple `UIAccessibilityElement`](https://developer.apple.com/documentation/uikit/uiaccessibilityelement)
- [Apple `UIAccessibility.Notification`](https://developer.apple.com/documentation/uikit/uiaccessibility/notification)
- [Apple `NSAccessibility` protocol](https://developer.apple.com/documentation/appkit/nsaccessibility)
- [Apple `NSAccessibility.Role`](https://developer.apple.com/documentation/appkit/nsaccessibility/role)
- [Android `AccessibilityNodeInfo`](https://developer.android.com/reference/android/view/accessibility/AccessibilityNodeInfo)
- [Android `View.announceForAccessibility`](https://developer.android.com/reference/android/view/View#announceForAccessibility(java.lang.CharSequence))
- [WAI-ARIA 1.2](https://www.w3.org/TR/wai-aria-1.2/)
- [Flutter `SemanticsNode`](https://api.flutter.dev/flutter/semantics/SemanticsNode-class.html)
- [Flutter `SemanticsConfiguration`](https://api.flutter.dev/flutter/semantics/SemanticsConfiguration-class.html)
