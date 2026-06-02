//! Installation — adding idea-ui to a new project.

use runtime_core::{ui, Element};

use crate::shell::{self, Callout, CodePanel, ComponentPage, H2, P};

pub fn page() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Installation".to_string(),
            lead = "Add idea-ui to a fresh or existing idealyst app.".to_string(),
        ) {
            H2(content = "Add the crate".to_string())
            P(content = "idea-ui is a workspace member; depend on it from your app crate's \
                Cargo.toml the same way you depend on `runtime-core`.".to_string())
            CodePanel(src = r##"[dependencies]
runtime-core = { workspace = true }
idea-ui     = { workspace = true }"##.to_string())

            H2(content = "Install a theme before render".to_string())
            P(content = "Components' stylesheets read tokens from the active theme. The \
                framework panics on first render if no theme has been installed, so do it \
                at the top of `app()`.".to_string())
            CodePanel(src = r##"use runtime_core::{component, ui, Element};
use idea_ui::{install_idea_theme, light_theme};

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());
    ui! { /* your tree */ }
}"##.to_string())

            Callout(label = "Web extras".to_string()) {
                P(content = "On the web backend, also call `backend_web::install_viewport_observer()` \
                    after constructing the backend if you plan to use breakpoint signals \
                    (`current_breakpoint()`).".to_string())
            }

            H2(content = "Optional: theme-aware code blocks".to_string())
            P(content = "If you want syntax-highlighted code panels (these docs use one), add \
                the codeblock SDK and register it alongside other backend handlers.".to_string())
            CodePanel(src = r##"[dependencies]
codeblock = { workspace = true }

# On wasm32:
codeblock::register(&mut backend);"##.to_string())

            H2(content = "What's next".to_string())
            P(content = "Walk through First component (the next page) for a one-screen app, \
                or jump into the component reference from the sidebar.".to_string())
        }
    })
}
