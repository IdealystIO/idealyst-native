//! # Page anatomy: Page, PageHeader and Section
//!
//! summary: The house page frame every screen sits in — scroll container, capped readable width, header with title/subtitle/actions, titled sections.
//! tags: layout, page, header, sections, spacing
//! render: / @phone => Billing, Change plan, Invoices
//! render: / @desktop => Billing, Plan and invoices for your workspace, Change plan, Current plan, Invoices, Download the PDF
//!
//! Every screen in an idea-ui app should be a `Page`. Copy this module into
//! the app as `src/page_layout.rs` — the other screen recipes `use
//! crate::page_layout::{…}` from exactly that path.
//!
//! What the frame decides, so screens don't re-decide it:
//! - **One scroll container per page**, filling the navigator outlet. Screens
//!   never add their own outer `scroll_view`.
//! - **Readable width.** Content is capped (`PageWidth::Regular` 1120,
//!   `Narrow` 720 for forms and settings) and centered; full-bleed only for
//!   tables and canvases (`Wide`).
//! - **Gutter follows the breakpoint**: `spacing.md` on phones, `spacing.xl`
//!   from `md` up. Never a pixel number in a screen.
//! - **Header**: exactly one H1 per screen (the page title), an optional
//!   muted subtitle, and the page's actions on the right — the primary
//!   action last, so it sits at the trailing edge. Actions wrap under the
//!   title on narrow screens instead of squeezing it.
//! - **One visible title.** A screen pushed onto a stack shows its title in
//!   the stack bar, so it renders `Page(header = false)` (see the
//!   `stack_list_detail` recipe).
//! - **Rhythm**: `spacing.xl` between sections, `spacing.md` inside one.
//!   Sections are titled with H3 (H2 is reserved for a page with a second
//!   level of grouping).

use idea_ui::{
    tone, typography_kind, variant, Button, Card, CardPadding, IdeaThemeRef, Stack, StackGap,
    Typography,
};
use runtime_core::{
    component, stylesheet, ui, AlignItems, AlignSelf, Element, FlexDirection, FlexWrap,
    JustifyContent, Length,
};
use std::rc::Rc;

/// How wide a page's content may grow.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum PageWidth {
    /// Forms, settings, single-column reading: 720.
    Narrow,
    /// The default for dashboards, lists and detail pages: 1120.
    #[default]
    Regular,
    /// Tables, boards and canvases that use every pixel.
    Wide,
}

stylesheet! {
    /// The page's one scroll container: fills the navigator outlet.
    PageScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
            width: Length::pct(100.0),
            background: t.color.background(),
        }
    }
}

stylesheet! {
    /// The centered, width-capped content column.
    PageBody<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            align_self: AlignSelf::Center,
            gap: t.spacing.xl(),
            padding: t.spacing.md(),
        }
        variant width {
            narrow(_t) { max_width: Length::Px(720.0) }
            #[default]
            regular(_t) { max_width: Length::Px(1120.0) }
            wide(_t) {}
        }
        breakpoint md(t) {
            padding: t.spacing.xl(),
        }
    }
}

stylesheet! {
    /// Title block on the left, actions on the right; wraps on narrow
    /// screens so the title keeps its full width.
    PageHeaderRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::FlexEnd,
            justify_content: JustifyContent::SpaceBetween,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    /// The title column grows; it is what wraps last.
    PageTitleBlock<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            flex_grow: 1.0,
            min_width: Length::Px(240.0),
        }
    }
}

stylesheet! {
    /// The action cluster: a row with the primary action last.
    PageActions<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

/// A screen's frame: scroll container, width cap, header, then the
/// screen's sections as children.
#[component]
pub fn Page(
    /// The page title — the screen's only H1.
    title: String,
    /// One muted line that says what the page is for.
    #[prop(default = None)]
    subtitle: Option<String>,
    /// The page's actions, primary last (a `Stack` row of Buttons, or one Button).
    #[prop(static, default = None)]
    actions: Option<Element>,
    /// The readable-width cap.
    #[prop(static, default = PageWidth::Regular)]
    width: PageWidth,
    /// `false` on a screen pushed onto a stack: the stack bar already shows
    /// its title (and its one action), and a page shows exactly one title.
    #[prop(static, default = true)]
    header: bool,
    children: Vec<Element>,
) -> Element {
    let body = PageBody().width(match width {
        PageWidth::Narrow => PageBodyWidth::Narrow,
        PageWidth::Regular => PageBodyWidth::Regular,
        PageWidth::Wide => PageBodyWidth::Wide,
    });
    ui! {
        scroll_view(style = PageScroll()) {
            view(style = body) {
                if header {
                    PageHeader(title = title, subtitle = subtitle, actions = actions)
                }
                children
            }
        }
    }
}

/// Title, subtitle and actions. Used by `Page`; use it directly only for a
/// page-like region that is not a whole screen (a modal's full-screen form).
#[component]
pub fn PageHeader(
    title: String,
    #[prop(default = None)]
    subtitle: Option<String>,
    #[prop(static, default = None)]
    actions: Option<Element>,
) -> Element {
    ui! {
        view(style = PageHeaderRow()) {
            view(style = PageTitleBlock()) {
                Typography(content = title, kind = typography_kind::H1)
                if let Some(sub) = subtitle.get() {
                    Typography(content = sub, muted = true)
                }
            }
            if let Some(actions) = actions {
                view(style = PageActions()) { actions }
            }
        }
    }
}

/// A titled group of related content. The title says what the group is;
/// the optional description says why it matters or what changing it does.
#[component]
pub fn Section(
    title: String,
    #[prop(default = None)]
    description: Option<String>,
    children: Vec<Element>,
) -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Stack(gap = StackGap::Xs) {
                Typography(content = title, kind = typography_kind::H3)
                if let Some(desc) = description.get() {
                    Typography(content = desc, muted = true, kind = typography_kind::BodySm)
                }
            }
            children
        }
    }
}

/// A demo screen in the frame.
pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());
    let change_plan: Rc<dyn Fn()> = Rc::new(|| {});
    let contact: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Page(
            title = "Billing",
            subtitle = Some("Plan and invoices for your workspace.".to_string()),
            actions = Some(ui! {
                Button(label = "Contact sales", on_click = contact, tone = tone::Neutral, variant = variant::Outlined)
                Button(label = "Change plan", on_click = change_plan)
            }),
        ) {
            Section(title = "Current plan") {
                Card(padding = CardPadding::Md) {
                    Typography(content = "Team — 12 seats", kind = typography_kind::BodyLg)
                    Typography(content = "Renews on the 1st of every month.", muted = true)
                }
            }
            Section(
                title = "Invoices",
                description = Some("Download the PDF for any past month.".to_string()),
            ) {
                Card(padding = CardPadding::Md) {
                    Typography(content = "No invoices yet.", muted = true)
                }
            }
        }
    }
}
