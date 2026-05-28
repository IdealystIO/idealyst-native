//! `app()` — composes the welcome scene from the components in
//! [`crate::components`]. The coordinator's [`use_welcome`] hook
//! creates every ref + wires the animations; this function just
//! lays out the tree.

use runtime_core::{ui, Primitive};

use crate::components::content_layer::{ContentLayer, ContentLayerProps};
use crate::components::page::page_sheet;
use crate::components::planet::{Planet, PlanetProps};
use crate::components::sun_glare::{SunGlare, SunGlareProps};
use crate::components::vignette::{Vignette, VignetteProps};
use crate::coordinator::use_welcome;

pub fn app() -> Primitive {
    let refs = use_welcome();
    let page = page_sheet();
    ui! {
        View(style = page) {
            Vignette(refs = refs)
            SunGlare(refs = refs)
            for i in 0..3 {
                Planet(idx = i, refs = refs)
            }
            ContentLayer(refs = refs)
        }.bind(refs.page)
    }
}
