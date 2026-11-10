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
    Button,
    /// A full-width call-to-action with a leading icon. `block = true`
    /// stretches the button to its container's width; `leading_icon` /
    /// `trailing_icon` take an `IconData` constant (from an icon pack
    /// like `icons_lucide`) and render it inline beside the label,
    /// inheriting the button's text color.
    pub fn button_icon_block() -> ::runtime_core::Element {
        use crate::Button;
        use ::runtime_core::{ui, FillRule, IconData};
        use ::std::rc::Rc;

        // In real code this is a pack constant, e.g. `icons_lucide::PLUS`.
        const PLUS: IconData = IconData {
            view_box: (24, 24),
            paths: &["M12 5v14M5 12h14"],
            fill_rule: FillRule::NonZero,
            filled: false,
        };
        let on_click: Rc<dyn Fn()> = Rc::new(|| { /* create */ });
        ui! {
            Button(
                label = "New project",
                on_click = on_click,
                leading_icon = Some(PLUS),
                block = true,
            )
        }
    }
);

recipe!(
    Icon,
    /// A sized, optionally tinted vector icon. `data` is an `IconData`
    /// constant (from an icon pack like `icons_lucide`); `size` sets the
    /// square in points. Pass `tone = Some(...)` to paint it in a
    /// semantic intent color, or `color = Some(...)` for an explicit one
    /// — with neither, it inherits the ambient text color.
    pub fn icon_tinted() -> ::runtime_core::Element {
        use crate::components::icon::Icon;
        use crate::tone;
        use ::runtime_core::{ui, FillRule, IconData};

        // In real code this is a pack constant, e.g. `icons_lucide::HEART`.
        const HEART: IconData = IconData {
            view_box: (24, 24),
            paths: &["M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1a5.5 5.5 0 1 0-7.8 7.8l1 1L12 21l7.8-7.5 1-1a5.5 5.5 0 0 0 0-7.9z"],
            fill_rule: FillRule::NonZero,
            filled: true,
        };
        ui! {
            Icon(data = HEART, size = 24.0, tone = tone::Danger)
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

        let value = signal("pear".to_string());
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
    Select,
    /// Rows with a muted second line: chain `.with_description(text)` on a
    /// `SelectOption` to draw a hint (or a note on what picking the row
    /// does) under its label in the menu. The trigger keeps showing only
    /// the label; `Autocomplete` takes the same rows and also filters on
    /// the description.
    pub fn select_option_descriptions() -> ::runtime_core::Element {
        use crate::{Select, SelectOption};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let value = signal(String::new());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));
        ui! {
            Select(
                value = value,
                on_change = on_change,
                options = vec![
                    SelectOption::new("form_entry.address", "Address")
                        .with_description("as its one-line label"),
                    SelectOption::new("form_entry.address_lines", "Address (multi-line)")
                        .with_description("one line per address part"),
                ],
                placeholder = Some("Insert a field".to_string()),
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

        let email = signal(String::new());
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

        let agreed = signal(false);
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

        let enabled = signal(true);
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
    /// raised look (surface-alt background + shadow), `Outline` for the
    /// border alone, or `Flat` for a borderless surface-alt panel;
    /// `padding` sets the inner spacing.
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
    Card,
    /// An intent-tinted card. Setting `tone = Some(...)` paints the card
    /// with a muted tone background + matching border (the Soft tint Alert
    /// uses) — for support/crisis/info panels that need to read as
    /// intent-colored. Works with either variant.
    pub fn card_toned() -> ::runtime_core::Element {
        use crate::{tone, typography_kind, Card, CardPadding, Typography};
        use ::runtime_core::ui;

        ui! {
            Card(padding = CardPadding::Md, tone = tone::Danger) {
                Typography(content = "Account at risk", kind = typography_kind::H3)
                Typography(content = "Verify your email to avoid suspension.")
            }
        }
    }
);

recipe!(
    Modal,
    /// A centered overlay with a dimming backdrop and a themed surface. The
    /// host owns an open-state `Signal<bool>` and passes it as `open` — the
    /// Modal is ALWAYS mounted (no `if open { .. }` gate); flipping `open`
    /// false animates the exit then unmounts. `content` is a closure (rebuilt
    /// on each open); flip the signal in `on_dismiss`.
    pub fn modal_confirm() -> ::runtime_core::Element {
        use crate::{typography_kind, Modal, Typography};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let open = signal(true);
        let on_dismiss: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        ui! {
            Modal(
                open = open,
                on_dismiss = Some(on_dismiss.clone()),
                content = move || ui! {
                    Typography(content = "Confirm", kind = typography_kind::H2)
                    Typography(content = "Are you sure you want to continue?")
                },
            )
        }
    }
);

recipe!(
    Modal,
    /// A command palette: `presentation = ModalPresentation::Top` pins the
    /// card's TOP edge below the top of the safe area (`top_offset`, default
    /// 15% of the safe height), so the card grows downward as results
    /// filter and the search field never moves under the caret. A
    /// `Centered` modal would re-center on every height change.
    pub fn modal_command_palette() -> ::runtime_core::Element {
        use crate::{Field, Modal, ModalPresentation, Stack, StackGap, Typography};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let open = signal(true);
        let query = signal(String::new());
        let on_dismiss: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        ui! {
            Modal(
                open = open,
                presentation = ModalPresentation::Top,
                on_dismiss = Some(on_dismiss.clone()),
                content = move || {
                    let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| query.set(v));
                    ui! {
                        Stack(gap = StackGap::Sm) {
                            Field(
                                value = query,
                                on_change = on_change,
                                placeholder = Some("Search commands…".to_string()),
                            )
                            Typography(content = "Open file")
                            Typography(content = "Go to line", muted = true)
                        }
                    }
                },
            )
        }
    }
);

recipe!(
    Modal,
    /// A long form whose title and actions never scroll away: `header` and
    /// `footer` render OUTSIDE the modal's scroller, pinned to the top and
    /// bottom of the surface. Once the content is taller than the modal's
    /// height cap, only `content` scrolls. Both are closures like `content`
    /// (rebuilt on each open).
    pub fn modal_form_with_footer() -> ::runtime_core::Element {
        use crate::{
            tone, typography_kind, variant, Button, Field, Modal, Stack, StackAxis, StackGap,
            StackJustify, Typography,
        };
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let open = signal(true);
        let name = signal(String::new());
        let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        let on_cancel = on_close.clone();
        let on_save = on_close.clone();
        ui! {
            Modal(
                open = open,
                on_dismiss = Some(on_close.clone()),
                header = move || ui! {
                    Typography(content = "Edit crew member", kind = typography_kind::H3)
                },
                content = move || {
                    let on_name: Rc<dyn Fn(String)> = Rc::new(move |v| name.set(v));
                    ui! {
                        Stack(gap = StackGap::Md) {
                            Field(value = name, on_change = on_name, placeholder = Some("Name".to_string()))
                            // …as many fields as the form needs; only this part scrolls.
                        }
                    }
                },
                footer = move || {
                    let on_cancel = on_cancel.clone();
                    let on_save = on_save.clone();
                    ui! {
                        Stack(axis = StackAxis::Row, justify = StackJustify::End, gap = StackGap::Sm) {
                            Button(label = "Cancel", on_click = on_cancel, tone = tone::Neutral, variant = variant::Soft)
                            Button(label = "Save", on_click = on_save, tone = tone::Primary, variant = variant::Filled)
                        }
                    }
                },
            )
        }
    }
);

recipe!(
    Tabs,
    /// A clickable tab strip. Tabs is pure UI: the host owns the active tab's
    /// `id` (a `Signal<String>`) and renders that tab's content itself (e.g. a
    /// `match` on `active.get()`). `tabs` is a `Signal<Vec<Tab>>` (a reactive,
    /// id-keyed list — wrap a fixed set in `signal!`); each `Tab::new(id, label)`
    /// carries its own identity.
    pub fn tabs_controlled() -> ::runtime_core::Element {
        use crate::{Tab, Tabs};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let active = signal("overview".to_string());
        let tabs = signal(vec![
            Tab::new("overview", "Overview"),
            Tab::new("activity", "Activity"),
            Tab::new("settings", "Settings"),
        ]);
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| active.set(id));
        ui! {
            Tabs(
                tabs = tabs,
                active = active,
                on_change = on_change,
            )
            // ... host renders content driven by `active.get()` here ...
        }
    }
);

// `Table` is behind the `table` feature (it pulls the table SDK), and
// `recipe!` only self-gates on `catalog`. Without this gate a build with
// `catalog` on and `table` off — `default-features = false` in a consumer
// that runs `idealyst dev`, which enables the catalog — failed to compile
// idea-ui at all (`unresolved imports crate::Table`).
#[cfg(feature = "table")]
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
    /// A banner with a title, optional body line, an optional trailing
    /// `action` slot, and a configurable `close`. Pick a semantic `tone`
    /// (Info/Success/Warning/Danger) and a `variant` (Soft/Filled/
    /// Outline). `close = AlertClose::Button(handler)` shows the standard
    /// ×; `AlertClose::Custom(element)` supplies your own; the default
    /// `AlertClose::None` shows nothing.
    pub fn alert_with_action() -> ::runtime_core::Element {
        use crate::{tone, variant, Alert, AlertClose, Button};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let retry: Rc<dyn Fn()> = Rc::new(|| { /* retry the request */ });
        let on_dismiss: Rc<dyn Fn()> = Rc::new(|| { /* hide the alert */ });
        ui! {
            Alert(
                title = "Couldn't save",
                body = Some("The server returned 503.".to_string()),
                tone = tone::Danger,
                variant = variant::Soft,
                action = Some(ui! { Button(label = "Retry", on_click = retry) }),
                close = AlertClose::Button(on_dismiss),
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
        use crate::AnchorTarget;
        use ::runtime_core::{signal, ui, PressableHandle, Ref};
        use ::std::rc::Rc;

        let trigger: Ref<PressableHandle> = Ref::new();
        let open = signal(false);
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

recipe!(
    IconButton,
    /// A square icon button rendering a vector (Lucide) icon rather than
    /// a text glyph. Pass `icon = Some(IconData)` and it takes precedence
    /// over `glyph`, tinting to match the tone × variant.
    pub fn icon_button_vector() -> ::runtime_core::Element {
        use crate::{tone, variant, IconButton, IconButtonSize};
        use ::runtime_core::{ui, FillRule, IconData};
        use ::std::rc::Rc;

        // In real code this is a pack constant, e.g. `icons_lucide::TRASH`.
        const TRASH: IconData = IconData {
            view_box: (24, 24),
            paths: &["M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6"],
            fill_rule: FillRule::NonZero,
            filled: false,
        };
        let on_click: Rc<dyn Fn()> = Rc::new(|| { /* delete */ });
        ui! {
            IconButton(
                icon = Some(TRASH),
                on_click = on_click,
                tone = tone::Danger,
                variant = variant::Soft,
                size = IconButtonSize::Md,
            )
        }
    }
);

// ---------------------------------------------------------------------
// Layout & containers
// ---------------------------------------------------------------------

recipe!(
    Stack,
    /// The everyday vertical layout: stacks its children in a column with
    /// a uniform `gap`. Switch to a horizontal row with `axis =
    /// StackAxis::Row`; `align`/`justify` control cross- and main-axis
    /// placement.
    pub fn stack_layout() -> ::runtime_core::Element {
        use crate::{typography_kind, Stack, StackGap, Typography};
        use ::runtime_core::ui;

        ui! {
            Stack(gap = StackGap::Md) {
                Typography(content = "Profile", kind = typography_kind::H3)
                Typography(content = "Manage your account details.", muted = true)
                Typography(content = "Last updated just now.")
            }
        }
    }
);

recipe!(
    Center,
    /// Centers its children on both axes inside the space it's given. Drop
    /// any single child (or a Stack) inside and it sits dead center —
    /// handy for empty states and loading screens.
    pub fn center_content() -> ::runtime_core::Element {
        use crate::{typography_kind, Center, Typography};
        use ::runtime_core::ui;

        ui! {
            Center {
                Typography(content = "Nothing here yet", kind = typography_kind::H2)
            }
        }
    }
);

recipe!(
    Grid,
    /// A fixed-column grid. `columns` sets how many equal-width tracks
    /// each row has; `gap` spaces both rows and columns. Children flow
    /// left-to-right, wrapping to a new row every `columns` items.
    pub fn grid_columns() -> ::runtime_core::Element {
        use crate::{typography_kind, Card, CardPadding, Grid, StackGap, Typography};
        use ::runtime_core::ui;

        ui! {
            Grid(columns = 3u32, gap = StackGap::Md) {
                Card(padding = CardPadding::Md) { Typography(content = "One", kind = typography_kind::H3) }
                Card(padding = CardPadding::Md) { Typography(content = "Two", kind = typography_kind::H3) }
                Card(padding = CardPadding::Md) { Typography(content = "Three", kind = typography_kind::H3) }
            }
        }
    }
);

recipe!(
    Divider,
    /// A hairline rule separating content. Defaults to a horizontal line
    /// that fills its parent's width; pass `axis = DividerAxis::Vertical`
    /// for a vertical rule inside a row.
    pub fn divider_separator() -> ::runtime_core::Element {
        use crate::{Divider, Stack, StackGap, Typography};
        use ::runtime_core::ui;

        ui! {
            Stack(gap = StackGap::Md) {
                Typography(content = "Account")
                Divider()
                Typography(content = "Danger zone")
            }
        }
    }
);

recipe!(
    Spacer,
    /// A flexible gap that pushes its siblings apart. In a row it expands
    /// to fill the free space, shoving the items on either side to the
    /// edges — the standard "title on the left, actions on the right"
    /// toolbar trick.
    pub fn spacer_gap() -> ::runtime_core::Element {
        use crate::{tone, variant, Spacer, Stack, StackAxis, StackAlign, Tag, Typography};
        use ::runtime_core::ui;

        ui! {
            Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                Typography(content = "Inbox")
                Spacer()
                Tag(label = "12 new", tone = tone::Primary, variant = variant::Soft)
            }
        }
    }
);

// ---------------------------------------------------------------------
// Display & status
// ---------------------------------------------------------------------

recipe!(
    Avatar,
    /// A round user chip. Pass `src` for a photo, or `initials` to render
    /// a colored monogram when there's no image. `tone` picks the color —
    /// a palette hue (`tone::Violet`), or `tone::palette_from_seed(..)` for
    /// a stable per-user color — `variant` the fill (Soft by default,
    /// `variant::Filled` for a solid disc), and `size` scales the circle.
    pub fn avatar_initials() -> ::runtime_core::Element {
        use crate::{tone, variant, Avatar, AvatarSize};
        use ::runtime_core::ui;

        ui! {
            Avatar(initials = "AL", tone = tone::palette_from_seed("ada@lovelace.dev"), size = AvatarSize::Lg)
            Avatar(initials = "GH", tone = tone::Teal, variant = variant::Filled, size = AvatarSize::Lg)
        }
    }
);

recipe!(
    Badge,
    /// A small status pill for counts and labels. Pick a semantic `tone`
    /// (Primary/Success/Danger/…) and a `variant` (Soft/Filled/Outline).
    /// `label` is reactive, so it can be driven by a signal.
    pub fn badge_status() -> ::runtime_core::Element {
        use crate::{tone, variant, Badge};
        use ::runtime_core::ui;

        ui! {
            Badge(label = "New", tone = tone::Primary, variant = variant::Soft)
        }
    }
);

recipe!(
    Tag,
    /// A pill label, optionally removable. Provide `on_remove = Some(...)`
    /// to show a close affordance (e.g. for filter chips); omit it for a
    /// static tag. `tone` × `variant` set the palette.
    pub fn tag_removable() -> ::runtime_core::Element {
        use crate::{tone, variant, Tag};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let on_remove: Rc<dyn Fn()> = Rc::new(|| { /* drop the filter */ });
        ui! {
            Tag(
                label = "Rust",
                tone = tone::Primary,
                variant = variant::Soft,
                on_remove = Some(on_remove),
            )
        }
    }
);

recipe!(
    Progress,
    /// A horizontal progress bar. Set `value` in 0.0..=1.0 for a
    /// value-driven bar (changes animate to the new width), or pick a
    /// `mode`: `Indeterminate` sweeps endlessly when you can't measure
    /// progress; `Simulated` creeps toward full like a fake page
    /// loader. `value` is reactive.
    pub fn progress_bar() -> ::runtime_core::Element {
        use crate::{tone, Progress, ProgressMode, Stack, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(gap = StackGap::Md) {
                Progress(value = 0.65f32, tone = tone::Primary)
                Progress(mode = ProgressMode::Indeterminate, tone = tone::Info)
                Progress(mode = ProgressMode::Simulated, tone = tone::Success)
            }
        }
    }
);

recipe!(
    Slider,
    /// A controlled horizontal value slider. The host owns
    /// `value: Signal<f32>`; `on_change` fires the new value during the
    /// drag. `min`/`max`/`step` bound and quantize it; `tone` colors the
    /// fill + thumb. Keep a fixed `width` and don't rebuild the Slider
    /// mid-drag (see its docs).
    pub fn slider_controlled() -> ::runtime_core::Element {
        use crate::{tone, Slider};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let level = signal(0.5f32);
        let on_change: Rc<dyn Fn(f32)> = Rc::new(move |v| level.set(v));
        ui! {
            Slider(
                value = level,
                on_change = on_change,
                tone = tone::Primary,
            )
        }
    }
);

recipe!(
    Spinner,
    /// A spinning loading indicator for indeterminate waits. `size` picks
    /// `Small` or `Large`. Pair it with a label or center it in the area
    /// that's loading.
    pub fn spinner_loading() -> ::runtime_core::Element {
        use crate::{Spinner, SpinnerSize};
        use ::runtime_core::ui;

        ui! {
            Spinner(size = SpinnerSize::Large)
        }
    }
);

recipe!(
    Skeleton,
    /// Placeholder shimmer blocks shown while content loads. Stack a few
    /// with varied `width`s (Full/ThreeQuarter/Half or `Px`) to suggest
    /// the shape of the incoming content; `height` sets each block's
    /// thickness.
    pub fn skeleton_placeholder() -> ::runtime_core::Element {
        use crate::{Skeleton, SkeletonWidth, Stack, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(gap = StackGap::Sm) {
                Skeleton(width = SkeletonWidth::Full, height = 16.0)
                Skeleton(width = SkeletonWidth::ThreeQuarter, height = 16.0)
                Skeleton(width = SkeletonWidth::Half, height = 16.0)
            }
        }
    }
);

recipe!(
    Image,
    /// A bitmap image. `src` is the URL/path; `alt` is the accessible
    /// description. Constrain it with `width`/`height` (points) and set
    /// `rounded = true` for rounded corners (e.g. thumbnails).
    pub fn image_rounded() -> ::runtime_core::Element {
        use crate::Image;
        use ::runtime_core::ui;

        ui! {
            Image(
                src = "https://picsum.photos/200",
                alt = Some("A random landscape".to_string()),
                width = Some(160.0_f32),
                height = Some(160.0_f32),
                rounded = true,
            )
        }
    }
);

recipe!(
    Link,
    /// An inline hyperlink to an external URL. `label` is the visible
    /// text; `url` is the destination. For in-app navigation between
    /// screens, use the framework's `link` primitive with a typed route
    /// instead.
    pub fn link_external() -> ::runtime_core::Element {
        use crate::Link;
        use ::runtime_core::ui;

        ui! {
            Link(label = "Idealyst docs", url = "https://idealyst.dev")
        }
    }
);

// ---------------------------------------------------------------------
// Lists, navigation & paging
// ---------------------------------------------------------------------

recipe!(
    List,
    /// A vertical list of rows. Compose `ListItem`s inside it; each row
    /// takes a `label`, an optional `on_press`, and optional
    /// `leading`/`trailing` slots for icons or controls.
    pub fn list_items() -> ::runtime_core::Element {
        use crate::{List, ListItem};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let on_press: Rc<dyn Fn()> = Rc::new(|| { /* navigate */ });
        ui! {
            List {
                ListItem(label = "Profile", on_press = Some(on_press.clone()))
                ListItem(label = "Billing", on_press = Some(on_press.clone()))
                ListItem(label = "Sign out", on_press = Some(on_press))
            }
        }
    }
);

recipe!(
    Breadcrumbs,
    /// A navigation trail. Build it from `Crumb`s — `Crumb::linked(label,
    /// on_press)` for clickable ancestors and `Crumb::new(label)` for the
    /// current (non-clickable) page. The `separator` between them is
    /// configurable.
    pub fn breadcrumbs_trail() -> ::runtime_core::Element {
        use crate::{Breadcrumbs, Crumb};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let go_home: Rc<dyn Fn()> = Rc::new(|| { /* navigate home */ });
        let go_library: Rc<dyn Fn()> = Rc::new(|| { /* navigate to library */ });
        ui! {
            Breadcrumbs(
                items = vec![
                    Crumb::linked("Home", go_home),
                    Crumb::linked("Library", go_library),
                    Crumb::new("Button"),
                ],
            )
        }
    }
);

recipe!(
    Pagination,
    /// A page selector. The host owns the current `page` (zero-based)
    /// `Signal<usize>`; `total` is the page count; `on_change` fires the
    /// newly chosen page so the host can refetch and update the signal.
    pub fn pagination_pager() -> ::runtime_core::Element {
        use crate::Pagination;
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let page = signal(0_usize);
        let on_change: Rc<dyn Fn(usize)> = Rc::new(move |p| page.set(p));
        ui! {
            Pagination(page = page, total = 20_usize, on_change = on_change)
        }
    }
);

// ---------------------------------------------------------------------
// Disclosure
// ---------------------------------------------------------------------

recipe!(
    Collapsible,
    /// A titled section that expands and collapses. The host owns the
    /// open-state `Signal<bool>`; `on_change` fires the toggled value.
    /// Children are revealed when open; the default `Measured` transition
    /// animates to the body's natural height.
    pub fn collapsible_section() -> ::runtime_core::Element {
        use crate::{Collapsible, Stack, StackGap, Typography};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let open = signal(false);
        let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| open.set(v));
        ui! {
            Collapsible(title = "Advanced settings", value = open, on_change = on_change) {
                Stack(gap = StackGap::Sm) {
                    Typography(content = "These options are hidden until you expand.")
                    Typography(content = "Tweak with care.", muted = true)
                }
            }
        }
    }
);

recipe!(
    Accordion,
    /// A set of collapsible items. `expand = AccordionExpand::Single`
    /// keeps at most one open at a time (Multi allows any subset). The
    /// host owns `open: Signal<Vec<bool>>` (one bool per item); the
    /// Accordion writes to it on click. Each `AccordionItem` carries a
    /// `title` and an `Element` `body`.
    pub fn accordion_single() -> ::runtime_core::Element {
        use crate::{AccordionExpand, AccordionItem, Accordion, Typography};
        use ::runtime_core::{signal, ui};

        let open = signal(vec![true, false, false]);
        ui! {
            Accordion(
                expand = AccordionExpand::Single,
                open = open,
                items = vec![
                    AccordionItem {
                        title: "Shipping".into(),
                        body: ui! { Typography(content = "Ships within 2 business days.") },
                    },
                    AccordionItem {
                        title: "Returns".into(),
                        body: ui! { Typography(content = "Free returns within 30 days.") },
                    },
                    AccordionItem {
                        title: "Support".into(),
                        body: ui! { Typography(content = "Chat with us 24/7.") },
                    },
                ],
            )
        }
    }
);

// ---------------------------------------------------------------------
// Selection controls
// ---------------------------------------------------------------------

recipe!(
    RadioGroup,
    /// A set of mutually exclusive options. The host owns `value:
    /// Signal<String>` (the selected option's id); `on_change` writes the
    /// picked id back. Build the rows with `RadioOption::new(id, label)`.
    /// RadioGroup coordinates exclusivity for you.
    pub fn radio_group_controlled() -> ::runtime_core::Element {
        use crate::{tone, RadioGroup, RadioOption};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let plan = signal("pro".to_string());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| plan.set(v));
        ui! {
            RadioGroup(
                value = plan,
                on_change = on_change,
                options = vec![
                    RadioOption::new("free", "Free"),
                    RadioOption::new("pro", "Pro"),
                    RadioOption::new("team", "Team"),
                ],
                tone = tone::Primary,
            )
        }
    }
);

recipe!(
    Radio,
    /// A standalone radio row — the single-row primitive `RadioGroup` is
    /// built from. Use it directly only when laying out the rows
    /// yourself; the host then owns each row's `selected: Signal<bool>`
    /// and coordinates exclusivity in `on_select`.
    pub fn radio_standalone() -> ::runtime_core::Element {
        use crate::{tone, Radio};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let selected = signal(true);
        let on_select: Rc<dyn Fn()> = Rc::new(move || selected.set(true));
        ui! {
            Radio(
                label = Some("Email me updates".to_string()),
                selected = selected,
                on_select = on_select,
                tone = tone::Primary,
            )
        }
    }
);

recipe!(
    Textarea,
    /// A multi-line text input that grows to fit its content. `rows` sets
    /// the resting height; `max_rows` caps the autogrow (past it the
    /// field scrolls). The host owns the `value` signal; `on_change`
    /// fires the new text on each edit.
    pub fn textarea_autogrow() -> ::runtime_core::Element {
        use crate::Textarea;
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let bio = signal(String::new());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| bio.set(v));
        ui! {
            Textarea(
                label = Some("Bio".to_string()),
                value = bio,
                on_change = on_change,
                placeholder = Some("Tell us about yourself…".to_string()),
                rows = 2u32,
                max_rows = 8u32,
            )
        }
    }
);

