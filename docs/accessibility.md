# Accessibility

The framework carries one cross-platform accessibility (a11y) model and
each backend maps it to its platform's native AX system — UIAccessibility
on iOS, NSAccessibility on macOS, `AccessibilityNodeInfo` on Android,
ARIA on web, a parallel semantics tree on the wgpu/GPU backend, no-op on
Roku.

This page is the author-facing guide: what you get for free, the data
model, and how to override it. For the per-platform mapping tables, the
Backend-trait surface, and the design rationale, see
[`accessibility-design.md`](./accessibility-design.md).

> **Status.** Shipped end-to-end. Every primitive carries an
> `AccessibilityProps`, every native backend applies it, and the
> author-facing surface — granular `a11y_*` setters / `ui!` attributes, a
> whole-struct `accessibility` setter, and the `announce()` free function
> — is live on all primitives. See [Setting props](#setting-props).

---

## Accessible by default

Every primitive ships a default semantic role: `Button` → button,
`Text` → text, `Image` → image, `Slider` → slider, and so on
(`runtime_core::accessibility::default_role`). For standard controls,
the platform derives the spoken label from the control's visible content
— a button announces its title, a text node announces its string, an
image announces its `alt`. So the common case needs no author work: a
labeled button is already announced as a button with that label on
VoiceOver, TalkBack, and web screen readers.

You only reach for the model below when the visible shape and the a11y
intent diverge (a `Pressable` that's really a navigation link), when an
element carries state a screen reader should announce (selected,
disabled, expanded), or when content is decorative and should be hidden.

---

## The model

The per-element data is `runtime_core::accessibility::AccessibilityProps`.
Every field is optional; `AccessibilityProps::default()` means "infer
everything from the primitive."

| Field | Type | Purpose |
| --- | --- | --- |
| `label` | `Option<String>` | Spoken name. `None` lets the platform derive it from visible content. Setting it also opts the element into announce-on-change (see [Live regions](#live-regions)). |
| `hint` | `Option<String>` | Longer description ("Double tap to open menu", "Step 3 of 5"). |
| `role` | `Option<Role>` | Override the inferred role. `None` uses the primitive's default. |
| `traits` | `AccessibilityTraits` | Orthogonal state flags (see below). Empty by default. |
| `hidden` | `bool` | Remove from the a11y tree entirely. Use for decorative content. |
| `live_region` | `Option<LiveRegionPriority>` | Announce updates to this element. `None` = not a live region. |
| `actions` | `Vec<AccessibilityAction>` | Custom rotor / TalkBack actions. |
| `identifier` | `Option<String>` | Stable id for external AX tooling (XCUITest, UIAutomator, web `id`). Distinct from the Robot harness `test_id`. |

```rust
use runtime_core::accessibility::{AccessibilityProps, Role, AccessibilityTraits};

let props = AccessibilityProps {
    label: Some("Close dialog".to_string()),
    role: Some(Role::Button),
    traits: AccessibilityTraits::DISABLED,
    ..Default::default()
};
```

### Role

The role taxonomy (`Role`, `#[non_exhaustive]`) names a widget's
semantics independent of how it looks:

- **Structural** — `Button`, `Link`, `Image`, `Text`, `Header`, `List`,
  `ListItem`, `Group`, `Separator`.
- **Input** — `TextField`, `TextArea`, `Switch`, `Slider`, `Checkbox`,
  `RadioButton`, `RadioGroup`, `ComboBox`, `SearchField`.
- **Disclosure / navigation** — `Tab`, `TabList`, `TabPanel`,
  `NavigationLink`, `MenuItem`, `Menu`, `MenuBar`, `Toolbar`.
- **Feedback** — `Alert`, `Status`, `ProgressBar`, `Spinner`.
- **Container / overlay** — `Dialog`, `AlertDialog`, `Drawer`,
  `Popover`, `Tooltip`, `Region`.

Set `role` only when the visible primitive's shape differs from its a11y
intent. A `Pressable` styled as a nav link sets
`role: Some(Role::NavigationLink)`.

### Traits

`AccessibilityTraits` is a `u16` bitflag set, orthogonal to `Role` —
compose freely with `|`:

`SELECTED`, `DISABLED`, `EXPANDED`, `COLLAPSED`, `CHECKED`, `MIXED`
(tri-state), `BUSY`, `REQUIRED`, `READONLY`, `INVALID`,
`UPDATES_FREQUENTLY`.

```rust
let traits = AccessibilityTraits::SELECTED | AccessibilityTraits::EXPANDED;
```

Each flag maps to the platform's matching AX attribute; the observable
result is "the screen reader announces selected / disabled / expanded"
everywhere.

### Live regions

`LiveRegionPriority` controls how an update is announced:

- `Polite` — queue behind the user's current screen-reader speech. For
  non-critical status updates.
- `Assertive` — interrupt and announce now. For genuine alerts
  (submission failures, error toasts). Use sparingly.

Setting `live_region` plus an explicit `label` opts the element into
announce-on-change: when a reactive update changes the `label`, the
backend re-announces at the chosen priority. Visible-text changes do not
auto-announce — you opt in with an explicit label.

### Custom actions

`AccessibilityAction { name, handler }` exposes an action to assistive
technology without a visible control: a rotor entry on VoiceOver, a
TalkBack action in the per-element menu, a custom-widget dispatch on web.
The handler runs on the reactive thread like a touch handler, so it can
update signals synchronously. Common uses: per-row "Delete" / "Archive",
per-card "Copy link", per-message "Reply".

```rust
use std::rc::Rc;
use runtime_core::accessibility::AccessibilityAction;

let archive = AccessibilityAction {
    name: "Archive".to_string(),
    handler: Rc::new(move || archived.set(true)),
};
```

---

## Setting props

Every primitive exposes the same a11y surface — both as builder methods
on the value a constructor returns and as attributes inside `ui!` / `jsx!`.

### Granular setters — the common case

Each maps to one `AccessibilityProps` field. As `ui!` attributes (the
named-prop form) and as builder methods, the names are identical:

| Setter / attribute | Sets | Accepts |
| --- | --- | --- |
| `a11y_label` | `label` | `impl Into<String>` |
| `a11y_hint` | `hint` | `impl Into<String>` |
| `a11y_role` | `role` | `Role` |
| `a11y_hidden` | `hidden` | `bool` |
| `a11y_traits` | `traits` | `AccessibilityTraits` |
| `live_region` | `live_region` | `LiveRegionPriority` |

```rust
use runtime_core::{ui, Role};

// `ui!` named-prop form — attributes live inside the parens:
ui! {
    button(
        label = "Save",
        on_click = on_save,
        a11y_label = "Save document",
        a11y_hint = "Writes changes to disk",
        a11y_role = Role::Button,
    )
}
```

```rust
// Builder-chain form — on the value any constructor returns. Use this
// with the positional primitive form (`button("Save", cb)`), which is a
// plain expression rather than a `ui!` tag:
view(children)
    .a11y_role(Role::Toolbar)
    .a11y_hidden(false)
```

### Whole-struct setter — `accessibility`

When you need to set several fields at once, or attach custom `actions`,
pass a full `AccessibilityProps`. Available as a builder method and as a
`ui!` attribute:

```rust
use runtime_core::{ui, AccessibilityProps, Role};

ui! {
    view(accessibility = AccessibilityProps {
        label: Some("Toolbar".into()),
        role: Some(Role::Toolbar),
        ..Default::default()
    })
}
```

The `LazyBuilder` container exposes the same setters (plus its original
`with_accessibility(props)`).

### Announcements — `announce`

For transient feedback with no focus target ("Saved", "Form submitted"),
call `runtime_core::announce` from any event handler or effect — no
`Backend` reference needed:

```rust
use runtime_core::{announce, LiveRegionPriority};

announce("Saved", LiveRegionPriority::Polite);
```

It routes to the active backend's announcer (web `aria-live`, iOS/macOS
AX post, Android `announceForAccessibility`, GPU pending-announcement
queue). On backends with no AX subsystem (terminal, CPU, Roku) it is a
silent no-op, as it is before the app has mounted.

---

## Per-platform realization

| Backend | Maps `AccessibilityProps` to |
| --- | --- |
| iOS | `UIAccessibility` (label, hint, traits, custom actions) |
| macOS | `NSAccessibility` protocol attributes |
| Android | `AccessibilityNodeInfo` (contentDescription, state, actions) |
| Web | ARIA (`role`, `aria-*`, `aria-live`) |
| wgpu / GPU | a parallel semantics tree the host projects into platform AX |
| Roku | no-op |

The exact attribute-by-attribute tables live in
[`accessibility-design.md`](./accessibility-design.md) §1–§6.

---

## See also

- [`accessibility-design.md`](./accessibility-design.md) — internals:
  the `Role`/trait mapping tables, Backend-trait signatures, the
  GPU-backend semantics tree, and open design questions.
- The **Accessibility** track in the tutorial app — the same material,
  hands-on.
