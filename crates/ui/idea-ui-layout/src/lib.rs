//! `idea-ui-layout` — responsive screen layouts for idea-ui.
//!
//! idea-ui decides what a control looks like. This kit decides how a
//! screen is put together at each width: a screen picks a layout and
//! hands it content, and the layout draws the phone, tablet and desktop
//! form. A screen built from it never reads the breakpoint.
//!
//! The rules the kit holds to are in `RULES.md` beside this crate, each
//! under a name the docs here cite (rule `layout`, rule `overlays`). How
//! to build a screen with it is the `layouts` guide
//! (`crates/mcp/catalog/guides/layouts.md`).
//!
//! # The ladder
//!
//! | | Phone (`Xs`, under 640) | Tablet (`Sm`, `Md`) | Desktop (`Lg` up) |
//! |---|---|---|---|
//! | Content | phone layout | desktop layout | desktop layout |
//! | App sidebar ([`AppFrame`]) | drawer | drawer | pinned |
//! | A pane beside the content | never | from the layout's width | yes |
//! | Menus and popovers | bottom sheet | anchored | anchored |
//! | Form dialogs | bottom sheet | card | card |
//!
//! # The layouts
//!
//! - [`AppFrame`] — the sidebar and the screen beside it, in one of two
//!   looks ([`FrameSurface`]): `Flat` (one surface, parted by a rule) or
//!   `Raised` (the screen on a card, the sidebar on the ground behind).
//! - [`Page`] — the frame: the header pinned, and the body in one of
//!   three seats ([`PageVariant`]): `Content` (the page scrolls),
//!   `Table` (the table in it scrolls), `Blank` (bounded, nothing else
//!   decided). [`PageHeader`] is its title row.
//! - [`SplitView`] — two regions side by side from one width per mode
//!   ([`SplitMode`]): `Nav`, `Detail`, `Inspector`.
//! - [`Flow`] — one step at a time, its actions pinned at the foot.
//!
//! # The header and the toolbar
//!
//! - [`PageHeader`] — the title row, as slots a screen fills: the way
//!   back (a trail on the web, an arrow on iOS and Android), the title,
//!   the record's menu, a status, and the verbs.
//! - [`Toolbar`] — the row under it: `start` for [`ToolbarTabs`], `end`
//!   for [`ToolbarSearch`] and [`ToolbarButton`]s. The controls wrap
//!   under the tabs by themselves when the row is tight.
//!
//! # The blocks
//!
//! [`Section`], [`FieldGrid`], [`LabelValue`], [`StatGrid`],
//! [`ActionBar`], [`StateMessage`] and [`ListStates`] go inside any
//! layout. Each owns the phone form of one arrangement.
//!
//! # The overlays
//!
//! [`ActionMenu`], [`AdaptivePopover`], [`DrillSheet`] and [`Dialog`]
//! are anchored or centred with a pointer and a bottom [`Sheet`] on a
//! phone; see the [`overlay`] module.

mod action;
mod app_frame;
pub mod blocks;
mod flow;
mod list_states;
pub mod overlay;
pub mod page;
mod slot;
mod split_view;
mod theme;
pub mod toolbar;
mod width;

pub use action::Action;
pub use app_frame::{
    AppFrame, AppFrameProps, FrameSurface, SidebarCollapse, SidebarItem, SidebarItemProps, SidebarState, SidebarToggle,
    SidebarToggleProps, SIDEBAR_RAIL_WIDTH, SIDEBAR_WIDTH,
};
pub use blocks::{
    ActionBar, ActionBarProps, BottomInset, FieldGrid, FieldGridProps, FieldGridRow, FieldGridRowProps, LabelValue,
    LabelValueProps, Section, SectionHeading, SectionHeadingProps, SectionProps, StatGrid, StatGridProps,
    StateMessage, StateMessageProps,
};
pub use flow::{Flow, FlowLabels, FlowProps, FlowStep};
pub use list_states::{Blank, ListPhase, ListStates, ListStatesProps};
pub use overlay::{
    dialog_block, dialog_presentation, sheet_serves, ActionMenu, ActionMenuProps, ActionMenuSurface,
    ActionMenuSurfaceProps, AdaptivePopover, AdaptivePopoverProps, Dialog, DialogFooter, DialogFooterProps,
    DialogProps, DrillCx, DrillSheet, DrillSheetProps, Sheet, SheetActionRow, SheetActionRowProps, SheetCheckRow,
    SheetCheckRowProps, SheetHeading, SheetHeadingProps, SheetProps, SheetPushRow, SheetPushRowProps,
};
pub use page::{
    HeaderBackButton, HeaderBackButtonProps, HeaderTrail, HeaderTrailProps, HeaderWayBack, HeaderWayBackProps, WayBack,
};
pub use page::{Page, PageFrame, PageHeader, PageHeaderProps, PageProps, PageVariant, PinnedPane, PinnedPaneProps};
pub use slot::{OptionalSlot, Slot};
pub use split_view::{
    SplitMode, SplitView, SplitViewProps, SPLIT_DOCK_MAX_PCT, SPLIT_NAV_WIDTH, SPLIT_PANE_WIDTH,
};
pub use theme::{Gutter, LayoutTheme};
pub use toolbar::{
    Toolbar, ToolbarButton, ToolbarButtonProps, ToolbarProps, ToolbarSearch, ToolbarSearchProps, ToolbarTabs,
    ToolbarTabsProps, TOOLBAR_START_MIN,
};
pub use width::{form_factor, is_phone, opens_on_phone, FormFactor};