// ---------------------------------------------------------------------
// Overlays (anchored / portal — shown as source, not a live preview)
// ---------------------------------------------------------------------

recipe!(
    Popover,
    /// An anchored floating panel. Anchor it to a trigger via a
    /// `Ref<PressableHandle>` (`bind_to` on the Button, `target =
    /// AnchorTarget::from(trigger)` on the Popover) and gate it behind an
    /// open-state signal. `side`/`align`/`offset` place it relative to
    /// the anchor.
    pub fn popover_anchored() -> ::runtime_core::Element {
        use crate::{typography_kind, Button, Popover, Stack, StackGap, Typography};
        use crate::AnchorTarget;
        use ::runtime_core::{signal, ui, PressableHandle, Ref};
        use ::std::rc::Rc;

        let trigger: Ref<PressableHandle> = Ref::new();
        let open = signal(false);
        let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
        let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        ui! {
            view {
                Button(label = "Details", on_click = on_open, bind_to = Some(trigger))
                if open.get() {
                    Popover(
                        target = Some(AnchorTarget::from(trigger)),
                        on_dismiss = Some(close.clone()),
                    ) {
                        Stack(gap = StackGap::Xs) {
                            Typography(content = "Order #1024", kind = typography_kind::H3)
                            Typography(content = "Shipped yesterday.", muted = true)
                        }
                    }
                }
            }
        }
    }
);

