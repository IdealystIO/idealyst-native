//! Built-in devcontainer services. Each module defines one [`DevService`];
//! [`super::service::registry`] lists them.
//!
//! [`DevService`]: super::service::DevService

pub mod database;
pub mod minio;
pub mod redis;
