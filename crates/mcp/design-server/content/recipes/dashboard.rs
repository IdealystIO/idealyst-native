//! # Overview dashboard: stat tiles, needs-attention list, activity
//!
//! summary: The "what needs my attention?" screen — a responsive grid of stat tiles (1 → 2 → 4 columns), an actionable list with status badges, and recent activity, all inside Page.
//! tags: dashboard, overview, grid, stats, responsive, layout
//! render: / @phone => Overview, Open invoices, $12,480, Needs attention, Contoso, Recent activity
//! render: / @tablet => Overview, Active projects, Needs attention
//! render: / @desktop => Overview, Open invoices, Overdue, New customers, Needs attention, Fabrikam, Export
//!
//! Order is the design: numbers first (is anything off?), then the list of
//! things to act on, then history. Every tile answers a question the user
//! asks daily; a number nobody acts on doesn't get a tile.
//!
//! - **Tiles reflow by breakpoint** — 1 column on phones, 2 on tablets, 4
//!   from `Lg` — through one `rx!` on `Grid(columns)`. Tiles never shrink to
//!   unreadable widths to keep 4 in a row.
//! - **A tile is label → value → change**: an overline label, the number as
//!   H2, and its trend in the semantic tone (up-good is success, up-bad is
//!   danger — the tone follows the *meaning*, not the arrow).
//! - **Needs attention is a List with status badges**, each row opening its
//!   item. It's the actionable part of the screen, so it comes before
//!   activity.
//!
//! Depends on the `page_layout` recipe (`crate::page_layout`).

use crate::page_layout::{Page, Section};
use idea_ui::{
    tone, typography_kind, variant, Badge, Button, Card, CardPadding, Grid, List,
    ListItem, Stack, StackGap, ToneRef, Typography,
};
use runtime_core::{current_breakpoint, rx, ui, Breakpoint, Element};
use std::rc::Rc;

struct Stat {
    label: &'static str,
    value: &'static str,
    change: &'static str,
    /// Whether the change is good news — decides its tone.
    good: Option<bool>,
}

const STATS: &[Stat] = &[
    Stat { label: "Open invoices", value: "$12,480", change: "+8% this week", good: None },
    Stat { label: "Overdue", value: "3", change: "+2 since Monday", good: Some(false) },
    Stat { label: "Active projects", value: "14", change: "+1 this week", good: Some(true) },
    Stat { label: "New customers", value: "6", change: "Same as last week", good: None },
];

const ATTENTION: &[(&str, &str, &str)] = &[
    ("Contoso", "Invoice 1043 is 12 days overdue", "Overdue"),
    ("Fabrikam", "Contract renews in 5 days", "Renewal"),
];

const ACTIVITY: &[&str] = &["Northwind paid invoice 1039", "Ada added the Atlas project", "Grace invited 2 teammates"];

fn change_tone(good: Option<bool>) -> Option<ToneRef> {
    match good {
        Some(true) => Some(tone::Success.into()),
        Some(false) => Some(tone::Danger.into()),
        None => None,
    }
}

pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());
    let export: Rc<dyn Fn()> = Rc::new(|| {});
    let open: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Page(
            title = "Overview",
            subtitle = Some("This week at Acme.".to_string()),
            actions = Some(ui! { Button(label = "Export", on_click = export, tone = tone::Neutral, variant = variant::Outlined) }),
        ) {
            Grid(
                columns = rx!(match current_breakpoint().get() {
                    Breakpoint::Xs => 1u32,
                    Breakpoint::Sm | Breakpoint::Md => 2,
                    _ => 4,
                }),
                gap = StackGap::Md,
            ) {
                for s in STATS.iter() {
                    StatTile(label = s.label, value = s.value, change = s.change, change_tone = change_tone(s.good))
                }
            }
            Section(title = "Needs attention") {
                List {
                    for (who, what, status) in ATTENTION.iter().copied() {
                        ListItem(
                            label = format!("{who} — {what}"),
                            trailing = Some(ui! { Badge(label = status, tone = tone::Warning, variant = variant::Soft) }),
                            on_press = Some(open.clone()),
                        )
                    }
                }
            }
            Section(title = "Recent activity") {
                Card(padding = CardPadding::Md) {
                    Stack(gap = StackGap::Sm) {
                        for line in ACTIVITY.iter().copied() {
                            Typography(content = line, kind = typography_kind::BodySm)
                        }
                    }
                }
            }
        }
    }
}

/// Label → value → change.
#[runtime_core::component]
fn StatTile(
    label: String,
    value: String,
    change: String,
    #[prop(static, default = None)] change_tone: Option<ToneRef>,
) -> Element {
    ui! {
        Card(padding = CardPadding::Md) {
            Stack(gap = StackGap::Xs) {
                Typography(content = label, kind = typography_kind::Overline, muted = true)
                Typography(content = value, kind = typography_kind::H2)
                if let Some(t) = change_tone.clone() {
                    Typography(content = change.clone(), kind = typography_kind::BodySm, tone = Some(t))
                } else {
                    Typography(content = change.clone(), kind = typography_kind::BodySm, muted = true)
                }
            }
        }
    }
}
