//! `redis` — a Redis instance. No variants. Exposes `REDIS_URL`.

use crate::devcontainer::service::{Ctx, DevService, ServiceFragment};

pub struct Redis;

impl DevService for Redis {
    fn id(&self) -> &'static str {
        "redis"
    }
    fn label(&self) -> &'static str {
        "Redis"
    }
    fn description(&self) -> &'static str {
        "Redis key-value store — exposes REDIS_URL to the app"
    }

    fn fragment(&self, _variant: Option<&str>, _ctx: &Ctx) -> ServiceFragment {
        ServiceFragment {
            service: serde_yaml::from_str(
                r#"
image: redis:7
restart: unless-stopped
volumes:
  - idealyst-redis-data:/data
"#,
            )
            .expect("valid redis service yaml"),
            app_env: vec![("REDIS_URL".into(), "redis://redis:6379".into())],
            volumes: vec!["idealyst-redis-data".into()],
        }
    }
}
