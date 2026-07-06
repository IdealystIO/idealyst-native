//! AWS SESv2 provider (feature `ses`).
//!
//! Delivers through Amazon SES using the v2 `SendEmail` API. Credentials and
//! region come from the standard AWS provider chain (env vars, shared config,
//! IAM role) via [`aws_config`], so a deployment configures SES the same way
//! it configures any other AWS SDK.

use crate::{Email, EmailError, EmailId, EmailProvider, Mailbox};
use async_trait::async_trait;
use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message};
use aws_sdk_sesv2::Client;

/// An [`EmailProvider`] backed by AWS SESv2.
pub struct SesProvider {
    client: Client,
    default_from: Option<Mailbox>,
    /// Optional SES configuration set (for dedicated IP pools, event
    /// publishing, etc.). Applied to every send when present.
    configuration_set: Option<String>,
}

impl SesProvider {
    /// Build a provider from the ambient AWS environment (env vars / shared
    /// config / IAM role, resolved region).
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self::new(Client::new(&config))
    }

    /// Build a provider from an already-constructed SESv2 client (lets a
    /// caller customize region / credentials / retries).
    pub fn new(client: Client) -> Self {
        Self { client, default_from: None, configuration_set: None }
    }

    /// Build a provider from a resolved AWS [`SdkConfig`](aws_config::SdkConfig)
    /// — the shape `idealyst-config` produces from a `[connections.<name>]`
    /// AWS profile, so email and jobs can share one account.
    pub fn from_aws(config: &aws_config::SdkConfig) -> Self {
        Self::new(Client::new(config))
    }

    /// Set a default sender used for messages that omit `from`. SES requires
    /// this address to be a verified identity.
    pub fn with_default_from(mut self, from: impl Into<Mailbox>) -> Self {
        self.default_from = Some(from.into());
        self
    }

    /// Attach an SES configuration set to every send.
    pub fn with_configuration_set(mut self, name: impl Into<String>) -> Self {
        self.configuration_set = Some(name.into());
        self
    }
}

/// Encode a list of mailboxes into SES `Name <addr>` address strings.
fn encode_all(boxes: &[Mailbox]) -> Vec<String> {
    boxes.iter().map(Mailbox::encoded).collect()
}

/// A `Content` (SES's charset-tagged string) or a provider error.
fn content(data: &str) -> Result<Content, EmailError> {
    Content::builder()
        .data(data)
        .charset("UTF-8")
        .build()
        .map_err(EmailError::provider)
}

#[async_trait]
impl EmailProvider for SesProvider {
    async fn send(&self, email: &Email) -> Result<EmailId, EmailError> {
        // `from` is resolved by `crate::send` before dispatch.
        let from = email
            .from
            .as_ref()
            .ok_or_else(|| EmailError::Invalid("SES send without a sender".into()))?;

        let destination = Destination::builder()
            .set_to_addresses(Some(encode_all(&email.to)))
            .set_cc_addresses((!email.cc.is_empty()).then(|| encode_all(&email.cc)))
            .set_bcc_addresses((!email.bcc.is_empty()).then(|| encode_all(&email.bcc)))
            .build();

        // Body: whichever parts are present. SES accepts html-only, text-only,
        // or both (a multipart/alternative message).
        let mut body = Body::builder();
        if let Some(html) = &email.html {
            body = body.html(content(html)?);
        }
        if let Some(text) = &email.text {
            body = body.text(content(text)?);
        }

        let message = Message::builder()
            .subject(content(&email.subject)?)
            .body(body.build())
            .build();

        let reply_to = email.reply_to.as_ref().map(|m| vec![m.encoded()]);

        let result = self
            .client
            .send_email()
            .from_email_address(from.encoded())
            .destination(destination)
            .content(EmailContent::builder().simple(message).build())
            .set_reply_to_addresses(reply_to)
            .set_configuration_set_name(self.configuration_set.clone())
            .send()
            .await
            .map_err(EmailError::provider)?;

        Ok(EmailId(result.message_id().unwrap_or_default().to_string()))
    }

    fn default_from(&self) -> Option<Mailbox> {
        self.default_from.clone()
    }
}
