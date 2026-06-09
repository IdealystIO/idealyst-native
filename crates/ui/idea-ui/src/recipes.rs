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
            Icon(data = HEART, size = 24.0, tone = Some(tone::Danger.into()))
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
    Card,
    /// An intent-tinted card. Setting `tone = Some(...)` paints the card
    /// with a muted tone background + matching border (the Soft tint Alert
    /// uses) — for support/crisis/info panels that need to read as
    /// intent-colored. Works with either variant.
    pub fn card_toned() -> ::runtime_core::Element {
        use crate::{tone, typography_kind, Card, CardPadding, Typography};
        use ::runtime_core::ui;

        ui! {
            Card(padding = CardPadding::Md, tone = Some(tone::Danger.into())) {
                Typography(content = "Account at risk", kind = typography_kind::H3)
                Typography(content = "Verify your email to avoid suspension.")
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
    /// A clickable tab strip. Tabs is pure UI: the host owns the active tab's
    /// `id` (a `Signal<String>`) and renders that tab's content itself (e.g. a
    /// `match` on `active.get()`). `tabs` is a `Signal<Vec<Tab>>` (a reactive,
    /// id-keyed list — wrap a fixed set in `signal!`); each `Tab::new(id, label)`
    /// carries its own identity.
    pub fn tabs_controlled() -> ::runtime_core::Element {
        use crate::{Tab, Tabs};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let active = signal!("overview".to_string());
        let tabs = signal!(vec![
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
    /// a colored monogram when there's no image. `color` picks the
    /// monogram palette and `size` scales the circle.
    pub fn avatar_initials() -> ::runtime_core::Element {
        use crate::{Avatar, AvatarColor, AvatarSize};
        use ::runtime_core::ui;

        ui! {
            Avatar(initials = "AL", color = AvatarColor::Primary, size = AvatarSize::Lg)
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
    /// determinate bar, or `indeterminate = true` for an ongoing
    /// animation when you can't measure progress. `value` is reactive.
    pub fn progress_bar() -> ::runtime_core::Element {
        use crate::{tone, Progress, Stack, StackGap};
        use ::runtime_core::ui;

        ui! {
            Stack(gap = StackGap::Md) {
                Progress(value = 0.65f32, tone = tone::Primary)
                Progress(indeterminate = true, tone = tone::Info)
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

        let level = signal!(0.5f32);
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

        let page = signal!(0_usize);
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

        let open = signal!(false);
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

        let open = signal!(vec![true, false, false]);
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

        let plan = signal!("pro".to_string());
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

        let selected = signal!(true);
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

        let bio = signal!(String::new());
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
        use ::runtime_core::primitives::portal::AnchorTarget;
        use ::runtime_core::{signal, ui, PressableHandle, Ref};
        use ::std::rc::Rc;

        let trigger: Ref<PressableHandle> = Ref::new();
        let open = signal!(false);
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
    /// A small hint anchored to a trigger. Anchor it the same way as a
    /// Popover (`bind_to` on the trigger, `target =
    /// AnchorTarget::from(trigger)` on the Tooltip) and reveal it on
    /// hover/focus by flipping its presence signal. `text` is the hint.
    pub fn tooltip_hint() -> ::runtime_core::Element {
        use crate::{Button, Tooltip};
        use ::runtime_core::primitives::portal::AnchorTarget;
        use ::runtime_core::{signal, ui, PressableHandle, Ref};
        use ::std::rc::Rc;

        let trigger: Ref<PressableHandle> = Ref::new();
        let show = signal!(false);
        let noop: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            view {
                Button(label = "Save", on_click = noop, bind_to = Some(trigger))
                if show.get() {
                    Tooltip(
                        target = Some(AnchorTarget::from(trigger)),
                        text = "Saves your changes",
                    )
                }
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
                ToastHost(placement = ToastPlacement::Bottom)
            }
        }
    }
);
