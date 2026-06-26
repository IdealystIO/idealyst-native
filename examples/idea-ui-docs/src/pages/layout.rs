//! Layout — Stack, Grid, Center (body-only pages).

use runtime_core::{ui, Element};
use idea_ui::{
    tone, typography_kind, variant, Badge, Center, Grid, Stack, StackAxis, StackGap, StackPadding,
    Surface, SurfaceColor, Typography,
};

use crate::shell::{Callout, CodePanel, DemoSurface, Prop, PropsTable, Section, H3, P};

// =============================================================================
// Stack
// =============================================================================

pub fn stack() -> Element {
    crate::pages::body(vec![
        ui! {
            Section(title = "Vertical (gap)".to_string()) {
                P(content = "The default axis is Column. `gap` reads the theme's spacing scale \
                    (None / Xs / Sm / Md / Lg / Xl / Xxl), so every Stack inherits the same \
                    vertical rhythm without per-call stylesheets.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Md) {
                        Badge(label = "one".to_string(),   tone = tone::Primary, variant = variant::Soft)
                        Badge(label = "two".to_string(),   tone = tone::Success, variant = variant::Soft)
                        Badge(label = "three".to_string(), tone = tone::Warning, variant = variant::Soft)
                    }
                }
                CodePanel(src = r##"Stack(gap = StackGap::Md) {
    Field(label = Some("Email".into()), value = email, on_change = on_email)
    Field(label = Some("Password".into()), value = pwd, on_change = on_pwd)
    Button(label = "Sign in".into(), on_click = submit, tone = tone::Primary, variant = variant::Filled)
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Horizontal (gap)".to_string()) {
                P(content = "Pass `axis = StackAxis::Row` for toolbars, button groups, and badge \
                    rows. The same `gap` scale spaces children along the main axis.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Badge(label = "one".to_string(),   tone = tone::Primary, variant = variant::Soft)
                        Badge(label = "two".to_string(),   tone = tone::Success, variant = variant::Soft)
                        Badge(label = "three".to_string(), tone = tone::Warning, variant = variant::Soft)
                    }
                }
                CodePanel(src = r##"Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
    Typography(content = "Title".into(), kind = typography_kind::H3)
    Spacer()
    Button(label = "Save".into(), on_click = save, tone = tone::Primary, variant = variant::Filled)
}"##.to_string())
            }
        },
        ui! {
            Section(title = "When to use Stack vs view".to_string()) {
                P(content = "Use `view` when you need raw flex with custom styling. Use Stack when \
                    the layout follows the theme's spacing scale — Stack reads gap and padding \
                    through tokens, so all your layouts inherit the same rhythm automatically.".to_string())
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "gap",
                        ty: "StackGap",
                        desc: "Spacing between children. None / Xs / Sm / Md / Lg / Xl / Xxl, mapped to spacing-* tokens.",
                    },
                    Prop {
                        name: "padding",
                        ty: "StackPadding",
                        desc: "Inner padding. Same scale as `gap`. Defaults to None.",
                    },
                    Prop {
                        name: "axis",
                        ty: "StackAxis",
                        desc: "Column (default) or Row. Toolbars and button groups use Row.",
                    },
                    Prop {
                        name: "align",
                        ty: "StackAlign",
                        desc: "Cross-axis alignment: Stretch / Start / Center / End.",
                    },
                    Prop {
                        name: "justify",
                        ty: "StackJustify",
                        desc: "Main-axis alignment: Start / Center / End / SpaceBetween / SpaceAround / SpaceEvenly.",
                    },
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Stack flattens fragment children via ChildList::append_to.",
                    },
                ])
            }
        },
    ])
}

// =============================================================================
// Grid
// =============================================================================

pub fn grid() -> Element {
    crate::pages::body(vec![
        ui! {
            Section(title = "Responsive grid".to_string()) {
                P(content = "Grid lays children out in `columns` equal-width columns, wrapping \
                    left-to-right into rows. There is no CSS grid under the hood — it chunks \
                    children into rows and flexes each cell equally (flex-grow: 1; flex-basis: 0).".to_string())
                DemoSurface {
                    Grid(columns = 4u32, gap = StackGap::Md) {
                        grid_cell("01")
                        grid_cell("02")
                        grid_cell("03")
                        grid_cell("04")
                        grid_cell("05")
                        grid_cell("06")
                        grid_cell("07")
                        grid_cell("08")
                    }
                }
                CodePanel(src = r##"Grid(columns = 3, gap = StackGap::Md) {
    Card { Typography(content = "Stat A".into(), kind = typography_kind::H3) }
    Card { Typography(content = "Stat B".into(), kind = typography_kind::H3) }
    Card { Typography(content = "Stat C".into(), kind = typography_kind::H3) }
    Card { Typography(content = "Stat D".into(), kind = typography_kind::H3) }  // wraps to row 2
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "columns",
                        ty: "u32",
                        desc: "Number of columns (>= 1, clamped). Default 2. Children flow left-to-right, wrapping into rows.",
                    },
                    Prop {
                        name: "gap",
                        ty: "StackGap",
                        desc: "Spacing between rows and between columns. Same spacing scale as Stack. Default Md.",
                    },
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Cells, laid out left-to-right then wrapping. Flattened via ChildList::append_to.",
                    },
                ])
            }
        },
        ui! {
            Callout(label = "Partial rows".to_string()) {
                P(content = "A partial final row's cells stretch to fill the row width rather than \
                    holding the same width as full rows — the conventional flex-grid behaviour. \
                    For a fixed cell width regardless of row fill, reach for a fixed-width child.".to_string())
            }
        },
    ])
}

fn grid_cell(label: &'static str) -> Element {
    ui! {
        Surface(background = SurfaceColor::SurfaceAlt, padding = StackPadding::Md) {
            Typography(content = label.to_string(), kind = typography_kind::H3)
        }
    }
}

// =============================================================================
// Center
// =============================================================================

pub fn center() -> Element {
    crate::pages::body(vec![
        ui! {
            Section(title = "Two-axis centering".to_string()) {
                P(content = "Center is the shorthand for the common case — placing a child in the \
                    middle of a region on both axes. Equivalent to a Stack with align: Center, \
                    justify: Center.".to_string())
                DemoSurface {
                    Center {
                        Badge(label = "Centered".to_string(), tone = tone::Primary, variant = variant::Soft)
                    }
                }
            }
        },
        ui! {
            Section(title = "Why it exists".to_string()) {
                P(content = "The shorthand exists so the common case (centering a loading spinner, \
                    an empty-state illustration) doesn't need a one-off stylesheet.".to_string())
                H3(content = "Recipe — empty state".to_string())
                CodePanel(src = r##"Center {
    Stack(gap = StackGap::Sm) {
        Icon(name = "inbox", size = 48.0)
        Typography(content = "No messages".into(), kind = typography_kind::H3)
        Typography(content = "You're all caught up.".into(), muted = true)
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Children to center on both axes. Incoming fragments are flattened into the centered container.",
                    },
                ])
            }
        },
    ])
}
