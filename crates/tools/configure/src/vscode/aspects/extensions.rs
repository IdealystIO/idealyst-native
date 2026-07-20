//! `extensions` — recommend the editor extensions an idealyst project wants:
//! rust-analyzer (types/expressions) and the idealyst DSL extension
//! (`ui!`/`jsx!` tag + prop completion, fed by `catalog-json`).

use std::path::Path;

use crate::vscode::aspect::{AspectContribution, VscodeAspect};

pub struct Extensions;

/// The idealyst VS Code extension's marketplace id (`<publisher>.<name>` from
/// `editors/vscode-idealyst/package.json`).
pub const IDEALYST_EXTENSION_ID: &str = "idealyst.vscode-idealyst";
pub const RUST_ANALYZER_ID: &str = "rust-lang.rust-analyzer";

impl VscodeAspect for Extensions {
    fn id(&self) -> &'static str {
        "extensions"
    }
    fn label(&self) -> &'static str {
        "Extension recommendations"
    }
    fn description(&self) -> &'static str {
        "Recommend rust-analyzer + the idealyst DSL extension in .vscode/extensions.json"
    }

    fn contribution(&self, _dir: &Path) -> AspectContribution {
        AspectContribution {
            recommendations: vec![RUST_ANALYZER_ID.into(), IDEALYST_EXTENSION_ID.into()],
            ..Default::default()
        }
    }
}
