# Accessibility Design — idealyst-native


> **Status: implemented.** The types live in `runtime_shared::accessibility`
> (`crates/runtime/shared/src/accessibility.rs`, re-exported as
> `runtime_core::…`). Every node primitive's payload in
> `crates/runtime/vocabulary/src/prims/` carries an `a11y: AccessibilityProps`
> field; its mount handler (`crates/runtime/vocabulary/src/handlers/`) passes
> it to the backend's `create_*` call; and the rest of the surface is the
> `caps::A11yOps` capability (`crates/runtime/vocabulary/src/caps/a11y.rs`).
> Author-facing guide: [`accessibility.md`](accessibility.md). The
> `accessibility-completeness` audit (`.claude/audits/`) keeps the surface in
> lockstep across backends.

This document defines the cross-platform accessibility (a11y) surface for
idealyst-native: one `Role` taxonomy, one `AccessibilityTraits` bitfield,
one `AccessibilityProps` struct, and the mapping each backend applies to its
native AX system.

## Implementation status

| Component | Status | Notes |
|---|---|---|
| `runtime_shared::accessibility` | ✅ | `Role`, `AccessibilityProps`, `AccessibilityTraits`, `LiveRegionPriority`, `AccessibilityAction`, `AccessibilityTree`, `AccessibilityNode`, `AccessibilityRect`, `PrimitiveKind`, `default_role()` |
| `caps::A11yOps`: `update_accessibility`, `announce_for_accessibility`, `dump_accessibility_tree` | ✅ | No-op defaults; every `create_*` capability method takes `&AccessibilityProps` |
| Primitive payloads carry `a11y: AccessibilityProps`; handlers pass it to `create_*` | ✅ | Control flow (`when` / `switch` / keyed lists / fragments) has none |
| **Web backend** | ✅ | ARIA attributes, `aria-live` announcer, `update_accessibility` reapplies idempotently (`crates/backend/web/src/a11y.rs`) |
| **iOS-mobile backend** | ✅ | `UIAccessibility*` setters + `UIAccessibility.post(.announcement)`. State flags without UIKit traits (CHECKED, EXPANDED, MIXED) ride `accessibilityValue`. iOS 17+ uses `UIAccessibilitySpeechAttributeAnnouncementPriority` on an `NSAttributedString` (Polite→Default, Assertive→High); older iOS falls back to a plain `NSString`. Runtime-gated via `NSProcessInfo.isOperatingSystemAtLeastVersion:`. |
| **Android backend** | ✅ | `setContentDescription` / `setTooltipText` / `setAccessibilityLiveRegion` + `announceForAccessibility`. State flags without direct API ride a `contentDescription` tail (TalkBack still announces). |
| **macOS backend** | ✅ | `NSAccessibility*` setters + `postNotificationWithUserInfo` (Polite→Medium, Assertive→High). `BUSY` trait unmapped (no AppKit equivalent). |
| **Roku backend** | ✅ no-op | Roku SceneGraph has no AT API; props dropped at the `_a11y` boundary. Audit rule `backend-roku-a11y.md` nudges plumbing when an SDK exposes it. |
| **GPU backend** | ✅ | Per-node `accessibility` storage; `dump_accessibility_tree` builds the parallel semantics tree; `pending_announcements` queue drained by the host shell. `crates/gpu-backend/host/wgpu-accesskit` projects the tree into the platform AX layer through AccessKit (wired in `host-winit`). |
| **Wire protocol** | ✅ | `WireAccessibilityProps` + `WireRole` (`#[serde(other)] Unknown` for forward-compat) + `WireLiveRegionPriority` + `WireAccessibilityAction` (name + `HandlerId` trampoline). Every `Create*` carries a11y; `UpdateAccessibility` + `AnnounceForAccessibility` commands. SceneModel snapshot replays per-node a11y so late-joining runtime-server clients see current state. AX action handlers cross the wire as reverse-channel `HandlerId`s — same trampoline mechanism as `on_click` / `on_change`. |

Documented gap (intentional): future `Role` variants surface as
`WireRole::Unknown` on older replayers → `None` (no role override).

The design follows the framework's two governing rules:

- **Rule 3 (core stays minimal)** — a11y types live in
  `runtime_shared::accessibility` and the capability surface is the
  smallest possible (`A11yOps`'s three methods plus a parameter on each
  `create_*`). Per-role authoring helpers (e.g. `aria-current="page"`
  shortcuts, named "landmark" components) belong outside core, on top of
  these primitives.