recipe!(
    Tooltip,
    /// A small hint that wraps its trigger and reveals itself on hover
    /// (desktop) or long-press (touch) — no host open-state signal. `text`
    /// is the hint shown in the bubble.
    pub fn tooltip_hint() -> ::runtime_core::Element {
        use crate::{Button, Tooltip};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let noop: Rc<dyn Fn()> = Rc::new(|| {});
        // The Tooltip wraps its trigger and shows on hover (desktop) /
        // long-press (touch) — no host open-state signal.
        ui! {
            Tooltip(text = "Saves your changes".to_string()) {
                Button(label = "Save", on_click = noop)
            }
        }
    }
);

recipe!(
    HoverCard,
    /// Rich detail on hover: the card opens after the pointer rests on the
    /// trigger (`open_delay_ms`), stays open while the pointer moves into
    /// it, and closes after it leaves both (`close_delay_ms`). Long-press
    /// opens it on touch. `content` is a closure rebuilt on each open; the
    /// `{ }` block is the trigger. The trigger wrapper is as wide as its
    /// content, so give the trigger its own size — a full-width child (a
    /// `Progress`) would shrink to nothing inside it.
    pub fn hover_card_breakdown() -> ::runtime_core::Element {
        use crate::{tone, HoverCard, Stack, StackAxis, StackGap, StackJustify, Tag, Typography};
        use ::runtime_core::ui;

        ui! {
            HoverCard(
                content = || ui! {
                    Stack(gap = StackGap::Xs) {
                        Stack(axis = StackAxis::Row, gap = StackGap::Lg, justify = StackJustify::Between) {
                            Typography(content = "Drilling")
                            Typography(content = "64%")
                        }
                        Stack(axis = StackAxis::Row, gap = StackGap::Lg, justify = StackJustify::Between) {
                            Typography(content = "Bolting")
                            Typography(content = "40%")
                        }
                    }
                },
            ) {
                Tag(label = "52% complete", tone = tone::Primary)
            }
        }
    }
);

