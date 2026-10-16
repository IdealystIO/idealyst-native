//! Per-primitive shared surface: the prop/handle/Ops STRUCTS of each
//! primitive family.
//!
//! The builder fns (`icon(..)`, `text_input(..)`, `portal(..)`, …) live
//! in `runtime-vocabulary` (`glue::primitives`), which re-exports these
//! types under `runtime_core::primitives::*`. Ungated by design: these
//! types appear in element payloads, wire frames, and caps trait
//! signatures, so feature gates apply to builders and handlers, never
//! to the types.

pub mod activity_indicator;
pub mod flat_list;
pub mod graphics;
pub mod icon;
pub mod image;
pub mod key;
pub mod lazy;
pub mod link;
pub mod navigator;
pub mod overlay;
pub mod portal;
pub mod presence;
pub mod scroll_view;
pub mod slider;
pub mod text_area;
pub mod text_input;
pub mod toggle;
pub mod virtual_grid;
pub mod virtualizer;
