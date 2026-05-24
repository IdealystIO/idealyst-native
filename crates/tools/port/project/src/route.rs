//! Frontend dispatch.
//!
//! Decides which `port-*` crate to send a given source file to.
//! Extension is the primary signal; for `.tsx`/`.jsx` the content
//! is sniffed to disambiguate React vs. Solid (a `solid-js`
//! import is the canonical tell).

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frontend {
    React,
    Solid,
    Vue,
    Svelte,
}

impl Frontend {
    pub fn label(&self) -> &'static str {
        match self {
            Frontend::React => "react",
            Frontend::Solid => "solid",
            Frontend::Vue => "vue",
            Frontend::Svelte => "svelte",
        }
    }
}

pub fn detect(path: &Path, content: &str) -> Option<Frontend> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "tsx" | "jsx" => Some(if looks_like_solid(content) {
            Frontend::Solid
        } else {
            Frontend::React
        }),
        "vue" => Some(Frontend::Vue),
        "svelte" => Some(Frontend::Svelte),
        _ => None,
    }
}

fn looks_like_solid(content: &str) -> bool {
    // Solid is identified by an import from `solid-js` or its
    // submodules. We keep the check substring-based rather than
    // full parsing — wrong-but-tolerable misclassification just
    // routes a file to the wrong frontend, which surfaces in the
    // report as a parse failure the user can review.
    content.contains("from \"solid-js\"")
        || content.contains("from 'solid-js'")
        || content.contains("from \"solid-js/")
        || content.contains("from 'solid-js/")
}