recipe!(
    ToastHost,
    /// The mount point for transient notifications. Render exactly one
    /// `ToastHost` near the root; anywhere in the app, call
    /// `push_toast(message, tone)` to enqueue a toast and it appears at
    /// the host's `placement`. `dismiss_toast(id)` removes one early.
    pub fn toast_host() -> ::runtime_core::Element {
        use crate::{push_toast, tone, Button, ToastHost, ToastPlacement};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let notify: Rc<dyn Fn()> = Rc::new(|| {
            push_toast("Saved!", tone::Success);
        });
        ui! {
            view {
                Button(label = "Notify", on_click = notify)
                ToastHost(placement = ToastPlacement::BottomLeft)
            }
        }
    }
);

// ---------------------------------------------------------------------------
// Theming recipes — how to install and customize the idea-ui theme. These
// target the theme functions/types rather than a component, so they surface
// via `list_recipes` / `describe_recipe` as the "how do I theme this?" answer.
// ---------------------------------------------------------------------------

recipe!(
    install_idea_theme,
    /// The one required setup call: install a theme once, before the first
    /// render. Component styles need no install of their own — each
    /// component composes its slot sheets on first render — so this single
    /// call is the whole setup for an app that doesn't need custom modifiers. `light_theme()`/`dark_theme()` are the
    /// built-in starting points.
    pub fn theme_install_light() -> ::runtime_core::Element {
        use crate::{install_idea_theme, light_theme, Button};
        use ::runtime_core::ui;

        // Call once at app startup, before rendering the tree.
        install_idea_theme(light_theme());

        ui! { Button(label = "Themed") }
    }
);

