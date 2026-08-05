//! `minio` — an S3-compatible object store. No variants. Exposes the
//! endpoint + root credentials to the app.

use crate::devcontainer::service::{Ctx, DevService, ServiceFragment};

pub struct Minio;

impl DevService for Minio {
    fn id(&self) -> &'static str {
        "minio"
    }
    fn label(&self) -> &'static str {
        "MinIO"
    }
    fn description(&self) -> &'static str {
        "MinIO S3-compatible object storage — exposes MINIO_ENDPOINT + credentials"
    }

    fn fragment(&self, _variant: Option<&str>, _ctx: &Ctx) -> ServiceFragment {
        ServiceFragment {
            // `server /data --console-address :9001` runs the S3 API on 9000
            // and the web console on 9001. Both stay on the compose network;
            // no host ports are published (the dev container reaches it by
            // service name), keeping the sidecar side-effect-free on the host.
            service: Some(
                serde_yaml::from_str(
                    r#"
image: minio/minio
restart: unless-stopped
command: server /data --console-address ":9001"
environment:
  MINIO_ROOT_USER: minioadmin
  MINIO_ROOT_PASSWORD: minioadmin
volumes:
  - idealyst-minio-data:/data
"#,
                )
                .expect("valid minio service yaml"),
            ),
            app_env: vec![
                ("MINIO_ENDPOINT".into(), "http://minio:9000".into()),
                ("MINIO_ACCESS_KEY".into(), "minioadmin".into()),
                ("MINIO_SECRET_KEY".into(), "minioadmin".into()),
            ],
            volumes: vec!["idealyst-minio-data".into()],
            ..Default::default()
        }
    }
}
