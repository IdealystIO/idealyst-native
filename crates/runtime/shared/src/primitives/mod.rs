//! Per-primitive shared surface: the prop/handle/Ops STRUCTS of each
//! primitive family — the walker-free half of the old
//! `runtime_core::primitives`.
//!
//! The `Element`/`Bound` builder fns (`icon(..)`, `text_input(..)`,
//! `portal(..)`, …) stay in `runtime-core`'s same-named modules, which
//! wildcard-re-export these types so every old path keeps resolving.
//! Ungated by design: these types appear in `Element` payloads, wire
//! frames, and `Backend`/caps trait signatures, so the old core's
//! `prim-*` features gate the *builders and dispatchers*, never the
//! types.

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
