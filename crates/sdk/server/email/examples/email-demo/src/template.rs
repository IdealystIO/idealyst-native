//! The email template — an idealyst component, authored exactly like any UI
//! screen, but rendered by `backend-email` into email-safe HTML. It takes
//! input props (`name`, `cta_url`) and composes `idea-ui-mail` components.

use idea_ui_mail::{Button, Divider, EmailBody, EmailContainer, Heading, Section, Text};
use runtime_core::{component, ui, Element, IdealystSchema, Reactive};

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct WelcomeEmailProps {
    /// The recipient's display name.
    pub name: String,
    /// Where the primary call-to-action button points.
    pub cta_url: String,
}

impl Default for WelcomeEmailProps {
    fn default() -> Self {
        Self {
            name: Reactive::Static("there".into()),
            cta_url: Reactive::Static("https://idealyst.dev".into()),
        }
    }
}

/// A welcome email, parameterized by the recipient's name and a CTA link.
#[component]
pub fn WelcomeEmail(props: &WelcomeEmailProps) -> Element {
    ui! {
        EmailBody(background = "#f4f4f7") {
            EmailContainer(width = 600.0) {
                Section(padding = 40.0, gap = 20.0) {
                    Heading(content = "Welcome to Idealyst 🎉", center = true)
                    Text(
                        content = format!(
                            "Hi {}, thanks for joining. Your account is ready — one author \
                             tree, every platform, and now your transactional email too.",
                            props.name.get()
                        ),
                    )
                    Button(label = "Open your dashboard", href = props.cta_url.get())
                    Divider()
                    Text(
                        content = "This email was rendered from idealyst components with no \
                                   WASM — the same primitives your app uses, emitted as \
                                   inline-styled, email-safe HTML.".to_string(),
                        size = 14.0,
                        color = "#98a2b3",
                    )
                }
            }
        }
    }
}
