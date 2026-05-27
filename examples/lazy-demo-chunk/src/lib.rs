//! `lazy-demo-chunk` — the chunk side of the lazy-primitive demo.
//!
//! Defines `ChunkProps` (shared with the parent demo crate via the
//! same `serde::Serialize` shape) and exposes `app(props)` that
//! returns a UI rendering the props' greeting.
//!
//! On native (terminal / iOS / macOS / Android) the parent crate
//! takes a normal cargo dep on this crate, registers a thunk via
//! `runtime_core::primitives::lazy::register`, and the framework's
//! walker dispatches inline when it hits the `Primitive::Lazy`. On
//! web (PR 6) this crate will be built as a separate wasm bundle
//! and loaded dynamically; the `Serialize` bound on `ChunkProps` is
//! what makes the cross-bundle prop transport possible.

use runtime_core::{ui, Primitive};

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ChunkProps {
    pub greeting: String,
    pub multiplier: u32,
}

pub fn app(props: ChunkProps) -> Primitive {
    let line = format!(
        "[chunk says] {}  (multiplier = {})",
        props.greeting, props.multiplier,
    );
    let attribution = "(rendered by lazy-demo-chunk::app)";
    ui! {
        View {
            Text { line }
            Text { attribution }
        }
    }
}
