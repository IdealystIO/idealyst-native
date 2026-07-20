//! `database` — a SQL database sidecar. Variants: `postgres` (default),
//! `mysql`. Exposes a `DATABASE_URL` to the dev service.

use crate::devcontainer::service::{Ctx, DevService, ServiceFragment, ServiceVariant};

pub struct Database;

const VARIANTS: &[ServiceVariant] = &[
    ServiceVariant { id: "postgres", label: "PostgreSQL" },
    ServiceVariant { id: "mysql", label: "MySQL" },
];

impl DevService for Database {
    fn id(&self) -> &'static str {
        "database"
    }
    fn label(&self) -> &'static str {
        "Database"
    }
    fn description(&self) -> &'static str {
        "SQL database (PostgreSQL or MySQL) — exposes DATABASE_URL to the app"
    }
    fn variants(&self) -> &'static [ServiceVariant] {
        VARIANTS
    }

    fn fragment(&self, variant: Option<&str>, _ctx: &Ctx) -> ServiceFragment {
        let variant = variant.unwrap_or("postgres");
        match variant {
            "mysql" => ServiceFragment {
                // MYSQL_ROOT_PASSWORD is required by the image; a dedicated
                // app user keeps DATABASE_URL off the root account.
                service: serde_yaml::from_str(
                    r#"
image: mysql:8
restart: unless-stopped
environment:
  MYSQL_ROOT_PASSWORD: root
  MYSQL_DATABASE: app
  MYSQL_USER: app
  MYSQL_PASSWORD: app
volumes:
  - idealyst-database-data:/var/lib/mysql
"#,
                )
                .expect("valid mysql service yaml"),
                app_env: vec![(
                    "DATABASE_URL".into(),
                    "mysql://app:app@database:3306/app".into(),
                )],
                volumes: vec!["idealyst-database-data".into()],
            },
            // Default: postgres.
            _ => ServiceFragment {
                service: serde_yaml::from_str(
                    r#"
image: postgres:16
restart: unless-stopped
environment:
  POSTGRES_USER: app
  POSTGRES_PASSWORD: app
  POSTGRES_DB: app
volumes:
  - idealyst-database-data:/var/lib/postgresql/data
"#,
                )
                .expect("valid postgres service yaml"),
                app_env: vec![(
                    "DATABASE_URL".into(),
                    "postgres://app:app@database:5432/app".into(),
                )],
                volumes: vec!["idealyst-database-data".into()],
            },
        }
    }
}
