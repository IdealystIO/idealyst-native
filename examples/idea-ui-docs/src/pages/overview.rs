//! Overview — landing page. What idea-ui is, the high-level
//! design (theme-as-trait, intent-as-trait), and links via the
//! sidebar to each category.

use runtime_core::{ui, Primitive};
use idea_ui::{Typography, Card, Stack, StackGap};

use crate::shell::page_header;

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "idea-ui",
                "A cross-platform component library built on the idealyst framework. \
                 These docs are themselves built with idea-ui — every control panel, \
                 every nav link, every overlay. The library documents itself."
            ) }

            Card {
                Typography(content = "Theme is a trait".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(content = "idea-ui doesn't ship a concrete `Theme` struct. The theme is a \
                                  trait every component's stylesheets read from. Apps that need \
                                  more fields than the built-in defaults implement the trait on \
                                  their own struct and pass it through `install_idea_theme(...)`. \
                                  Dark/light mode just swaps which struct is installed.".to_string(),
                     muted = true)
            }

            Card {
                Typography(content = "Intent is a global vocabulary".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(content = "`Primary`, `Secondary`, `Neutral`, `Ghost`, `Success`, `Warning`, \
                                  `Danger` — and any custom intent your app defines — are shared \
                                  across every themed component. Define a new intent once, and \
                                  it works in Pressable, Badge, Alert, Tag, Avatar, IconButton, \
                                  and any future intent-aware component.".to_string(),
                     muted = true)
            }

            Card {
                Typography(content = "Live, interactive docs".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(content = "Every component's page below has a live preview alongside a \
                                  control panel built from idea-ui itself. Twiddle the controls; \
                                  the preview updates in place. Where the type system can \
                                  reflect on a Props struct, the control panel is generated \
                                  automatically via the `DocControls` derive.".to_string(),
                     muted = true)
            }
        }
    }
}
