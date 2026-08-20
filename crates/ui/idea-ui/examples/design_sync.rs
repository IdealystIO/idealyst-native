//! The `/design-sync` converter — exports idea-ui's design system as the
//! artifacts a non-Rust consumer (claude.ai/design) can render.
//!
//! Run:
//!   RUSTFLAGS="--cfg idealyst_premint --cfg idealyst_premint_dump" \
//!     cargo run -p idea-ui --features style-dump,catalog \
//!       --example design_sync -- <out-dir>
//!
//! Emits into `<out-dir>`:
//!
//! - `tokens.css`     — every theme token as a CSS custom property, light
//!                      (`:root`) + dark (`[data-theme=dark]`). The names are
//!                      the ones `ThemeTokens::tokens()` installs, which is
//!                      exactly what the web backend emits as `var(--<name>)`
//!                      for a `Tokenized<T>`. The runtime's own contract, not
//!                      a translation of it.
//! - `components.css` — idea-ui's real preminted component CSS, from
//!                      `premint_dump::dump_all_css()`. Every rule resolves
//!                      against the tokens above.
//! - `manifest.json`  — component name → preminted base class + each variant
//!                      axis, its values and its default.
//! - `recipes.json`   — every compile-checked recipe in `idea_ui::recipes`,
//!                      rendered to real HTML through the SSR backend.
//!
//! # Why the manifest is load-bearing
//!
//! `components.css` keys every rule on an FNV hash of the sheet's identity
//! string (`premint_class_name`), and that hash is one-way: from the CSS
//! alone you cannot tell which `iy-xxxxxxxxxxxx` block is Button's. The
//! manifest walks the same sheets from the NAMED side, so a consumer can
//! stamp the right classes. The list to stamp is exactly what
//! `StyleApplication::preminted_class_list` builds:
//!
//!     <base> <base>-<axis>-<value>   (one per axis, sheet order)
//!
//! # Why the components are RENDERED, not described
//!
//! A component's DOM shape and class list are decided by the framework —
//! `Card` assembles its sheet per variant-set at mount, `Button` picks
//! `layout-row` only when it carries an icon, several components nest
//! sub-sheets (`ExtButton` + `ExtButtonLabel`). Inferring that from source
//! is guesswork that silently drifts. Rendering through `backend_ssr` makes
//! the framework the source of truth.
//!
//! # The pass order, and why it is not negotiable
//!
//! Sheets reach the premint registry two ways: `stylesheet!` registers at
//! LINK time, but a component that assembles its sheet AT MOUNT (Select's
//! dropdown, Slider's container, Table's cells, Toast) only registers once
//! something renders it. So:
//!
//!   1. render every recipe with the minted-class guard DISARMED — this is
//!      what registers the mount-built sheets;
//!   2. dump the CSS, now covering both kinds;
//!   3. render every recipe AGAIN with the guard armed from that CSS — any
//!      class still missing is a real hole and warns loudly, instead of
//!      shipping markup that references rules nothing defines.
//!
//! All three happen in ONE process: the registry is process-local, so a
//! separate dump binary would never see what step 1 registered.
//!
//! # The two cfgs
//!
//! - `idealyst_premint_dump` — makes `stylesheet!` register its sheets into
//!   `PREMINT_SHEETS` at link time (the CSS dump reads that registry).
//! - `idealyst_premint` — makes a `StyleApplication` ATTACH as a preminted
//!   class stamp instead of resolving through the live engine. Without it
//!   the render emits live-engine `ui-<hash>` classes, which the dumped
//!   `iy-*` CSS has no rules for.

use std::rc::Rc;

use runtime_core::{Element, Length, StyleSheet, TokenValue};

use idea_theme::{dark_theme, light_theme, IdeaThemeRef, ThemeTokens, DEFAULT_FONT_STACK};

// ---------------------------------------------------------------------
// tokens.css
// ---------------------------------------------------------------------

