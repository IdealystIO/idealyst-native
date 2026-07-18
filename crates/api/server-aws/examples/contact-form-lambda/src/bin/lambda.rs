//! AWS Lambda entry point for the contact-form API.
//!
//! Builds the server-fn router and hands it to the Lambda runtime via the
//! `server-aws` adapter. Deploy with:
//!
//! ```text
//! cargo lambda build --release --arm64 --features server
//! cargo lambda deploy contact-form-lambda \
//!   --env-var CONTACT_TABLE=ContactSubmissions \
//!   --env-var CONTACT_FROM=noreply@yourdomain.com \
//!   --env-var CONTACT_TO=you@yourdomain.com \
//!   --enable-function-url
//! ```
//!
//! The function's IAM role needs `dynamodb:PutItem` on the table and
//! `ses:SendEmail`. See `template.yaml` for the SAM equivalent.
//!
//! FORCE-LINK: referencing `contact_form_lambda::submit_contact` keeps the
//! linker from dead-stripping the lib's `inventory::submit!` registration —
//! without a reference into the lib, `server::router()` registers zero routes
//! and every `/_srv/<fn>` 404s. (`server::router()` also warns at startup when
//! it finds no routes, as a backstop.)
use contact_form_lambda::submit_contact as _force_link;

#[tokio::main]
async fn main() -> Result<(), server_aws::Error> {
    let _ = _force_link; // touch the symbol; see FORCE-LINK note above.
    server_aws::run().await
}
