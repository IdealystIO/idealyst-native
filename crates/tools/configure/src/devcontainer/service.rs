//! The devcontainer service extension point.
//!
//! Every optional sidecar (database, redis, minio, …) is a [`DevService`].
//! Each has an idealyst **canonical id** (`"database"`, `"redis"`, `"minio"`)
//! that doubles as its compose service key, so we only ever regenerate the
//! services *we* emit — a user's own `minio` added under a different key in
//! their `docker-compose.yml` is never in this registry and never touched.
//!
//! Adding a service is deliberately cheap: write one file under `services/`
//! that implements this trait, then add one line to [`registry`]. Nothing
//! else in the crate — or the CLI / MCP front-ends — needs to change; the
//! CLI derives its `--<id>` flags and the wizard derives its checklist from
//! whatever [`registry`] returns.

use serde_yaml::Value;

/// One selectable flavour of a service (e.g. `postgres` vs `mysql` for the
/// database). Services with no variants return an empty slice.
#[derive(Clone, Copy, Debug)]
pub struct ServiceVariant {
    /// Stable id — used as the CLI flag value and stored in `x-idealyst`.
    pub id: &'static str,
    /// Human label for the wizard.
    pub label: &'static str,
}

/// Context a service needs to build its compose fragment.
#[derive(Clone, Debug)]
pub struct Ctx {
    /// The compose service key of the main dev container (read from
    /// `devcontainer.json`'s `"service"`, default `"dev"`). Services attach
    /// their connection env + `depends_on` edge to this service.
    pub app_service: String,
}

/// Everything a single enabled service contributes to the managed compose
/// file: its own service definition, the env vars merged into the dev
/// service, and any top-level named volumes it declares.
#[derive(Clone, Debug, Default)]
pub struct ServiceFragment {
    /// The compose service mapping — inserted under the service's canonical
    /// id in the top-level `services:` map.
    pub service: Value,
    /// `KEY=value` pairs merged into `<app_service>.environment` so app code
    /// finds the sidecar (e.g. `DATABASE_URL`). Deterministic order.
    pub app_env: Vec<(String, String)>,
    /// Top-level named volumes this service references (declared under the
    /// file's `volumes:` key so Compose creates them).
    pub volumes: Vec<String>,
}

/// A configurable devcontainer sidecar service.
pub trait DevService: Send + Sync {
    /// Canonical id — the compose service key AND the CLI flag/`x-idealyst`
    /// key. Must be stable and unique across the registry.
    fn id(&self) -> &'static str;

    /// Human-facing name for the wizard (e.g. `"Database"`).
    fn label(&self) -> &'static str;

    /// One-line description for the wizard / MCP tool schema.
    fn description(&self) -> &'static str;

    /// Selectable flavours. Empty = the service has a single fixed form.
    fn variants(&self) -> &'static [ServiceVariant] {
        &[]
    }

    /// The default variant id (first declared), or `None` when variantless.
    fn default_variant(&self) -> Option<&'static str> {
        self.variants().first().map(|v| v.id)
    }

    /// True when `variant` is a valid id for this service (always true for
    /// the `None`/default of a variantless service).
    fn accepts_variant(&self, variant: Option<&str>) -> bool {
        match variant {
            None => true,
            Some(v) => self.variants().iter().any(|sv| sv.id == v),
        }
    }

    /// Build this service's compose contribution for the chosen `variant`.
    /// `variant` is `None` for variantless services or when the caller wants
    /// the default (implementations should treat `None` as [`Self::default_variant`]).
    fn fragment(&self, variant: Option<&str>, ctx: &Ctx) -> ServiceFragment;
}

/// The service registry — the single list the whole feature is driven by.
///
/// **To add a service:** implement [`DevService`] in a new `services/<id>.rs`
/// module and push it here. The CLI flags, wizard entries, and MCP schema all
/// derive from this list.
pub fn registry() -> Vec<Box<dyn DevService>> {
    vec![
        Box::new(super::services::database::Database),
        Box::new(super::services::redis::Redis),
        Box::new(super::services::minio::Minio),
    ]
}

/// Look up a service by canonical id.
pub fn find(id: &str) -> Option<Box<dyn DevService>> {
    registry().into_iter().find(|s| s.id() == id)
}
