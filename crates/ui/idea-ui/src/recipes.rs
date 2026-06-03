//! Compile-checked usage **recipes** for idea-ui components.
//!
//! Each `recipe!(Component, fn ...)` is a real, type-checked example of
//! how to use a component. Because the fn is compiled against the
//! component's live props, a prop change that isn't reflected here is a
//! compile error (whenever the catalog is built) — so these examples
//! can't silently rot, and the MCP/docs surface them as trustworthy
//! "how do I use this?" context.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) these expand to nothing — the recipes, and the
//! imports inside them, don't compile at all. So there's no `#[cfg]`
//! here and no cost in shipped apps.
//!
//! Recipes are self-contained — imports live inside each fn — so the
//! captured `source` reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    Button,
    /// A primary action button that runs a callback when pressed. The
    /// default `tone`/`variant`/`size`/`shape` give a filled primary
    /// button; pass them explicitly to vary it.
    pub fn button_basic() -> ::runtime_core::Element {
        use crate::Button;
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let on_click: Rc<dyn Fn()> = Rc::new(|| {
            // handle the press
        });
        ui! {
            Button(label = "Save", on_click = on_click)
        }
    }
);

recipe!(
    Select,
    /// A controlled dropdown. The host owns the `value` signal (the
    /// chosen option's `id`); `on_change` writes the picked id back into
    /// it. Build the rows with `SelectOption::new(id, label)`.
    pub fn select_controlled() -> ::runtime_core::Element {
        use crate::{Select, SelectOption};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let value = signal!("pear".to_string());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));
        ui! {
            Select(
                value = value,
                on_change = on_change,
                options = vec![
                    SelectOption::new("apple", "Apple"),
                    SelectOption::new("pear", "Pear"),
                    SelectOption::new("banana", "Banana"),
                ],
                placeholder = Some("Choose a fruit".to_string()),
            )
        }
    }
);

recipe!(
    Field,
    /// A labeled, controlled text input. The host owns the `value`
    /// signal; `on_change` fires the new text on each edit. Add `help`
    /// for hint text or `error = Some(...)` to flag a validation problem
    /// (which paints the input in the Danger tone automatically).
    pub fn field_controlled() -> ::runtime_core::Element {
        use crate::Field;
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let email = signal!(String::new());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| email.set(v));
        ui! {
            Field(
                label = Some("Email".to_string()),
                value = email,
                on_change = on_change,
                placeholder = Some("you@example.com".to_string()),
                help = Some("We'll never share your email.".to_string()),
            )
        }
    }
);

recipe!(
    Checkbox,
    /// A controlled checkbox with a label. The host owns the `value:
    /// Signal<bool>`; `on_change` fires the toggled value. Tapping
    /// anywhere on the row (box or label) toggles it.
    pub fn checkbox_controlled() -> ::runtime_core::Element {
        use crate::{tone, Checkbox};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let agreed = signal!(false);
        let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| agreed.set(v));
        ui! {
            Checkbox(
                label = Some("I agree to the terms".to_string()),
                value = agreed,
                on_change = on_change,
                tone = tone::Primary,
            )
        }
    }
);

recipe!(
    Switch,
    /// A controlled slide-toggle with an inline label. The host owns the
    /// `value: Signal<bool>`; `on_change` fires the flipped value. Use a
    /// semantic `tone` (e.g. Success) to color the "on" track.
    pub fn switch_controlled() -> ::runtime_core::Element {
        use crate::{tone, Switch};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let enabled = signal!(true);
        let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| enabled.set(v));
        ui! {
            Switch(
                label = Some("Notifications".to_string()),
                value = enabled,
                on_change = on_change,
                tone = tone::Success,
            )
        }
    }
);

recipe!(
    Card,
    /// A surface container that wraps its children in a themed, rounded,
    /// bordered panel. Use `variant = card::variant::Elevated` for a
    /// raised look (surface-alt background + shadow); `padding` sets the
    /// inner spacing.
    pub fn card_elevated() -> ::runtime_core::Element {
        use crate::components::card::variant;
        use crate::{typography_kind, Card, CardPadding, Typography};
        use ::runtime_core::ui;

        ui! {
            Card(variant = variant::Elevated, padding = CardPadding::Md) {
                Typography(content = "Monthly stats", kind = typography_kind::H2)
                Typography(content = "Up 12% from last month.", muted = true)
            }
        }
    }
);

recipe!(
    Modal,
    /// A centered overlay with a dimming backdrop and a themed surface.
    /// idea-ui's Modal does NOT auto-unmount — the host gates it behind
    /// an open-state signal (`if open.get() { Modal { .. } }`) and flips
    /// that signal in `on_dismiss`.
    pub fn modal_confirm() -> ::runtime_core::Element {
        use crate::{typography_kind, Modal, Typography};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let open = signal!(true);
        let on_dismiss: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        ui! {
            if open.get() {
                Modal(on_dismiss = Some(on_dismiss.clone())) {
                    Typography(content = "Confirm", kind = typography_kind::H2)
                    Typography(content = "Are you sure you want to continue?")
                }
            }
        }
    }
);

