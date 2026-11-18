//! [`Page`] — the frame every screen stands in — and what sits in it:
//! the [`PageHeader`], a [`PageFrame`] and a [`PinnedPane`].

mod frame;
mod header;
mod page;
mod pinned_pane;

pub use frame::PageFrame;
pub use header::{PageHeader, PageHeaderProps, HEADER_MAX_ACTIONS};
pub use page::{Page, PageProps, PageVariant};
pub use pinned_pane::{PinnedPane, PinnedPaneProps};

pub(crate) use frame::watch_height;
pub(crate) use pinned_pane::pin_rules;
