//! Local development server for the contact-form API — the same router the
//! Lambda runs, bound to a TCP port instead of the Lambda runtime.
//!
//! ```text
//! # Round-trip without AWS credentials (logs instead of calling AWS):
//! CONTACT_DRY_RUN=1 cargo run -p contact-form-lambda --bin local --features server
//!
//! # Against real AWS (uses your default credential chain):
//! CONTACT_TABLE=ContactSubmissions CONTACT_FROM=… CONTACT_TO=… \
//!   cargo run -p contact-form-lambda --bin local --features server
//! ```
//!
//! Then POST a submission:
//!
//! ```text
//! curl -s localhost:3000/_srv/submit_contact \
//!   -H 'content-type: application/json' \
//!   -d '{"name":"Ada","email":"ada@example.com","message":"hello"}'
//! ```
//!
//! FORCE-LINK: see the note in `lambda.rs`.
use contact_form_lambda::submit_contact as _force_link;

#[tokio::main]
async fn main() {
    let _ = _force_link;

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    println!("contact-form API → http://{addr}/_srv/submit_contact");
    if matches!(
        std::env::var("CONTACT_DRY_RUN").ok().as_deref(),
        Some("1") | Some("true")
    ) {
        println!("(CONTACT_DRY_RUN: AWS calls are skipped; submissions are logged)");
    }
    server::serve(addr).await.expect("serve");
}
