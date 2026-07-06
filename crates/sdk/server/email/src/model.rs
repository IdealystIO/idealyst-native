//! The email value types: [`Mailbox`], [`Email`], and the send-result
//! [`EmailId`].

use serde::{Deserialize, Serialize};

/// One email address, optionally with a display name. Renders as
/// `Name <addr@host>` when a name is present, else the bare address.
///
/// Construct from a string (`"a@b.com".into()`), a `(name, addr)` pair
/// (`("Sam", "sam@b.com").into()`), or the explicit [`Mailbox::new`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mailbox {
    pub address: String,
    pub name: Option<String>,
}

impl Mailbox {
    pub fn new(address: impl Into<String>) -> Self {
        Self { address: address.into(), name: None }
    }

    pub fn named(name: impl Into<String>, address: impl Into<String>) -> Self {
        Self { address: address.into(), name: Some(name.into()) }
    }

    /// RFC 5322 rendering: `Name <addr>` if named, else the bare address.
    pub fn encoded(&self) -> String {
        match &self.name {
            Some(name) => format!("{name} <{}>", self.address),
            None => self.address.clone(),
        }
    }
}

impl From<&str> for Mailbox {
    fn from(s: &str) -> Self {
        Mailbox::new(s)
    }
}

impl From<String> for Mailbox {
    fn from(s: String) -> Self {
        Mailbox::new(s)
    }
}

impl From<(&str, &str)> for Mailbox {
    fn from((name, address): (&str, &str)) -> Self {
        Mailbox::named(name, address)
    }
}

/// A composed email message. Build it with the fluent methods; the `to`
/// recipient(s), a subject, and at least one body part (`html` or `text`) are
/// required to send. `from` may be omitted if the configured provider carries
/// a default sender.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Email {
    pub from: Option<Mailbox>,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    pub reply_to: Option<Mailbox>,
    pub subject: String,
    /// The `text/html` part.
    pub html: Option<String>,
    /// The `text/plain` alternative part.
    pub text: Option<String>,
}

impl Email {
    /// Start a message to one recipient. Add more with [`Email::to_also`].
    pub fn to(addr: impl Into<Mailbox>) -> Self {
        Email { to: vec![addr.into()], ..Default::default() }
    }

    /// Start a message with no recipients yet (add them with `to_also`).
    pub fn builder() -> Self {
        Email::default()
    }

    pub fn to_also(mut self, addr: impl Into<Mailbox>) -> Self {
        self.to.push(addr.into());
        self
    }

    pub fn from(mut self, addr: impl Into<Mailbox>) -> Self {
        self.from = Some(addr.into());
        self
    }

    pub fn cc(mut self, addr: impl Into<Mailbox>) -> Self {
        self.cc.push(addr.into());
        self
    }

    pub fn bcc(mut self, addr: impl Into<Mailbox>) -> Self {
        self.bcc.push(addr.into());
        self
    }

    pub fn reply_to(mut self, addr: impl Into<Mailbox>) -> Self {
        self.reply_to = Some(addr.into());
        self
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// Set the `text/html` body from a raw string.
    pub fn html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }

    /// Set the `text/plain` body from a raw string.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Render an idealyst template into this email's `html` + `text` parts via
    /// [`backend_email`]. Styles are inlined and theme tokens resolved to
    /// literals — no WASM, no browser. If the template declares a page
    /// metadata title and no [`Email::subject`] was set yet, it becomes the
    /// subject.
    ///
    /// ```ignore
    /// Email::to(addr).template(|| Welcome(WelcomeProps { name }))
    /// ```
    pub fn template<F>(self, app: F) -> Self
    where
        F: FnOnce() -> runtime_core::Element,
    {
        self.template_with(|_| {}, app)
    }

    /// Like [`Email::template`] but runs `setup` against the email backend
    /// first — the hook to install theme tokens / app background for the
    /// render (e.g. your theme's `install`).
    pub fn template_with<S, F>(mut self, setup: S, app: F) -> Self
    where
        S: FnOnce(&mut backend_email::EmailBackend),
        F: FnOnce() -> runtime_core::Element,
    {
        let rendered = backend_email::render_email_with(setup, app);
        self.html = Some(rendered.html);
        self.text = Some(rendered.text);
        if self.subject.is_empty() {
            if let Some(subject) = rendered.subject {
                self.subject = subject;
            }
        }
        self
    }
}

/// A provider-assigned identifier for an accepted message (e.g. the SES
/// `MessageId`). Opaque; useful for logging / correlating delivery events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailId(pub String);

impl std::fmt::Display for EmailId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
