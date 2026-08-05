//! Serverless contact form — submit a form, store it in DynamoDB, email a
//! notification. One `#[server]` fn (`submit_contact`) runs on the client as a
//! typed RPC stub and on the server (here, an AWS Lambda) as the real body.
//!
//! - **Client** (default features): `app()` renders three fields + a submit
//!   button. Submitting calls `submit_contact(..)`, which POSTs to
//!   `/_srv/submit_contact`.
//! - **Server** (`--features server`): the body validates, writes a row to
//!   DynamoDB, and sends an email via SES. Hosted as a Lambda by
//!   `src/bin/lambda.rs` (`server_aws::run()`), or locally by `src/bin/local.rs`
//!   (`server::serve()`).
//!
//! Set `CONTACT_DRY_RUN=1` on the server build to skip the AWS calls and just
//! log — useful for exercising the round-trip locally without credentials.

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};

use runtime_core::{
    mutation, signal, ui, Element, FlexDirection, IntoElement, Length, Mutation, Signal,
    StyleRules, StyleSheet,
};
use serde::{Deserialize, Serialize};
use server::{server, ServerError};
use std::rc::Rc;

// ============================================================================
// Wire type — shared between server and client.
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactSubmission {
    pub name: String,
    pub email: String,
    pub message: String,
}

// ============================================================================
// The server function. One definition, two compilations:
//   - client build  → typed stub POSTing to /_srv/submit_contact
//   - server build  → the real body below (DynamoDB + SES)
// On success it returns a confirmation id (the DynamoDB row key) so the
// client can show the user a reference.
// ============================================================================

#[server]
pub async fn submit_contact(input: ContactSubmission) -> Result<String, ServerError> {
    // Cheap validation runs on the server regardless of any client checks —
    // the client stub is bypassable, so the server is the source of truth.
    if input.name.trim().is_empty() || input.message.trim().is_empty() {
        return Err(ServerError::failed("name and message are required"));
    }
    if !input.email.contains('@') {
        return Err(ServerError::failed("a valid email is required"));
    }

    backend::handle_submission(input)
        .await
        .map_err(|e| ServerError::failed(e.to_string()))
}

// ============================================================================
// Server-only backend: DynamoDB write + SES email.
//
// Gated behind `feature = "server"` so the wasm/client build never compiles
// the AWS SDK or tokio. The AWS clients are built once per execution
// environment (cold start) and reused across warm invocations via a
// `OnceCell` — re-building them per request would add hundreds of ms.
// ============================================================================

#[cfg(feature = "server")]
mod backend {
    use super::ContactSubmission;
    use std::error::Error;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::OnceCell;

    type BoxErr = Box<dyn Error + Send + Sync>;

    /// Shared AWS config (region + credentials from the Lambda execution role /
    /// the local environment). Loaded once, then reused by both clients.
    static CONFIG: OnceCell<aws_config::SdkConfig> = OnceCell::const_new();
    static DDB: OnceCell<aws_sdk_dynamodb::Client> = OnceCell::const_new();
    static SES: OnceCell<aws_sdk_sesv2::Client> = OnceCell::const_new();

    async fn config() -> &'static aws_config::SdkConfig {
        CONFIG
            .get_or_init(|| async {
                aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await
            })
            .await
    }

    async fn dynamo() -> &'static aws_sdk_dynamodb::Client {
        DDB.get_or_init(|| async { aws_sdk_dynamodb::Client::new(config().await) })
            .await
    }

    async fn ses() -> &'static aws_sdk_sesv2::Client {
        SES.get_or_init(|| async { aws_sdk_sesv2::Client::new(config().await) })
            .await
    }

    fn epoch_millis() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    /// Store the submission, then notify. Returns the new row's id.
    pub async fn handle_submission(input: ContactSubmission) -> Result<String, BoxErr> {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = epoch_millis().to_string();

        // `CONTACT_DRY_RUN=1` short-circuits the AWS calls so the round-trip can
        // be exercised locally with no credentials / no provisioned resources.
        if dry_run() {
            eprintln!(
                "[dry-run] would store + notify: id={id} name={:?} email={:?} msg={:?}",
                input.name, input.email, input.message
            );
            return Ok(id);
        }

        store(&id, &created_at, &input).await?;
        notify(&input).await?;
        Ok(id)
    }

    fn dry_run() -> bool {
        matches!(
            std::env::var("CONTACT_DRY_RUN").ok().as_deref(),
            Some("1") | Some("true")
        )
    }

    async fn store(id: &str, created_at: &str, input: &ContactSubmission) -> Result<(), BoxErr> {
        use aws_sdk_dynamodb::types::AttributeValue;
        let table = std::env::var("CONTACT_TABLE")
            .map_err(|_| "CONTACT_TABLE env var is required (DynamoDB table name)")?;

        dynamo()
            .await
            .put_item()
            .table_name(table)
            .item("id", AttributeValue::S(id.to_string()))
            .item("created_at", AttributeValue::S(created_at.to_string()))
            .item("name", AttributeValue::S(input.name.clone()))
            .item("email", AttributeValue::S(input.email.clone()))
            .item("message", AttributeValue::S(input.message.clone()))
            .send()
            .await?;
        Ok(())
    }

    async fn notify(input: &ContactSubmission) -> Result<(), BoxErr> {
        use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message};
        let from = std::env::var("CONTACT_FROM")
            .map_err(|_| "CONTACT_FROM env var is required (verified SES sender)")?;
        let to = std::env::var("CONTACT_TO")
            .map_err(|_| "CONTACT_TO env var is required (notification recipient)")?;

        let subject = Content::builder()
            .data(format!("New contact form submission from {}", input.name))
            .build()?;
        let text_body = Content::builder()
            .data(format!(
                "Name: {}\nEmail: {}\n\n{}",
                input.name, input.email, input.message
            ))
            .build()?;
        let message = Message::builder()
            .subject(subject)
            .body(Body::builder().text(text_body).build())
            .build();

        ses()
            .await
            .send_email()
            .from_email_address(from)
            .destination(Destination::builder().to_addresses(to).build())
            .content(EmailContent::builder().simple(message).build())
            .send()
            .await?;
        Ok(())
    }
}

