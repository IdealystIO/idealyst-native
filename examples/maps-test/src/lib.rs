//! Smoke test for the third-party External primitive pipeline.
//!
//! Demonstrates:
//! - Framework-core's `Primitive::External` mounting through the
//!   `WebBackend` registry.
//! - The `maps` umbrella crate's cfg-routed `register(...)` selecting
//!   the web leaf at compile time.
//! - End-to-end render: `MapView(MapViewProps { ... })` produces an
//!   `<iframe>` embedding OpenStreetMap, mounted into the DOM tree
//!   alongside ordinary framework primitives via the `ui!` macro.

#[cfg(target_arch = "wasm32")]
mod web;

use framework_core::{ui, Primitive};
use framework_theme::{install_theme, ThemeTokens};
use maps::{MapView, MapViewProps};

/// Trivial theme so `install_theme` is satisfied (framework requires
/// it before render, even when no theme tokens are read).
#[derive(Clone)]
pub struct Theme {}

impl ThemeTokens for Theme {
    fn tokens(&self) -> Vec<framework_core::TokenEntry> {
        Vec::new()
    }
}

/// The shared app — built once, rendered by `web.rs` on wasm. Three
/// `MapView` calls demonstrate that the registry dispatches correctly
/// per payload TypeId and that multiple instances of the same
/// external kind coexist.
///
/// Third-party externals don't get native `ui!` block syntax (the
/// macro only recognizes the first-party primitive set), so `MapView`
/// is interpolated as an expression with `{ MapView(...) }`. The
/// PascalCase name reads identically to a first-party `Overlay { }`
/// at the call site — same visual cadence, one extra brace pair.
/// The `unused_braces` lint fires post-macro-expansion because rustc
/// can't see that the braces disambiguated tag-vs-expression at the
/// `ui!` parse level.
#[allow(unused_braces)]
pub fn app() -> Primitive {
    let cities = [
        ("San Francisco", 37.7749, -122.4194, 11.0f32),
        ("Tokyo", 35.6762, 139.6503, 11.0),
        ("Reykjavík", 64.1466, -21.9426, 11.0),
    ];

    ui! {
        View {
            Text { "External primitive smoke test" }
            Text {
                "Each map below is a MapView(...) call lowered to \
                 Primitive::External, dispatched through WebBackend's \
                 ExternalRegistry to the maps-web leaf, which builds \
                 an OpenStreetMap iframe."
            }
            for (label, lat, lon, zoom) in cities {
                { MapView(MapViewProps { lat, lon, zoom }) }
                Text { { format!("{label} — {lat:.4}, {lon:.4}") } }
            }
        }
    }
}

pub fn install() {
    install_theme(Theme {});
}
