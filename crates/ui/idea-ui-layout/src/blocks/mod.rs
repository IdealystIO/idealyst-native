//! The blocks any layout holds. Each owns the phone form of one
//! arrangement, so a screen writes no breakpoint overlay for gutters,
//! gaps, stacking or column counts (rule `layout`).

mod action_bar;
mod field_grid;
mod label_value;
mod section;
mod stat_grid;
mod state_message;

pub use action_bar::{ActionBar, ActionBarProps, ACTION_BAR_MAX_SECONDARY, ACTION_BAR_NOTE_MIN};
pub use field_grid::{FieldGrid, FieldGridProps, FieldGridRow, FieldGridRowProps};
pub use label_value::{LabelValue, LabelValueProps, LABEL_WIDTH, LABEL_WIDTH_WIDE};
pub use section::{Section, SectionHeading, SectionHeadingProps, SectionProps};
pub use stat_grid::{StatGrid, StatGridProps};
pub use state_message::{StateMessage, StateMessageProps};