recipe!(
    IdeaThemeDefaults,
    /// Tweak individual design tokens on a built-in theme, then install it.
    /// Color fields are `Tokenized<Color>`; spacing/radius/type-scale fields
    /// are plain `f32`. The string passed to `Tokenized::token` is cosmetic —
    /// install keys every field off its fixed canonical name — so a plain
    /// `Tokenized::Literal` override is all you need.
    pub fn theme_customize_tokens() -> ::runtime_core::Element {
        use crate::{install_idea_theme, light_theme, Button};
        use ::runtime_core::{ui, Color, Tokenized};

        let mut theme = light_theme();
        // A brand primary, plus a slightly larger radius / spacing / body size.
        theme.intents.primary.solid_bg = Tokenized::Literal(Color("#0066ff".into()));
        theme.radius.md = 10.0;
        theme.spacing.lg = 20.0;
        theme.typography.body_size = 15.0;
        install_idea_theme(theme);

        ui! { Button(label = "Brand") }
    }
);

recipe!(
    IdeaTokens,
    /// Write your OWN `stylesheet!` that follows the installed idea-ui theme.
    /// Declare the theme type in the `<…>` slot and the block binding (`base(t)`)
    /// becomes the token vocabulary: `t.spacing.md()` references the `spacing-md`
    /// token. The accessor path IS the token name, so a typo won't compile —
    /// unlike a string, which compiles and silently renders its fallback
    /// forever. Namespaces: `t.color.*`, `t.intent.<intent>.<slot>()`,
    /// `t.spacing.*`, `t.radius.*`, `t.typography.*_size()`. No palette hex
    /// appears here, so a reskin (or a light/dark swap) re-flows this sheet with
    /// zero edits to it — the theme stays the single source of truth.
    pub fn custom_sheet_theme_tokens() -> ::runtime_core::Element {
        use crate::{install_idea_theme, light_theme, IdeaThemeRef};
        use ::runtime_core::{stylesheet, ui, FlexDirection, Length};

        stylesheet! {
            Sidebar<IdeaThemeRef> {
                base(t) {
                    flex_direction: FlexDirection::Column,
                    height: Length::pct(100.0),
                    padding: t.spacing.md(),
                    gap: t.spacing.md(),
                    background: t.color.surface(),
                    border_color: t.color.border(),
                    border_radius: t.radius.lg(),
                }
                // Overlays bind the vocabulary too — variants, states,
                // breakpoints, and container queries all take a binding.
                state hovered(t) {
                    border_color: t.color.border_hover(),
                }
            }
        }

        install_idea_theme(light_theme());
        ui! { view(style = Sidebar()) { text { "Navigation" } } }
    }
);

recipe!(
    install_idea_theme_reactive,
    /// Signal-driven light/dark. `install_idea_theme_reactive` re-runs its
    /// selector whenever a signal the selector reads changes, swapping the
    /// active theme — no hand-rolled `effect!` needed. Flipping the `dark`
    /// signal re-themes the whole app; component sheets pull the new token
    /// values automatically.
    pub fn theme_dark_mode_toggle() -> ::runtime_core::Element {
        use crate::{dark_theme, install_idea_theme_reactive, light_theme, Button};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let dark = signal(false);
        install_idea_theme_reactive(move || if dark.get() { dark_theme() } else { light_theme() });

        let toggle: Rc<dyn Fn()> = Rc::new(move || dark.set(!dark.get()));
        ui! { Button(label = "Toggle theme", on_click = toggle) }
    }
);

// ---------------------------------------------------------------------------
// Typed value inputs, the segmented control, and Surface
// ---------------------------------------------------------------------------
//
// These trail the block above because they were added when the design-sync
// export revealed the gap: the exporter renders one preview per recipe, so a
// component with no recipe here ships no card and no usage docs. Note that
// `typed_field` and `menu_panel` are deliberately absent — they are
// `pub(crate)` wiring shared by the inputs and the anchored-menu family, not
// components. `ToastCard` is absent too: `ToastEntry::render` is private
// (a toast is only ever built through the `Toast` builder / `push_toast`), so
// a card cannot be constructed from outside the module to render statically.
//
// Recipes also live NEXT TO the components they document (chip.rs,
// date_input.rs, date_picker.rs, field.rs, segmented_control.rs) — check
// there before adding one here, or you will write a near-duplicate.

