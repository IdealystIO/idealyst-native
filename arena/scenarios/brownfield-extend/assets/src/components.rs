//! Dashboard components. Each module exports one `#[component]`; the
//! `ui!`/`jsx!` trees invoke them by their PascalCase tag through the
//! `BuildElement` glue `#[component]` generates — no `#[macro_use]`
//! threading required.
//!
//! The composition graph the catalog records:
//!   Dashboard → Header → { Card, StatBadge }
//!   Dashboard → Toolbar → Card
//!   Dashboard → TaskList → TaskRow → Card

pub mod card;
pub mod dashboard;
pub mod header;
pub mod stat_badge;
pub mod task_list;
pub mod task_row;
pub mod toolbar;
