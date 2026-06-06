//! The complete [Lucide](https://lucide.dev) icon pack for the
//! idealyst-native framework — every icon in the upstream set (see
//! [`ICON_COUNT`]), each exposed as a `pub const IconData`.
//!
//! Icons are generated at build time from the SVGs in the `assets/`
//! directory. Because each icon is a `const`, **only the icons your
//! application actually imports end up in the final binary** — LTO drops
//! the thousands you don't reference, so depending on the full pack costs
//! nothing at runtime.
//!
//! # Usage
//!
//! ```ignore
//! use icons_lucide::{SEARCH, MENU, X};
//! use runtime_core::icon;
//!
//! icon(SEARCH)
//! icon(MENU).color(|| theme.primary())
//! ```
//!
//! The constant name is the icon's kebab-case Lucide name in
//! `SCREAMING_SNAKE_CASE` (`arrow-right` → `ARROW_RIGHT`, `trash-2` →
//! `TRASH_2`). Browse names at <https://lucide.dev/icons/>.
//!
//! # Updating the pack
//!
//! The `assets/` directory mirrors `lucide-static`'s `icons/` folder
//! verbatim — to refresh, `npm pack lucide-static`, extract, and copy the
//! SVGs over. `build.rs` normalizes every drawable SVG element (`path`,
//! `circle`, `ellipse`, `line`, `polyline`, `polygon`, `rect`) into path
//! `d` data, so raw upstream SVGs drop straight in with no preprocessing.
//!
//! Lucide is distributed under the ISC license; see `LICENSE-LUCIDE`.

/// A named icon — the unit of the [`ALL`] registry.
///
/// Only compiled with the `registry` feature. Use the per-icon `const`s
/// (`SEARCH`, `MENU`, …) for normal app code; `IconEntry`/[`ALL`] exist for
/// tools that need to enumerate the whole set (galleries, pickers, docs).
#[cfg(feature = "registry")]
pub struct IconEntry {
    /// The icon's Lucide kebab-case name, e.g. `"arrow-right"`.
    pub name: &'static str,
    /// The icon geometry. Same value as the matching `const` (`ARROW_RIGHT`).
    pub data: runtime_core::IconData,
}

include!(concat!(env!("OUT_DIR"), "/icons_generated.rs"));

/// Self-register this pack into the MCP catalog's `IconSetEntry` slice so
/// it's discoverable through the MCP `list_icon_sets` / `search_icons`
/// tools and renders in the docs site's icon gallery. An icon pack is the
/// open extension point for this slice: it submits one entry pointing at
/// the build-generated [`ICON_REFS`] name table — the same self-registration
/// pattern `#[component]` uses. Gated behind `catalog` (off by default) so
/// referencing `ICON_REFS` doesn't defeat per-`const` tree-shaking for
/// normal apps.
#[cfg(feature = "catalog")]
mcp_catalog::inventory::submit! {
    mcp_catalog::IconSetEntry {
        name: "icons-lucide",
        title: "Lucide",
        docs: "Lucide — the community-maintained fork of Feather. ~1600 \
               outlined (stroke-only) icons. Reference an icon by its \
               SCREAMING_SNAKE_CASE constant (`icons_lucide::ARROW_RIGHT`) \
               in the `icon(...)` primitive; only icons you import end up \
               in the binary.",
        import_path: "icons_lucide",
        license: "ISC",
        homepage: "https://lucide.dev",
        icons: ICON_REFS,
    }
}