recipe!(
    Autocomplete,
    /// A type-to-filter combo box. The host owns the `value` signal (the
    /// committed option's `id`); `on_change` fires with that id when a row
    /// is picked. `options` are matched against the typed text (label and
    /// optional description) — pass the same `SelectOption::new(id, label)`
    /// rows a `Select` takes. Use `empty_text` for the "no matches" line.
    pub fn autocomplete_filtering() -> ::runtime_core::Element {
        use crate::{Autocomplete, SelectOption};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let value = signal(String::new());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));
        ui! {
            Autocomplete(
                value = value,
                on_change = on_change,
                options = vec![
                    SelectOption::new("rs", "Rust"),
                    SelectOption::new("ts", "TypeScript"),
                    SelectOption::new("go", "Go"),
                ],
                placeholder = Some("Search languages".to_string()),
                empty_text = Some("No languages match".to_string()),
            )
        }
    }
);


recipe!(
    Surface,
    /// A themed background panel. `background` picks the elevation layer by
    /// token (`Background` is the recessed page base, `Surface` the raised
    /// panel, `SurfaceAlt` a layer above it), so nesting Surfaces reads as
    /// depth without hand-picked colors. `grow` is the flex weight, which is
    /// what makes Surface the usual split-pane building block.
    pub fn surface_panel() -> ::runtime_core::Element {
        use crate::{Surface, SurfaceColor, Typography};
        use ::runtime_core::ui;

        ui! {
            Surface(background = SurfaceColor::Surface) {
                Typography(content = "Panel content")
            }
        }
    }
);

recipe!(
    Calendar,
    /// An inline month grid for picking a single date. The host owns the
    /// `value` signal; `on_change` fires with the clicked day. `min`/`max`
    /// bound the selectable range and `framed = true` draws the panel
    /// border (leave it off when the calendar already sits inside a Card
    /// or a popover).
    pub fn calendar_inline() -> ::runtime_core::Element {
        use crate::{Calendar, CivilDate};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let value = signal(CivilDate::new(2026, 3, 14));
        let on_change: Rc<dyn Fn(CivilDate)> = Rc::new(move |d| value.set(Some(d)));
        ui! {
            Calendar(value = value, on_change = on_change, framed = true)
        }
    }
);

recipe!(
    RangeCalendar,
    /// The two-ended sibling of [`calendar_inline`]. `value` holds the
    /// `(start, end)` pair; `on_change` fires with both ends once the
    /// second click lands, so a half-made range never reaches the host.
    pub fn range_calendar_inline() -> ::runtime_core::Element {
        use crate::{CivilDate, RangeCalendar};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        // Seeded with a real range: an unset RangeCalendar renders the epoch
        // month with no highlight, which documents neither the range styling
        // nor the component's normal state.
        let value = signal(
            CivilDate::new(2026, 3, 9).zip(CivilDate::new(2026, 3, 20)),
        );
        let on_change: Rc<dyn Fn(CivilDate, CivilDate)> =
            Rc::new(move |start, end| value.set(Some((start, end))));
        ui! {
            RangeCalendar(value = value, on_change = on_change, framed = true)
        }
    }
);


recipe!(
    TimeInput,
    /// The time-of-day counterpart to [`date_input_masked`], with the same
    /// parse-on-type / normalize-on-blur contract. `format` sets the mask
    /// (12- or 24-hour); `icon = true` shows the clock affordance.
    pub fn time_input_masked() -> ::runtime_core::Element {
        use crate::{CivilTime, TimeInput};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let value = signal(CivilTime::new(9, 30, 0));
        let on_change: Rc<dyn Fn(Option<CivilTime>)> = Rc::new(move |t| value.set(t));
        ui! {
            TimeInput(
                value = value,
                on_change = on_change,
                label = Some("Starts at".to_string()),
                icon = true,
            )
        }
    }
);


// ---------------------------------------------------------------------------
// Side-by-side comparisons and states. The docs playground shows one prop
// combination at a time; these show the set together, so a reader can pick
// one by eye.
// ---------------------------------------------------------------------------

recipe!(
    Button,
    /// The four surface treatments side by side. `variant` picks how the
    /// `tone` is painted: `Filled` for the primary action, `Soft` and
    /// `Outlined` for secondary ones, `Ghost` for low-emphasis actions in
    /// toolbars and table rows.
    pub fn button_variants() -> ::runtime_core::Element {
        use crate::{variant, Button, Stack, StackAxis, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, wrap = true) {
                Button(label = "Filled", variant = variant::Filled)
                Button(label = "Soft", variant = variant::Soft)
                Button(label = "Outlined", variant = variant::Outlined)
                Button(label = "Ghost", variant = variant::Ghost)
            }
        }
    }
);

recipe!(
    Button,
    /// A destructive pair and a busy save. `tone` carries the intent —
    /// `Danger` for delete, `Neutral` for cancel — and `loading = true`
    /// swaps the label for a spinner and blocks presses while a request is
    /// in flight. `disabled = true` dims the button and blocks presses.
    pub fn button_tones_and_states() -> ::runtime_core::Element {
        use crate::{tone, variant, Button, Stack, StackAxis, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, wrap = true) {
                Button(label = "Cancel", tone = tone::Neutral, variant = variant::Soft)
                Button(label = "Delete", tone = tone::Danger)
                Button(label = "Saving", loading = true)
                Button(label = "Unavailable", disabled = true)
            }
        }
    }
);

recipe!(
    Button,
    /// The three sizes, and the corner shapes. `size` matches Field,
    /// Select and SegmentedControl at the same step, so a button lines up
    /// beside them in a toolbar; `shape = shape::Pill` rounds the ends fully.
    pub fn button_sizes_and_shapes() -> ::runtime_core::Element {
        use crate::{shape, size, Button, Stack, StackAlign, StackAxis, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center, wrap = true) {
                Button(label = "Small", size = size::Sm)
                Button(label = "Medium", size = size::Md)
                Button(label = "Large", size = size::Lg)
                Button(label = "Pill", shape = shape::Pill)
            }
        }
    }
);

recipe!(
    Badge,
    /// One badge per tone. Pair the tone with the meaning (`Success` for
    /// done, `Warning` for attention, `Danger` for failure) so status reads
    /// the same everywhere; `Filled` raises the emphasis of the one that
    /// matters most.
    pub fn badge_tones() -> ::runtime_core::Element {
        use crate::{tone, variant, Badge, Stack, StackAxis, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, wrap = true) {
                Badge(label = "Draft", tone = tone::Neutral)
                Badge(label = "Info", tone = tone::Info)
                Badge(label = "Active", tone = tone::Success)
                Badge(label = "Due soon", tone = tone::Warning)
                Badge(label = "Failed", tone = tone::Danger)
                Badge(label = "3 new", tone = tone::Primary, variant = variant::Filled)
            }
        }
    }
);

recipe!(
    Alert,
    /// The four status tones, each with a title and a body line. `Soft` (the
    /// default) suits inline messages; `variant::Filled` is the loud form
    /// for the one banner a page must not miss.
    pub fn alert_tones() -> ::runtime_core::Element {
        use crate::{tone, variant, Alert, Stack, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(gap = StackGap::Sm) {
                Alert(title = "Heads up", body = Some("Maintenance is scheduled for Sunday.".to_string()), tone = tone::Info)
                Alert(title = "Saved", body = Some("Your changes are live.".to_string()), tone = tone::Success)
                Alert(title = "Storage almost full", body = Some("You have used 92% of your plan.".to_string()), tone = tone::Warning)
                Alert(title = "Payment failed", body = Some("Update your card to keep your plan.".to_string()), tone = tone::Danger, variant = variant::Filled)
            }
        }
    }
);