recipe!(
    Tabs,
    /// A clickable tab strip. Tabs is pure UI: the host owns the active
    /// index (`Signal<usize>`) and renders the active tab's content
    /// itself (e.g. with a `match` on `active.get()`). Position in the
    /// `tabs` vec is each tab's index.
    pub fn tabs_controlled() -> ::runtime_core::Element {
        use crate::{Tab, Tabs};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let active = signal!(0_usize);
        let on_change: Rc<dyn Fn(usize)> = Rc::new(move |idx| active.set(idx));
        ui! {
            Tabs(
                tabs = vec![
                    Tab::new("Overview"),
                    Tab::new("Activity"),
                    Tab::new("Settings"),
                ],
                active = active,
                on_change = on_change,
            )
            // ... host renders content driven by `active.get()` here ...
        }
    }
);

recipe!(
    Table,
    /// A themed data table: a header row (cells with `header = true`)
    /// plus body rows. Use `TableCell(header = true, text = "...")` for
    /// the simple text case; pass a `children` block for richer cell
    /// content.
    pub fn table_basic() -> ::runtime_core::Element {
        use crate::{Table, TableCell, TableRow};
        use ::runtime_core::ui;

        ui! {
            Table {
                TableRow {
                    TableCell(header = true, text = Some("Name".to_string()))
                    TableCell(header = true, text = Some("Role".to_string()))
                }
                TableRow {
                    TableCell(text = Some("Ada".to_string()))
                    TableCell(text = Some("Engineer".to_string()))
                }
                TableRow {
                    TableCell(text = Some("Grace".to_string()))
                    TableCell(text = Some("Admiral".to_string()))
                }
            }
        }
    }
);

recipe!(
    Typography,
    /// The standard way to put themed text on screen. `kind` picks the
    /// type role (H1…H6, Body, Caption, …) from the theme's scale; set
    /// `muted = true` for secondary text or `tone = Some(...)` for
    /// intent-colored text.
    pub fn typography_heading() -> ::runtime_core::Element {
        use crate::{typography_kind, Typography};
        use ::runtime_core::ui;

        ui! {
            Typography(content = "Welcome back", kind = typography_kind::H1)
        }
    }
);

recipe!(
    Alert,
    /// A banner with a title, optional body line, and an optional
    /// dismiss button. Pick a semantic `tone` (Info/Success/Warning/
    /// Danger) and a `variant` (Soft/Filled/Outline). Provide
    /// `on_dismiss = Some(...)` to show the close affordance.
    pub fn alert_dismissible() -> ::runtime_core::Element {
        use crate::{tone, variant, Alert};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let on_dismiss: Rc<dyn Fn()> = Rc::new(|| { /* hide the alert */ });
        ui! {
            Alert(
                title = "Couldn't save",
                body = Some("The server returned 503.".to_string()),
                tone = tone::Danger,
                variant = variant::Soft,
                on_dismiss = Some(on_dismiss),
            )
        }
    }
);

recipe!(
    Menu,
    /// An anchored command panel. Anchor it to a trigger via a
    /// `Ref<PressableHandle>` (`bind_to` on the Button, `target =
    /// AnchorTarget::from(trigger)` on the Menu) and gate it behind an
    /// open-state signal. Compose `MenuItem`/`MenuLabel`/`MenuSeparator`
    /// children; flip the signal in each `on_select` and `on_dismiss`.
    pub fn menu_anchored() -> ::runtime_core::Element {
        use crate::{Button, Menu, MenuItem, MenuLabel, MenuSeparator};
        use ::runtime_core::primitives::portal::AnchorTarget;
        use ::runtime_core::{signal, ui, PressableHandle, Ref};
        use ::std::rc::Rc;

        let trigger: Ref<PressableHandle> = Ref::new();
        let open = signal!(false);
        let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
        // A single `close` callback, cloned at each use site — the
        // reactive `if open.get()` branch is an `Fn` closure, so any
        // `Rc` it uses must be cloned (not moved) into it.
        let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        ui! {
            view {
                Button(
                    label = "Actions",
                    on_click = on_open,
                    bind_to = Some(trigger),
                )
                if open.get() {
                    Menu(
                        target = Some(AnchorTarget::from(trigger)),
                        on_dismiss = Some(close.clone()),
                    ) {
                        MenuLabel(text = "Edit")
                        MenuItem(label = "Rename", on_select = close.clone())
                        MenuSeparator()
                        MenuItem(label = "Delete", on_select = close.clone())
                    }
                }
            }
        }
    }
);

recipe!(
    IconButton,
    /// A square, single-glyph clickable. Pick a `tone` × `variant` ×
    /// `size`; `glyph` is the character drawn inside (e.g. `"×"` for a
    /// close button). `on_click` fires on press.
    pub fn icon_button_close() -> ::runtime_core::Element {
        use crate::{tone, variant, IconButton, IconButtonSize};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let on_click: Rc<dyn Fn()> = Rc::new(|| { /* dismiss */ });
        ui! {
            IconButton(
                glyph = "×",
                on_click = on_click,
                tone = tone::Neutral,
                variant = variant::Ghost,
                size = IconButtonSize::Md,
            )
        }
    }
);
