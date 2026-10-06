//! The resolution service on AWS Lambda, behind a Function URL or API
//! Gateway: the same router, run by `server-aws`. Configured like the
//! server (`OTA_LOCATION` and the settings in the crate docs); the
//! function's role grants the bucket access, so no keys are needed when
//! Lambda supplies AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY /
//! AWS_SESSION_TOKEN, as it does.
//!
//! The resolver is built once per execution environment (cold start) and
//! reused across warm invocations, with its manifest and index caches.

use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), server_aws::Error> {
    let resolver = ota_resolver::Resolver::from_env().map_err(|e| format!("ota-resolver: {e:#}"))?;
    server_aws::run_router(ota_resolver::router(Arc::new(resolver))).await
}
