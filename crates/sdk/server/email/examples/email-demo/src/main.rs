//! email-demo — render an idealyst template to email-safe HTML and send it.
//!
//! Run it:
//!
//! ```sh
//! cargo run -p email-demo -- sam@example.com
//! ```
//!
//! By default this uses the in-memory **capture** provider: it doesn't hit the
//! network, it records the message, prints the rendered HTML + plaintext, and
//! writes the HTML to a temp file you can open in a browser to preview.
//!
//! For real delivery through AWS SES, build with the `ses` feature and set the
//! provider via env (standard AWS credentials/region apply):
//!
//! ```sh
//! IDEALYST_EMAIL_PROVIDER=ses \
//! IDEALYST_EMAIL_FROM="Idealyst <no-reply@yourdomain.dev>" \
//! cargo run -p email-demo --features ses -- sam@example.com
//! ```

mod template;

use runtime_core::Reactive;
use template::{WelcomeEmail, WelcomeEmailProps};

#[tokio::main]
async fn main() {
    let recipient = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sam@example.com".to_string());

    // Configure the provider. A real deployment configures everything from
    // files with ONE call:
    //
    //     idealyst_config::configure_all().await?;
    //
    // which reads `idealyst.toml` (+ `mail.toml`), resolves connection profiles
    // (so SES can share an AWS account with the jobs SDK), and installs the
    // provider. See `idealyst.toml` in this crate for the SES setup.
    //
    // For this DEMO we want to preview what was sent, so on the default path we
    // install a memory provider we hold a handle to. When a real provider is
    // selected (IDEALYST_EMAIL_PROVIDER=ses, or an `[email]` block in
    // idealyst.toml), we hand off to `configure_all`.
    let provider = email::MemoryProvider::new()
        .with_default_from(("Idealyst", "no-reply@idealyst.dev"));

    match std::env::var("IDEALYST_EMAIL_PROVIDER").ok().as_deref() {
        Some("memory") | None => email::configure(provider.clone()),
        // A real provider → the unified config layer resolves + installs it.
        Some(_) => idealyst_config::configure_all()
            .await
            .expect("configure email provider from idealyst.toml / env"),
    }

    // Compose the message. `.template(...)` renders the idealyst component to
    // email-safe HTML (styles inlined, tokens resolved) AND derives a plaintext
    // alternative — no browser, no WASM.
    let message = email::Email::to(recipient.as_str())
        .subject("Welcome to Idealyst")
        .reply_to("support@idealyst.dev")
        .template(|| {
            WelcomeEmail(&WelcomeEmailProps {
                name: Reactive::Static("Sam".into()),
                cta_url: Reactive::Static("https://idealyst.dev/dashboard".into()),
            })
        });

    let id = email::send(message).await.expect("send email");
    println!("✓ sent — provider message id: {id}");

    // If we're on the capture provider, show + save what it recorded.
    let captured = provider.sent();
    if let Some(sent) = captured.last() {
        println!("  to:      {}", sent.to[0].address);
        println!("  from:    {}", sent.from.as_ref().unwrap().encoded());
        println!("  subject: {}", sent.subject);

        let path = std::env::temp_dir().join("idealyst-email-demo.html");
        if let Some(html) = &sent.html {
            std::fs::write(&path, html).expect("write html preview");
            println!("\nHTML preview written to: {}", path.display());
            println!("  open it in a browser to see the rendered email.");
        }
        if let Some(text) = &sent.text {
            println!("\n--- text/plain alternative ---\n{text}");
        }
    } else {
        println!("(provider does not capture messages — it delivered directly)");
    }
}