recipe!(
    Avatar,
    /// Every size, from a 24px list chip to an 80px profile header, with
    /// colors from `tone::palette_from_seed` — the same person always gets
    /// the same color.
    pub fn avatar_sizes() -> ::runtime_core::Element {
        use crate::{tone, Avatar, AvatarSize, Stack, StackAlign, StackAxis, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                Avatar(initials = "XS", size = AvatarSize::Xs, tone = tone::palette_from_seed("xs"))
                Avatar(initials = "SM", size = AvatarSize::Sm, tone = tone::palette_from_seed("sm"))
                Avatar(initials = "MD", size = AvatarSize::Md, tone = tone::palette_from_seed("md"))
                Avatar(initials = "LG", size = AvatarSize::Lg, tone = tone::palette_from_seed("lg"))
                Avatar(initials = "XL", size = AvatarSize::Xl, tone = tone::palette_from_seed("xl"))
            }
        }
    }
);

recipe!(
    Typography,
    /// The type scale, largest to smallest. Use the heading kinds for
    /// structure (one `H1` per screen), `Body` for running text and
    /// `Caption` / `muted = true` for secondary detail.
    pub fn typography_scale() -> ::runtime_core::Element {
        use crate::{typography_kind, Stack, StackGap, Typography};
        use ::runtime_core::ui;

        ui! {
            Stack(gap = StackGap::Xs) {
                Typography(content = "Display", kind = typography_kind::Display)
                Typography(content = "Heading 1", kind = typography_kind::H1)
                Typography(content = "Heading 2", kind = typography_kind::H2)
                Typography(content = "Heading 3", kind = typography_kind::H3)
                Typography(content = "Body text for paragraphs and descriptions.", kind = typography_kind::Body)
                Typography(content = "Small body text", kind = typography_kind::BodySm)
                Typography(content = "Caption, muted", kind = typography_kind::Caption, muted = true)
                Typography(content = "Overline", kind = typography_kind::Overline)
            }
        }
    }
);

recipe!(
    Card,
    /// The four card treatments. `Regular` (the default) frames content on
    /// the page surface, `Outline` draws only the border, `Elevated` lifts
    /// it with a shadow, and `Flat` is a borderless tinted panel for
    /// grouping inside another surface.
    pub fn card_variants() -> ::runtime_core::Element {
        use crate::components::card::variant;
        use crate::{Card, CardPadding, Grid, StackGap, Typography};
        use ::runtime_core::ui;

        ui! {
            Grid(columns = 2u32, gap = StackGap::Md) {
                Card(variant = variant::Regular, padding = CardPadding::Md) { Typography(content = "Regular") }
                Card(variant = variant::Outline, padding = CardPadding::Md) { Typography(content = "Outline") }
                Card(variant = variant::Elevated, padding = CardPadding::Md) { Typography(content = "Elevated") }
                Card(variant = variant::Flat, padding = CardPadding::Md) { Typography(content = "Flat") }
            }
        }
    }
);

recipe!(
    Field,
    /// A sign-up form's fields in their three states: valid with a hint,
    /// rejected with an `error` (which tints the input Danger and replaces
    /// the hint), and `disabled` for a value the user can't change.
    pub fn field_states() -> ::runtime_core::Element {
        use crate::{Field, Stack, StackGap};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let name = signal("Ada Lovelace".to_string());
        let email = signal("ada@".to_string());
        let team = signal("Analytical Engines".to_string());
        let on_name: Rc<dyn Fn(String)> = Rc::new(move |v| name.set(v));
        let on_email: Rc<dyn Fn(String)> = Rc::new(move |v| email.set(v));
        ui! {
            Stack(gap = StackGap::Md) {
                Field(label = Some("Name".to_string()), value = name, on_change = on_name, help = Some("As it appears on your ID.".to_string()))
                Field(label = Some("Email".to_string()), value = email, on_change = on_email, error = Some("Enter a full email address.".to_string()))
                Field(label = Some("Team".to_string()), value = team, disabled = true)
            }
        }
    }
);

recipe!(
    Checkbox,
    /// The states side by side. A partly-selected parent ("select all" over
    /// some rows) is a CHECKED box with a dash `icon` and `indeterminate =
    /// true`: `indeterminate` only tells assistive tech and the robot the
    /// box is mixed, the icon is what draws the dash. `disabled` dims the
    /// box and blocks toggling.
    pub fn checkbox_states() -> ::runtime_core::Element {
        use crate::{Checkbox, Stack, StackGap};
        use ::runtime_core::{signal, ui, FillRule, IconData};

        // In real code this is a pack constant, e.g. `icons_lucide::MINUS`.
        const DASH: IconData = IconData {
            view_box: (24, 24),
            paths: &["M5 12h14"],
            fill_rule: FillRule::NonZero,
            filled: false,
        };
        ui! {
            Stack(gap = StackGap::Sm) {
                Checkbox(label = Some("Unchecked".to_string()), value = signal(false))
                Checkbox(label = Some("Checked".to_string()), value = signal(true))
                Checkbox(
                    label = Some("Some channels".to_string()),
                    value = signal(true),
                    icon = Some(DASH),
                    indeterminate = true,
                )
                Checkbox(label = Some("Disabled".to_string()), value = signal(true), disabled = true)
            }
        }
    }
);

recipe!(
    Progress,
    /// The three thicknesses with rounded caps, in different tones — e.g.
    /// a thin bar under a header, a medium one in a card, a thick one for
    /// an upload.
    pub fn progress_sizes() -> ::runtime_core::Element {
        use crate::{tone, Progress, ProgressCap, ProgressSize, Stack, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(gap = StackGap::Md) {
                Progress(value = 0.3f32, size = ProgressSize::Sm, cap = ProgressCap::Rounded)
                Progress(value = 0.6f32, size = ProgressSize::Md, cap = ProgressCap::Rounded, tone = tone::Success)
                Progress(value = 0.9f32, size = ProgressSize::Lg, cap = ProgressCap::Rounded, tone = tone::Warning)
            }
        }
    }
);

recipe!(
    Tabs,
    /// The compact `TabIndicator::Dot` switcher, for a strip inside a card
    /// or toolbar where an underline would be too heavy.
    pub fn tabs_dot_indicator() -> ::runtime_core::Element {
        use crate::{Tab, TabIndicator, Tabs};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let active = signal("day".to_string());
        let tabs = signal(vec![Tab::new("day", "Day"), Tab::new("week", "Week"), Tab::new("month", "Month")]);
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| active.set(id));
        ui! {
            Tabs(tabs = tabs, active = active, on_change = on_change, indicator = TabIndicator::Dot)
        }
    }
);

// ---------------------------------------------------------------------------
// Rows and table pieces, shown in the container they belong to.
// ---------------------------------------------------------------------------