// ============================================================================
// Client: point the SDK at the API host, then render the form.
// ============================================================================

fn configure_server() {
    // Production split (static site + Lambda on different origins): bake the
    // Function URL / API Gateway base in at build time via CONTACT_API_BASE.
    // Falls back to same-origin, which is what `src/bin/local.rs` serves.
    #[cfg(target_arch = "wasm32")]
    {
        let base = option_env!("CONTACT_API_BASE")
            .map(|s| s.to_string())
            .or_else(|| {
                web_sys::window().and_then(|w| w.location().origin().ok())
            })
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        server::configure(server::ClientConfig::new(base));
    }

    #[cfg(all(not(feature = "server"), not(target_arch = "wasm32")))]
    {
        let base = option_env!("CONTACT_API_BASE").unwrap_or("http://127.0.0.1:3000");
        server::configure(server::ClientConfig::new(base));
    }
}

/// SDK-handler registration seam, invoked by the CLI-generated wrappers after
/// `runtime_vocabulary::register_builtins`. Registry-generic over the scene
/// `Host` so ONE seam serves every backend. This app registers no third-party
/// scene handlers — but the seam is mandatory: an unregistered payload panics
/// at realize.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

/// Android entry: the generated wrapper's `attach` mounts `scene_app()`
/// through `backend_android::newcore::start`.
pub fn scene_app() -> Element {
    app()
}

// ============================================================================
// `app()` — the idealyst UI: three fields + submit, with a live status line.
// ============================================================================

pub fn app() -> Element {
    install_idea_theme(light_theme());
    configure_server();

    let name: Signal<String> = signal(String::new());
    let email: Signal<String> = signal(String::new());
    let message: Signal<String> = signal(String::new());

    // Submit folds the server's confirmation id into a status signal; the
    // mutation's own lifecycle (loading/error) is projected separately below.
    // The handler writes `status` on success, so the response lands in local
    // state with no second round-trip. `Signal` is `Copy` and routes to its own
    // world, so the future captures it directly.
    let status: Signal<String> = signal("Fill in the form and submit.".to_string());
    let submit: Mutation<ContactSubmission, (), ServerError> = mutation(move |input| async move {
        let confirmation = submit_contact(input).await?;
        status.set(format!("Thanks — your message was sent (ref {confirmation})."));
        Ok(())
    });

    let on_submit = {
        let submit = submit.clone();
        move || {
            submit.trigger(ContactSubmission {
                name: name.get(),
                email: email.get(),
                message: message.get(),
            });
        }
    };

    // Two status lines, each reading a single reactive source — kept apart
    // because folding both the mutation's lifecycle and the `status` signal
    // into one `Fn() -> String` text closure trips inference. The transient
    // line tracks the mutation (sending/error); the persistent line shows the
    // stored instruction or success message.
    let submit_for_status = submit.clone();

    ui! {
        view(style = root_fill()) {
            Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
                Typography(
                    content = "Contact us".to_string(),
                    kind = idea_ui::typography_kind::H1,
                )
                Typography(
                    content = "Submitting calls a #[server] fn that, on AWS, writes the \
                        message to DynamoDB and emails a notification via SES — the same \
                        function runs locally for development."
                        .to_string(),
                    muted = true,
                )
                text_input(value = name, on_change = move |s| name.set(s), placeholder = "Your name")
                text_input(value = email, on_change = move |s| email.set(s), placeholder = "you@example.com")
                text_input(value = message, on_change = move |s| message.set(s), placeholder = "Your message")
                button(label = "Send".to_string(), on_click = on_submit)
                text {
                    move || {
                        let state = submit_for_status.state();
                        if state.loading {
                            "Sending…".to_string()
                        } else if let Some(e) = &state.error {
                            format!("Error: {e}")
                        } else {
                            String::new()
                        }
                    }
                }
                text { move || status.get() }
            }
        }
    }
}

/// Full-screen column root so the mounted tree fills the native window.
fn root_fill() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        flex_direction: Some(FlexDirection::Column),
        ..Default::default()
    }))
}