- **Rule 7 (backend determines how things render, implementations
  converge in behavior)** — one `Role` taxonomy, one `AccessibilityTraits`
  bitfield, one `AccessibilityProps` struct. Each backend maps to its
  native AX system. No `is_simulator()`-style per-platform branches in
  framework code; no per-platform fudge factors on the public surface.

---

## 1. `Role` taxonomy

A single `Role` enum with one variant per semantic role authors think
in. ~35 variants. Defined in `runtime_core::accessibility`.

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

The shape every `create_*` receives. Owned by `runtime_core::accessibility`.

```rust
#[derive(Clone, Debug, Default)]
pub struct AccessibilityProps {
    /// Author-supplied a11y name. `None` means "derive from the
    /// primitive's natural content" (Button's label, Text's content,
    /// Image's alt, etc.). The backend implements derivation; the
    /// framework doesn't pre-fill.
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
`Pressable` → `Role::Button`, `Text` → `Role::Text`, …). The resolved
role is `props.role.unwrap_or(default_role(kind))`, so an author override
always wins. The default-role table lives in
`runtime_shared::accessibility::default_role()` (re-exported as
`runtime_core::accessibility::default_role`) and is documented in the
primitive index.

**`actions: Vec<AccessibilityAction>` belongs in core.** Custom AX
actions are not a peripheral feature — VoiceOver / TalkBack expose them
as the rotor menu, and any moderately complex list ("delete row",
"archive row" without showing the buttons) needs them. Cost is one
`Vec` per primitive (typically empty); the indirection is a single
non-null check in the backend.

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

## 4. The backend surface

Every `create_*` capability method takes the node's props as
`a11y: &AccessibilityProps`, borrowed: the mount handler has the payload in
hand and lends it for the duration of the call. Each backend either ignores
it (Roku), maps it to per-attribute setter calls (no retention), or — where
it needs the props later (the GPU backend's semantics tree) — clones the parts
it needs into its own state. This matches how `&Rc<StyleRules>` flows through
`apply_style`.

The rest of the surface is `caps::A11yOps`, all three methods with no-op
defaults:

```rust
pub trait A11yOps: Host {
    /// Replace the accessibility prop bag on an existing node.
    /// `inferred_role` is the primitive's default role, so update
    /// resolves the same role as create.
    fn update_accessibility(&mut self, node: &Self::Node, a11y: &AccessibilityProps,
                            inferred_role: Option<Role>) {}

    /// Post a one-shot live-region announcement (no focus target).
    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {}

    /// GPU/canvas backends: snapshot the parallel semantics tree.
    /// Native widget backends return `None`.
    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> { None }
}
```

`announce_for_accessibility` maps to:

- iOS: `UIAccessibility.post(notification: .announcement, argument: msg)`
- Android: `View.announceForAccessibility(msg)` on the root view
- Web: `msg` written into a hidden `<div aria-live="polite|assertive">`
- macOS: `NSAccessibilityAnnouncementRequestedNotification` with an
  announcement key + priority

The author-side `runtime_core::announce(...)` reaches it through the
announcer the boot path installs (`crates/runtime/vocabulary/src/backend.rs`).

**The props are a field on each payload, not derived from other props.**
`AccessibilityProps` has fields (`hint`, `actions`, `identifier`, a custom
`role`) with no analog in the primitive-specific props; computing them
post-hoc would mean a parallel a11y-spec layer in core. Adding the field
directly is what every other prop system does (Flutter's
`SemanticsConfiguration`, React Native's `accessibilityProps`).

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

## 7. Open design questions

Open items:

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
   they're tied. Undecided; today they are separate.

3. **Live regions and the reactive system — composability.** When a
   `Status` primitive has `live_region: Some(Polite)` and the inner
   text reactively changes, the backend needs to be told both about
   the new text *and* that this constitutes a live-region update worth
   announcing. The naive design re-fires `update_accessibility` on
   every text change. That risks announcing every typo in a search
   field. Need a rule for "what change is announce-worthy" — likely
   "only when `AccessibilityProps.label` itself changes (computed or
   raw); not when the primitive's visible text changes." But that
   means the `update_text` path can't auto-derive the a11y label, or
   we re-announce on every keystroke. Unresolved.

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