recipe!(
    ListItem,
    /// Settings rows with both slots filled: an avatar `leading`, a badge
    /// `trailing` (pushed to the right edge), and `active = true` marking the
    /// current row. `on_press` makes a row clickable.
    pub fn list_item_slots() -> ::runtime_core::Element {
        use crate::{tone, Avatar, AvatarSize, Badge, List, ListItem};
        use ::runtime_core::ui;
        use ::std::rc::Rc;

        let open: Rc<dyn Fn()> = Rc::new(|| { /* open the conversation */ });
        ui! {
            List {
                ListItem(
                    label = "Ada Lovelace",
                    leading = Some(ui! { Avatar(initials = "AL", size = AvatarSize::Sm) }),
                    trailing = Some(ui! { Badge(label = "2", tone = tone::Primary) }),
                    on_press = Some(open.clone()),
                    active = true,
                )
                ListItem(
                    label = "Grace Hopper",
                    leading = Some(ui! { Avatar(initials = "GH", size = AvatarSize::Sm, tone = tone::Teal) }),
                    on_press = Some(open),
                )
            }
        }
    }
);

#[cfg(feature = "table")]
recipe!(
    TableFooter,
    /// A totals row. `TableFooter` holds `TableRow`s that stay pinned below
    /// the body — the place for sums and counts over the rows above.
    pub fn table_footer_totals() -> ::runtime_core::Element {
        use crate::{Table, TableCell, TableFooter, TableRow};
        use ::runtime_core::ui;

        ui! {
            Table {
                TableRow {
                    TableCell(header = true, text = Some("Item".to_string()))
                    TableCell(header = true, text = Some("Hours".to_string()))
                }
                TableRow {
                    TableCell(text = Some("Drilling".to_string()))
                    TableCell(text = Some("6.5".to_string()))
                }
                TableRow {
                    TableCell(text = Some("Bolting".to_string()))
                    TableCell(text = Some("3.0".to_string()))
                }
                TableFooter {
                    TableRow {
                        TableCell(text = Some("Total".to_string()))
                        TableCell(text = Some("9.5".to_string()))
                    }
                }
            }
        }
    }
);

#[cfg(feature = "table")]
recipe!(
    TableGroup,
    /// Rows banded under a titled, collapsible group header. Each
    /// `TableGroup` takes its rows as children; its header row spans the
    /// table and toggles the band.
    pub fn table_grouped_rows() -> ::runtime_core::Element {
        use crate::{Table, TableCell, TableGroup, TableRow};
        use ::runtime_core::ui;

        ui! {
            Table {
                TableRow {
                    TableCell(header = true, text = Some("Machine".to_string()))
                    TableCell(header = true, text = Some("Status".to_string()))
                }
                TableGroup(title = "Shaft A".to_string()) {
                    TableRow {
                        TableCell(text = Some("Drill 4".to_string()))
                        TableCell(text = Some("Running".to_string()))
                    }
                    TableRow {
                        TableCell(text = Some("Drill 7".to_string()))
                        TableCell(text = Some("Idle".to_string()))
                    }
                }
                TableGroup(title = "Shaft B".to_string()) {
                    TableRow {
                        TableCell(text = Some("Loader 2".to_string()))
                        TableCell(text = Some("Running".to_string()))
                    }
                }
            }
        }
    }
);

#[cfg(feature = "table")]
recipe!(
    TablePager,
    /// Paging under a table. The host owns the offset (the first row on the
    /// page) and the page size; `on_offset` / `on_page_size` write them
    /// back, and the host renders that window of rows. Passing
    /// `page_sizes` adds the "Rows" picker.
    pub fn table_pager_controlled() -> ::runtime_core::Element {
        use crate::TablePager;
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let offset = signal(0_usize);
        let page_size = signal(25_usize);
        let on_offset: Rc<dyn Fn(usize)> = Rc::new(move |o| offset.set(o));
        let on_page_size: Rc<dyn Fn(usize)> = Rc::new(move |s| {
            page_size.set(s);
            offset.set(0);
        });
        ui! {
            TablePager(
                offset = offset,
                page_size = page_size,
                total = 240_usize,
                noun = "shifts".to_string(),
                on_offset = on_offset,
                page_sizes = vec![25, 50, 100],
                on_page_size = Some(on_page_size),
            )
        }
    }
);

// ---------------------------------------------------------------------------
// Date + time.
// ---------------------------------------------------------------------------

recipe!(
    DateTimeInput,
    /// A typed date-and-time field: the mask parses as the user types and
    /// normalizes on blur, and `picker = true` adds the calendar button.
    /// The host owns the `Option<CivilDateTime>` signal.
    pub fn date_time_input_start() -> ::runtime_core::Element {
        use crate::{CivilDate, CivilDateTime, CivilTime, DateTimeInput};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let start = signal(
            CivilDate::new(2026, 3, 14).zip(CivilTime::new(9, 30, 0)).map(|(d, t)| CivilDateTime::new(d, t)),
        );
        let on_change: Rc<dyn Fn(Option<CivilDateTime>)> = Rc::new(move |v| start.set(v));
        ui! {
            DateTimeInput(value = start, on_change = on_change, label = Some("Starts".to_string()), picker = true)
        }
    }
);

recipe!(
    DateTimePicker,
    /// A button-style picker for a date and a time: the calendar commits
    /// the day, the time row the hour and minute. `clearable` adds a Clear
    /// action that sends `None`.
    pub fn date_time_picker_meeting() -> ::runtime_core::Element {
        use crate::{CivilDateTime, DateTimePicker};
        use ::runtime_core::{signal, ui, Signal};
        use ::std::rc::Rc;

        let at: Signal<Option<CivilDateTime>> = signal(None);
        let on_change: Rc<dyn Fn(Option<CivilDateTime>)> = Rc::new(move |v| at.set(v));
        ui! {
            DateTimePicker(value = at, on_change = on_change, placeholder = "Schedule a meeting".to_string(), clearable = true)
        }
    }
);

// ---------------------------------------------------------------------------
// Menus.
// ---------------------------------------------------------------------------

recipe!(
    SubMenu,
    /// A nested menu: a `SubMenu` row inside a `Menu` opens its `items` to
    /// the side (hover on desktop, tap on touch). Each `MenuEntry::new(label,
    /// on_select)` is one row.
    pub fn submenu_move_to() -> ::runtime_core::Element {
        use crate::{AnchorTarget, Button, Menu, MenuEntry, MenuItem, SubMenu};
        use ::runtime_core::{signal, ui, PressableHandle, Ref};
        use ::std::rc::Rc;

        let trigger: Ref<PressableHandle> = Ref::new();
        let open = signal(false);
        let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
        let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        ui! {
            view {
                Button(label = "File", on_click = on_open, bind_to = Some(trigger))
                if open.get() {
                    Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(close.clone())) {
                        MenuItem(label = "Rename", on_select = close.clone())
                        SubMenu(
                            label = "Move to",
                            items = vec![
                                MenuEntry::new("Inbox", close.clone()),
                                MenuEntry::new("Archive", close.clone()),
                                MenuEntry::new("Trash", close.clone()),
                            ],
                        )
                    }
                }
            }
        }
    }
);