/// `12.0` → `12`; keeps generated CSS free of pointless `.0` suffixes.
fn trim(n: f32) -> String {
    let s = format!("{n}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

fn token_value(v: &TokenValue) -> String {
    match v {
        TokenValue::Color(c) => c.0.clone(),
        TokenValue::Length(l) => match l {
            Length::Px(n) => format!("{}px", trim(*n)),
            Length::Percent(n) => format!("{}%", trim(*n)),
            Length::Auto => "0".into(),
            // `Length::Full` on a corner radius resolves to `min(w,h)/2`
            // (`Length::resolve_radius`). CSS reaches the same value from
            // a large px radius: the spec scales radii down until no pair
            // overlaps a side, clamping at exactly half the shorter side.
            // So 9999px *is* the pill here, not an approximation of one —
            // it stays a pill at any box size.
            Length::Full => "9999px".into(),
        },
        TokenValue::Number(n) => trim(*n),
    }
}

fn decls(theme: &IdeaThemeRef, indent: &str) -> String {
    theme
        .tokens()
        .iter()
        .map(|e| format!("{indent}--{}: {};", e.name, token_value(&e.value)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn tokens_css() -> String {
    let light = IdeaThemeRef::new(light_theme());
    let dark = IdeaThemeRef::new(dark_theme());
    let mut s = String::new();
    s.push_str(
        "/* idea-ui default theme tokens — GENERATED by\n   `cargo run -p idea-ui --example \
         design_sync_dump`. Do not edit by hand.\n\n   Names match the token registry \
         `ThemeTokens::tokens()` installs, which is\n   what the web backend emits as \
         `var(--<name>)` for every `Tokenized<T>`. */\n\n",
    );
    s.push_str(":root {\n");
    s.push_str(&format!("  --iy-default-font: {DEFAULT_FONT_STACK};\n"));
    s.push_str(&decls(&light, "  "));
    s.push_str("\n}\n\n");
    // Explicit opt-in wins over the media query below, so a consumer can
    // force either mode regardless of OS preference.
    s.push_str("[data-theme=\"dark\"] {\n");
    s.push_str(&decls(&dark, "  "));
    s.push_str("\n}\n\n");
    s.push_str("@media (prefers-color-scheme: dark) {\n  :root:not([data-theme=\"light\"]) {\n");
    s.push_str(&decls(&dark, "    "));
    s.push_str("\n  }\n}\n");
    s
}

// ---------------------------------------------------------------------
// manifest.json
// ---------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn emit(name: &str, sheet: &Rc<StyleSheet>, out: &mut Vec<String>) {
    // A sheet with no premint identity resolves only through the live
    // style engine — it has no build-time CSS, so there is no class for a
    // consumer to stamp. Recording it would be a dangling reference.
    let Some(base) = sheet.premint_class() else { return };
    let axes = sheet
        .premint_variant_axes()
        .iter()
        .map(|(axis, values, default)| {
            let vals = values
                .iter()
                .map(|v| format!("\"{}\"", esc(v)))
                .collect::<Vec<_>>()
                .join(", ");
            let def = match default {
                Some(d) => format!("\"{}\"", esc(d)),
                None => "null".to_string(),
            };
            format!(
                "      {{ \"axis\": \"{}\", \"values\": [{}], \"default\": {} }}",
                esc(axis),
                vals,
                def
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    out.push(format!(
        "  {{\n    \"name\": \"{}\",\n    \"class\": \"{}\",\n    \"axes\": [{}]\n  }}",
        esc(name),
        esc(base),
        if axes.is_empty() { String::new() } else { format!("\n{axes}\n    ") }
    ));
}

fn manifest_json() -> String {
    let mut out: Vec<String> = Vec::new();

    let macro_sheets: Vec<(&str, Rc<StyleSheet>)> = vec![
        ("Stack", idea_ui::stylesheets::stack_style()),
        ("Button", idea_ui::stylesheets::button_style()),
        ("Typography", idea_ui::stylesheets::typography_style()),
        ("Card", idea_ui::stylesheets::card_style()),
        ("Field", idea_ui::stylesheets::field_style()),
        ("FieldGroup", idea_ui::stylesheets::field_group_style()),
        ("FieldLabel", idea_ui::stylesheets::field_label_style()),
        ("FieldHelp", idea_ui::stylesheets::field_help_style()),
        ("Divider", idea_ui::stylesheets::divider_style()),
        ("Badge", idea_ui::stylesheets::badge_style()),
        ("SwitchRow", idea_ui::stylesheets::switch_row_style()),
        ("SwitchThumb", idea_ui::stylesheets::switch_thumb_style()),
        ("ControlRow", idea_ui::stylesheets::control_row_style()),
        ("SurfaceSheet", idea_ui::stylesheets::surface_sheet_style()),
        ("ToastStack", idea_ui::stylesheets::toast_stack_style()),
        ("SelectTrigger", idea_ui::stylesheets::select_trigger_style()),
        ("SelectMenu", idea_ui::stylesheets::select_menu_style()),
        ("SelectOption", idea_ui::stylesheets::select_option_style()),
        ("AutocompleteBox", idea_ui::stylesheets::autocomplete_box_style()),
        ("AutocompleteInput", idea_ui::stylesheets::autocomplete_input_style()),
        ("AutocompleteChevron", idea_ui::stylesheets::autocomplete_chevron_style()),
        ("AutocompleteEmpty", idea_ui::stylesheets::autocomplete_empty_style()),
        ("Spacer", idea_ui::stylesheets::spacer_style()),
        ("Center", idea_ui::stylesheets::center_style()),
        ("IconButton", idea_ui::stylesheets::icon_button_style()),
        ("Avatar", idea_ui::stylesheets::avatar_style()),
        ("AvatarImage", idea_ui::stylesheets::avatar_image_style()),
        ("AvatarText", idea_ui::stylesheets::avatar_text_style()),
        ("Tag", idea_ui::stylesheets::tag_style()),
        ("TagLabel", idea_ui::stylesheets::tag_label_style()),
        ("TagClose", idea_ui::stylesheets::tag_close_style()),
        ("Alert", idea_ui::stylesheets::alert_style()),
        ("AlertContent", idea_ui::stylesheets::alert_content_style()),
        ("Skeleton", idea_ui::stylesheets::skeleton_style()),
        ("TabBar", idea_ui::stylesheets::tab_bar_style()),
        ("TabButton", idea_ui::stylesheets::tab_button_style()),
        ("TabButtonDot", idea_ui::stylesheets::tab_button_dot_style()),
        ("TabDot", idea_ui::stylesheets::tab_dot_style()),
        ("TabPanel", idea_ui::stylesheets::tab_panel_style()),
        ("Modal", idea_ui::stylesheets::modal_style()),
        ("Popover", idea_ui::stylesheets::popover_style()),
        ("Table", idea_ui::stylesheets::table_style()),
        ("TableHeadCell", idea_ui::stylesheets::table_head_cell_style()),
        ("TableBodyCell", idea_ui::stylesheets::table_body_cell_style()),
        ("TableHeadText", idea_ui::stylesheets::table_head_text_style()),
        ("TableBodyText", idea_ui::stylesheets::table_body_text_style()),
        ("TableCellInner", idea_ui::stylesheets::table_cell_inner_style()),
        ("CollapsibleContainer", idea_ui::stylesheets::collapsible_container_style()),
        ("CollapsibleHeader", idea_ui::stylesheets::collapsible_header_style()),
        ("CollapsibleChevron", idea_ui::stylesheets::collapsible_chevron_style()),
        ("CollapsibleBody", idea_ui::stylesheets::collapsible_body_style()),
        ("CollapsibleBodyAnimated", idea_ui::stylesheets::collapsible_body_animated_style()),
        ("AccordionContainer", idea_ui::stylesheets::accordion_container_style()),
        ("AccordionItemSeparator", idea_ui::stylesheets::accordion_item_separator_style()),
        ("TooltipBubble", idea_ui::stylesheets::tooltip_bubble_style()),
        ("TooltipBubbleText", idea_ui::stylesheets::tooltip_bubble_text_style()),
        ("MenuItemRow", idea_ui::stylesheets::menu_item_row_style()),
        ("MenuLabel", idea_ui::stylesheets::menu_label_style()),
        ("MenuSeparator", idea_ui::stylesheets::menu_separator_style()),
        ("MenuChevron", idea_ui::stylesheets::menu_chevron_style()),
        ("MenuCheckbox", idea_ui::stylesheets::menu_checkbox_style()),
        ("MenuCheckMark", idea_ui::stylesheets::menu_check_mark_style()),
        ("BreadcrumbRow", idea_ui::stylesheets::breadcrumb_row_style()),
        ("BreadcrumbItem", idea_ui::stylesheets::breadcrumb_item_style()),
        ("BreadcrumbSeparator", idea_ui::stylesheets::breadcrumb_separator_style()),
        ("PaginationRow", idea_ui::stylesheets::pagination_row_style()),
        ("PageButton", idea_ui::stylesheets::page_button_style()),
        ("ListContainer", idea_ui::stylesheets::list_container_style()),
        ("ListItemRow", idea_ui::stylesheets::list_item_row_style()),
        ("GridContainer", idea_ui::stylesheets::grid_container_style()),
        ("LinkText", idea_ui::stylesheets::link_text_style()),
        ("ImageBox", idea_ui::stylesheets::image_box_style()),
        ("CalendarPanel", idea_ui::stylesheets::calendar_panel_style()),
        ("CalendarHeader", idea_ui::stylesheets::calendar_header_style()),
        ("CalendarTitleButton", idea_ui::stylesheets::calendar_title_button_style()),
        ("CalendarWeekdayCell", idea_ui::stylesheets::calendar_weekday_cell_style()),
        ("CalendarWeekRow", idea_ui::stylesheets::calendar_week_row_style()),
        ("CalendarDay", idea_ui::stylesheets::calendar_day_style()),
        ("CalendarZoomCell", idea_ui::stylesheets::calendar_zoom_cell_style()),
    ];
    for (name, sheet) in &macro_sheets {
        emit(name, sheet, &mut out);
    }

    // Trait-driven sheets — assembled at theme install, identity via
    // `premint_as`. These are the open-extension components whose axes
    // come from the installed tone/variant/size/shape vocabularies, so
    // their value lists reflect what an app actually has available.
    let assembled: Vec<(&str, Rc<StyleSheet>)> = vec![
        ("ExtButton", idea_theme::extensible::installed_button_sheet()),
        ("ExtButtonLabel", idea_theme::extensible::installed_button_label_sheet()),
        ("ExtBadge", idea_theme::extensible::installed_badge_sheet()),
        ("ExtTag", idea_theme::extensible::installed_tag_sheet()),
        ("ExtAlert", idea_theme::extensible::installed_alert_sheet()),
        ("ExtTypography", idea_theme::extensible::installed_typography_sheet()),
        ("ExtIconButton", idea_theme::extensible::installed_icon_button_sheet()),
        ("ExtSwitch", idea_theme::extensible::installed_switch_sheet()),
    ];
    for (name, sheet) in &assembled {
        emit(name, sheet, &mut out);
    }

    // Every OTHER registered assembled sheet — the ones components build at
    // MOUNT (`Card` assembles per variant-set, `Field` per tone-set, …).
    // They have no `installed_*` accessor to name them, but their class and
    // axes are what a consumer must stamp, and without them a component like
    // Card exports with no documented axes at all. Named by class so the
    // lookup still works; callers key on `class`, not `name`.
    let named: std::collections::BTreeSet<String> =
        out.iter().filter_map(|e| {
            e.split("\"class\": \"").nth(1).and_then(|r| r.split('"').next()).map(str::to_string)
        }).collect();
    for sheet in runtime_core::premint::assembled_sheets() {
        let Some(class) = sheet.premint_class() else { continue };
        if named.contains(class) {
            continue;
        }
        let class = class.to_string();
        emit(&class, &sheet, &mut out);
    }

    format!("[\n{}\n]\n", out.join(",\n"))
}

// ---------------------------------------------------------------------
// recipes.json — the components, rendered by the framework itself
// ---------------------------------------------------------------------

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Scan `iy-…` class tokens out of the dumped CSS. Hand-rolled rather
/// than pulling a regex crate into a dev example.
fn scan_minted_classes(css: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = css.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.' {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b'_') {
                j += 1;
            }
            if j > start && css[start..j].starts_with("iy-") {
                out.push(css[start..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Render every recipe on its own fresh world + backend, so one recipe's
/// signals and theme install cannot leak into the next.
fn render_recipes(
    recipes: &[(&str, &str, &str, fn() -> Element)],
    quiet: bool,
) -> (String, String) {
    let mut items: Vec<String> = Vec::new();
    // Not every sheet can premint: an application carrying per-call-site
    // overrides or a computed layer is disqualified by
    // `preminted_class_list`, so it resolves through the LIVE engine and
    // its rules land in `RenderedPage::head_css` instead of the dumped
    // asset (Table's measured columns and Toast's stack are the two in
    // this set). Dropping that CSS is what collapsed Table's columns to
    // min-content — the markup referenced `ui-<hash>` classes nothing
    // defined. Collect and ship it alongside the preminted asset.
    let mut head_css = String::new();
    for (component, name, doc, build) in recipes {
        let page = backend_ssr::newcore::render_path("/", || {
            idea_theme::install_idea_theme(light_theme());
            // SSR requires exactly one top-level node, and some recipes are
            // legitimately multi-root (a Tooltip is anchor + floating panel).
            // An unstyled `view` is the documented wrapper for that; the card
            // generator strips it back off, so it never reaches the preview.
            runtime_core::IntoElement::into_element(runtime_core::view(vec![build()]))
        });
        let html = page.html;
        if !page.head_css.is_empty() && !head_css.contains(&page.head_css) {
            head_css.push_str(&page.head_css);
            head_css.push('\n');
        }
        if !quiet {
            eprintln!("[design-sync]   {component}::{name} ({} bytes)", html.len());
        }
        items.push(format!(
            "  {{\n    \"component\": \"{}\",\n    \"recipe\": \"{}\",\n    \"doc\": \"{}\",\n    \"html\": \"{}\"\n  }}",
            json_escape(component),
            json_escape(name),
            json_escape(doc),
            json_escape(&html)
        ));
    }
    (format!("[\n{}\n]\n", items.join(",\n")), head_css)
}

fn main() {
    let out_dir = std::env::args().nth(1).expect("usage: design_sync <out-dir>");
    std::fs::create_dir_all(&out_dir).expect("create out dir");

    let recipes: Vec<(&str, &str, &str, fn() -> Element)> = vec![
        ("Accordion", "accordion_single", "A set of collapsible items. `expand = AccordionExpand::Single` keeps at most one open at a time (Multi allows any subset). The host owns `open: Signal<Vec<bool>>` (one bool per item); the Accordion writes to it on click. Each `AccordionItem` carries a `title` and an `Element` `body`.", idea_ui::recipes::accordion_single as fn() -> Element),
        ("Alert", "alert_with_action", "A banner with a title, optional body line, an optional trailing `action` slot, and a configurable `close`. Pick a semantic `tone` (Info/Success/Warning/Danger) and a `variant` (Soft/Filled/ Outline). `close = AlertClose::Button(handler)` shows the standard ×; `AlertClose::Custom(element)` supplies your own; the default `AlertClose::None` shows nothing.", idea_ui::recipes::alert_with_action as fn() -> Element),
        ("Avatar", "avatar_initials", "A round user chip. Pass `src` for a photo, or `initials` to render a colored monogram when there's no image. `color` picks the monogram palette and `size` scales the circle.", idea_ui::recipes::avatar_initials as fn() -> Element),
        ("Badge", "badge_status", "A small status pill for counts and labels. Pick a semantic `tone` (Primary/Success/Danger/…) and a `variant` (Soft/Filled/Outline). `label` is reactive, so it can be driven by a signal.", idea_ui::recipes::badge_status as fn() -> Element),
        ("Breadcrumbs", "breadcrumbs_trail", "A navigation trail. Build it from `Crumb`s — `Crumb::linked(label, on_press)` for clickable ancestors and `Crumb::new(label)` for the current (non-clickable) page. The `separator` between them is configurable.", idea_ui::recipes::breadcrumbs_trail as fn() -> Element),
        ("Button", "button_basic", "A primary action button that runs a callback when pressed. The default `tone`/`variant`/`size`/`shape` give a filled primary button; pass them explicitly to vary it.", idea_ui::recipes::button_basic as fn() -> Element),
        ("Button", "button_icon_block", "A full-width call-to-action with a leading icon. `block = true` stretches the button to its container's width; `leading_icon` / `trailing_icon` take an `IconData` constant (from an icon pack like `icons_lucide`) and render it inline beside the label, inheriting the button's text color.", idea_ui::recipes::button_icon_block as fn() -> Element),
        ("Card", "card_elevated", "A surface container that wraps its children in a themed, rounded, bordered panel. Use `variant = card::variant::Elevated` for a raised look (surface-alt background + shadow); `padding` sets the inner spacing.", idea_ui::recipes::card_elevated as fn() -> Element),
        ("Card", "card_toned", "An intent-tinted card. Setting `tone = Some(...)` paints the card with a muted tone background + matching border (the Soft tint Alert uses) — for support/crisis/info panels that need to read as intent-colored. Works with either variant.", idea_ui::recipes::card_toned as fn() -> Element),
        ("Center", "center_content", "Centers its children on both axes inside the space it's given. Drop any single child (or a Stack) inside and it sits dead center — handy for empty states and loading screens.", idea_ui::recipes::center_content as fn() -> Element),
        ("Checkbox", "checkbox_controlled", "A controlled checkbox with a label. The host owns the `value: Signal<bool>`; `on_change` fires the toggled value. Tapping anywhere on the row (box or label) toggles it.", idea_ui::recipes::checkbox_controlled as fn() -> Element),
        ("Collapsible", "collapsible_section", "A titled section that expands and collapses. The host owns the open-state `Signal<bool>`; `on_change` fires the toggled value. Children are revealed when open; the default `Measured` transition animates to the body's natural height.", idea_ui::recipes::collapsible_section as fn() -> Element),
        ("Divider", "divider_separator", "A hairline rule separating content. Defaults to a horizontal line that fills its parent's width; pass `axis = DividerAxis::Vertical` for a vertical rule inside a row.", idea_ui::recipes::divider_separator as fn() -> Element),
        ("Field", "field_controlled", "A labeled, controlled text input. The host owns the `value` signal; `on_change` fires the new text on each edit. Add `help` for hint text or `error = Some(...)` to flag a validation problem (which paints the input in the Danger tone automatically).", idea_ui::recipes::field_controlled as fn() -> Element),
        ("Grid", "grid_columns", "A fixed-column grid. `columns` sets how many equal-width tracks each row has; `gap` spaces both rows and columns. Children flow left-to-right, wrapping to a new row every `columns` items.", idea_ui::recipes::grid_columns as fn() -> Element),
        ("Icon", "icon_tinted", "A sized, optionally tinted vector icon. `data` is an `IconData` constant (from an icon pack like `icons_lucide`); `size` sets the square in points. Pass `tone = Some(...)` to paint it in a semantic intent color, or `color = Some(...)` for an explicit one — with neither, it inherits the ambient text color.", idea_ui::recipes::icon_tinted as fn() -> Element),
        ("IconButton", "icon_button_close", "A square, single-glyph clickable. Pick a `tone` × `variant` × `size`; `glyph` is the character drawn inside (e.g. `\"×\"` for a close button). `on_click` fires on press.", idea_ui::recipes::icon_button_close as fn() -> Element),
        ("IconButton", "icon_button_vector", "A square icon button rendering a vector (Lucide) icon rather than a text glyph. Pass `icon = Some(IconData)` and it takes precedence over `glyph`, tinting to match the tone × variant.", idea_ui::recipes::icon_button_vector as fn() -> Element),
        ("Image", "image_rounded", "A bitmap image. `src` is the URL/path; `alt` is the accessible description. Constrain it with `width`/`height` (points) and set `rounded = true` for rounded corners (e.g. thumbnails).", idea_ui::recipes::image_rounded as fn() -> Element),
        ("Link", "link_external", "An inline hyperlink to an external URL. `label` is the visible text; `url` is the destination. For in-app navigation between screens, use the framework's `link` primitive with a typed route instead.", idea_ui::recipes::link_external as fn() -> Element),
        ("List", "list_items", "A vertical list of rows. Compose `ListItem`s inside it; each row takes a `label`, an optional `on_press`, and optional `leading`/`trailing` slots for icons or controls.", idea_ui::recipes::list_items as fn() -> Element),
        ("Menu", "menu_anchored", "An anchored command panel. Anchor it to a trigger via a `Ref<PressableHandle>` (`bind_to` on the Button, `target = AnchorTarget::from(trigger)` on the Menu) and gate it behind an open-state signal. Compose `MenuItem`/`MenuLabel`/`MenuSeparator` children; flip the signal in each `on_select` and `on_dismiss`.", idea_ui::recipes::menu_anchored as fn() -> Element),
        ("Modal", "modal_confirm", "A centered overlay with a dimming backdrop and a themed surface. The host owns an open-state `Signal<bool>` and passes it as `open` — the Modal is ALWAYS mounted (no `if open { .. }` gate); flipping `open` false animates the exit then unmounts. `content` is a closure (rebuilt on each open); flip the signal in `on_dismiss`.", idea_ui::recipes::modal_confirm as fn() -> Element),
        ("Pagination", "pagination_pager", "A page selector. The host owns the current `page` (zero-based) `Signal<usize>`; `total` is the page count; `on_change` fires the newly chosen page so the host can refetch and update the signal.", idea_ui::recipes::pagination_pager as fn() -> Element),
        ("Popover", "popover_anchored", "An anchored floating panel. Anchor it to a trigger via a `Ref<PressableHandle>` (`bind_to` on the Button, `target = AnchorTarget::from(trigger)` on the Popover) and gate it behind an open-state signal. `side`/`align`/`offset` place it relative to the anchor.", idea_ui::recipes::popover_anchored as fn() -> Element),
        ("Progress", "progress_bar", "A horizontal progress bar. Set `value` in 0.0..=1.0 for a value-driven bar (changes animate to the new width), or pick a `mode`: `Indeterminate` sweeps endlessly when you can't measure progress; `Simulated` creeps toward full like a fake page loader. `value` is reactive.", idea_ui::recipes::progress_bar as fn() -> Element),
        ("Radio", "radio_standalone", "A standalone radio row — the single-row primitive `RadioGroup` is built from. Use it directly only when laying out the rows yourself; the host then owns each row's `selected: Signal<bool>` and coordinates exclusivity in `on_select`.", idea_ui::recipes::radio_standalone as fn() -> Element),
        ("RadioGroup", "radio_group_controlled", "A set of mutually exclusive options. The host owns `value: Signal<String>` (the selected option's id); `on_change` writes the picked id back. Build the rows with `RadioOption::new(id, label)`. RadioGroup coordinates exclusivity for you.", idea_ui::recipes::radio_group_controlled as fn() -> Element),
        ("Select", "select_controlled", "A controlled dropdown. The host owns the `value` signal (the chosen option's `id`); `on_change` writes the picked id back into it. Build the rows with `SelectOption::new(id, label)`.", idea_ui::recipes::select_controlled as fn() -> Element),
        ("Skeleton", "skeleton_placeholder", "Placeholder shimmer blocks shown while content loads. Stack a few with varied `width`s (Full/ThreeQuarter/Half or `Px`) to suggest the shape of the incoming content; `height` sets each block's thickness.", idea_ui::recipes::skeleton_placeholder as fn() -> Element),
        ("Slider", "slider_controlled", "A controlled horizontal value slider. The host owns `value: Signal<f32>`; `on_change` fires the new value during the drag. `min`/`max`/`step` bound and quantize it; `tone` colors the fill + thumb. Keep a fixed `width` and don't rebuild the Slider mid-drag (see its docs).", idea_ui::recipes::slider_controlled as fn() -> Element),
        ("Spacer", "spacer_gap", "A flexible gap that pushes its siblings apart. In a row it expands to fill the free space, shoving the items on either side to the edges — the standard \"title on the left, actions on the right\" toolbar trick.", idea_ui::recipes::spacer_gap as fn() -> Element),
        ("Spinner", "spinner_loading", "A spinning loading indicator for indeterminate waits. `size` picks `Small` or `Large`. Pair it with a label or center it in the area that's loading.", idea_ui::recipes::spinner_loading as fn() -> Element),
        ("Stack", "stack_layout", "The everyday vertical layout: stacks its children in a column with a uniform `gap`. Switch to a horizontal row with `axis = StackAxis::Row`; `align`/`justify` control cross- and main-axis placement.", idea_ui::recipes::stack_layout as fn() -> Element),
        ("Switch", "switch_controlled", "A controlled slide-toggle with an inline label. The host owns the `value: Signal<bool>`; `on_change` fires the flipped value. Use a semantic `tone` (e.g. Success) to color the \"on\" track.", idea_ui::recipes::switch_controlled as fn() -> Element),
        ("Table", "table_basic", "A themed data table: a header row (cells with `header = true`) plus body rows. Use `TableCell(header = true, text = \"...\")` for the simple text case; pass a `children` block for richer cell content.", idea_ui::recipes::table_basic as fn() -> Element),
        ("Tabs", "tabs_controlled", "A clickable tab strip. Tabs is pure UI: the host owns the active tab's `id` (a `Signal<String>`) and renders that tab's content itself (e.g. a `match` on `active.get()`). `tabs` is a `Signal<Vec<Tab>>` (a reactive, id-keyed list — wrap a fixed set in `signal!`); each `Tab::new(id, label)` carries its own identity.", idea_ui::recipes::tabs_controlled as fn() -> Element),
        ("Tag", "tag_removable", "A pill label, optionally removable. Provide `on_remove = Some(...)` to show a close affordance (e.g. for filter chips); omit it for a static tag. `tone` × `variant` set the palette.", idea_ui::recipes::tag_removable as fn() -> Element),
        ("Textarea", "textarea_autogrow", "A multi-line text input that grows to fit its content. `rows` sets the resting height; `max_rows` caps the autogrow (past it the field scrolls). The host owns the `value` signal; `on_change` fires the new text on each edit.", idea_ui::recipes::textarea_autogrow as fn() -> Element),
        ("ToastHost", "toast_host", "The mount point for transient notifications. Render exactly one `ToastHost` near the root; anywhere in the app, call `push_toast(message, tone)` to enqueue a toast and it appears at the host's `placement`. `dismiss_toast(id)` removes one early.", idea_ui::recipes::toast_host as fn() -> Element),
        ("Tooltip", "tooltip_hint", "A small hint that wraps its trigger and reveals itself on hover (desktop) or long-press (touch) — no host open-state signal. `text` is the hint shown in the bubble.", idea_ui::recipes::tooltip_hint as fn() -> Element),
        ("Typography", "typography_heading", "The standard way to put themed text on screen. `kind` picks the type role (H1…H6, Body, Caption, …) from the theme's scale; set `muted = true` for secondary text or `tone = Some(...)` for intent-colored text.", idea_ui::recipes::typography_heading as fn() -> Element),
    ];

    // Pass 1 — guard disarmed. Registers the sheets components build at
    // mount; the HTML is thrown away.
    eprintln!("[design-sync] pass 1: registering mount-built sheets");
    let _ = render_recipes(&recipes, true);
    // The manifest is built after this pass on purpose: `assembled_sheets()`
    // only contains a mount-built sheet once something has mounted it.

    // Sheet assembly and token install are world-backed (`signal()`) and
    // abort outside a reactive context. `host_mock::Harness` is the repo's
    // standard way to obtain one without a real backend.
    let harness = host_mock::Harness::new();
    let (tokens, manifest, components) = harness.world.enter(|| {
        idea_theme::install_idea_theme(light_theme());
        let tokens = tokens_css();
        // Manifest before the CSS dump: calling each `*_style()` accessor is
        // what forces the lazily-cached sheet to exist, and `dump_all_css`
        // walks the registry those calls populate.
        let manifest = manifest_json();
        let components = premint_dump::dump_all_css();
        (tokens, manifest, components)
    });

    let write = |name: &str, body: &str| {
        let path = std::path::Path::new(&out_dir).join(name);
        std::fs::write(&path, body).unwrap_or_else(|e| panic!("write {name}: {e}"));
        eprintln!("[design-sync] {name}: {} bytes", body.len());
    };
    write("tokens.css", &tokens);
    write("manifest.json", &manifest);
    write("components.css", &components);

    // Pass 2 — arm the guard from the asset we just wrote, then re-render.
    // Any "no CSS in the shipped asset" warning from here is a real hole.
    let classes = scan_minted_classes(&components);
    eprintln!("[design-sync] pass 2: {} minted classes armed", classes.len());
    backend_ssr::install_premint_classes(classes);
    let (rendered, head_css) = render_recipes(&recipes, false);
    write("recipes.json", &rendered);
    // The live-engine remainder — see `render_recipes`. Kept in its own
    // file rather than concatenated onto the preminted asset so the two
    // origins stay distinguishable; `styles.css` imports both.
    write("runtime.css", &head_css);
}
