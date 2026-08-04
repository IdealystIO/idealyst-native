//! The configuration schema + field-level merge.
//!
//! A [`Config`] is what a single file deserializes into. The loader merges
//! several of them (base + per-tool + `extends` parents), so every section is
//! `Option`-of-fields and merge is "later file's set fields win."

use serde::de::{self, Deserializer};
use serde::Deserialize;
use std::collections::HashMap;

/// One config file's contents. Files compose via [`Config::merge`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Parent file(s) to merge BEFORE this one (literal inheritance). A single
    /// path or a list. Resolved relative to this file's directory.
    #[serde(default, deserialize_with = "string_or_vec")]
    pub extends: Vec<String>,

    /// Named connection profiles — the shared credential/endpoint definitions
    /// tools reference by name. Merged as a union across files (later wins per
    /// name).
    #[serde(default)]
    pub connections: HashMap<String, Connection>,

    #[serde(default)]
    pub jobs: Option<JobsSection>,
    #[serde(default)]
    pub pubsub: Option<PubsubSection>,
    #[serde(default)]
    pub email: Option<EmailSection>,
    #[serde(default)]
    pub cache: Option<CacheSection>,
}

impl Config {
    /// Overlay `other` onto `self`: `other`'s connections union in (overriding
    /// same-named), and each present section overlays field-by-field.
    pub fn merge(&mut self, other: Config) {
        for (name, conn) in other.connections {
            self.connections.insert(name, conn);
        }
        overlay_section(&mut self.jobs, other.jobs);
        overlay_section(&mut self.pubsub, other.pubsub);
        overlay_section(&mut self.email, other.email);
        overlay_section(&mut self.cache, other.cache);
        // `extends` is consumed by the loader before merge; nothing to carry.
    }

    /// Resolve a connection reference to an [`AwsConnection`], or build one
    /// from inline `region`/`profile` when no reference is given. `Err` if the
    /// named connection is missing or isn't an AWS connection.
    pub fn aws_for(
        &self,
        connection: Option<&str>,
        inline_region: Option<&str>,
        inline_profile: Option<&str>,
    ) -> Result<AwsConnection, String> {
        if let Some(name) = connection {
            match self.connections.get(name) {
                Some(Connection::Aws(a)) => Ok(a.clone()),
                Some(other) => Err(format!(
                    "connection `{name}` is a {} connection, but an AWS connection was expected",
                    other.kind_str()
                )),
                None => Err(format!("no `[connections.{name}]` defined")),
            }
        } else {
            Ok(AwsConnection {
                region: inline_region.map(str::to_string),
                profile: inline_profile.map(str::to_string),
                endpoint_url: None,
            })
        }
    }

    /// Resolve a connection reference to a URL string (redis/postgres), or fall
    /// back to `inline_url`. `Err` if the named connection is missing or isn't
    /// a URL connection.
    pub fn url_for(
        &self,
        connection: Option<&str>,
        inline_url: Option<&str>,
    ) -> Result<String, String> {
        if let Some(name) = connection {
            match self.connections.get(name) {
                Some(Connection::Redis(u)) | Some(Connection::Postgres(u)) => Ok(u.url.clone()),
                Some(other) => Err(format!(
                    "connection `{name}` is a {} connection, but a URL connection was expected",
                    other.kind_str()
                )),
                None => Err(format!("no `[connections.{name}]` defined")),
            }
        } else {
            inline_url
                .map(str::to_string)
                .ok_or_else(|| "no `connection` reference and no inline `url`".to_string())
        }
    }
}

/// A named connection profile: a shared credential/endpoint definition that
/// tools reference by name. Tagged by `kind`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Connection {
    /// An AWS account/identity — region + optional shared-config profile. Used
    /// by SES (email) and SQS (jobs). Two tools referencing the same AWS
    /// connection share one account; different names keep them separate.
    Aws(AwsConnection),
    /// A Redis endpoint.
    Redis(UrlConnection),
    /// A Postgres endpoint.
    Postgres(UrlConnection),
}

impl Connection {
    fn kind_str(&self) -> &'static str {
        match self {
            Connection::Aws(_) => "aws",
            Connection::Redis(_) => "redis",
            Connection::Postgres(_) => "postgres",
        }
    }
}

/// An AWS connection. Credentials resolve via the standard AWS provider chain
/// (env, shared config, IAM role); `profile` picks a named shared-config
/// profile, `region` pins the region, `endpoint_url` overrides the endpoint
/// (for LocalStack / testing).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwsConnection {
    pub region: Option<String>,
    pub profile: Option<String>,
    pub endpoint_url: Option<String>,
}

/// A URL-string connection (redis / postgres).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UrlConnection {
    pub url: String,
}

/// The `[jobs]` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobsSection {
    /// `memory` | `redis` | `postgres` | `sqs`.
    pub backend: Option<String>,
    /// Reference to a `[connections.<name>]` (redis/postgres URL, or AWS for sqs).
    pub connection: Option<String>,
    /// Inline broker URL (redis/postgres) when not using a `connection`.
    pub url: Option<String>,
    /// SQS: the queue URL for the default logical queue.
    pub queue_url: Option<String>,
    /// SQS: optional dead-letter queue URL.
    pub dead_letter_url: Option<String>,
}

/// The `[pubsub]` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PubsubSection {
    /// `memory` | `redis` | `postgres`.
    pub backend: Option<String>,
    pub connection: Option<String>,
    pub url: Option<String>,
}

/// The `[cache]` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSection {
    /// `memory` | `redis`.
    pub backend: Option<String>,
    /// Reference to a `[connections.<name>]` redis connection.
    pub connection: Option<String>,
    /// Inline Redis URL when not using a `connection`.
    pub url: Option<String>,
    /// Key-prefix namespace (default `cache`) — set when several apps share
    /// one Redis.
    pub namespace: Option<String>,
}

/// The `[email]` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailSection {
    /// `memory` | `ses`.
    pub provider: Option<String>,
    /// Reference to a `[connections.<name>]` AWS connection (for `ses`).
    pub connection: Option<String>,
    /// Default sender (`Name <addr>` or a bare address).
    pub from: Option<String>,
    /// Inline AWS region (when not using a `connection`).
    pub region: Option<String>,
    /// Inline AWS shared-config profile (when not using a `connection`).
    pub profile: Option<String>,
    /// Optional SES configuration set.
    pub configuration_set: Option<String>,
}

/// Overlay `other` onto `slot` field-by-field; if `slot` is `None`, take
/// `other` wholesale.
fn overlay_section<T: Overlay>(slot: &mut Option<T>, other: Option<T>) {
    match (slot.as_mut(), other) {
        (Some(base), Some(o)) => base.overlay(o),
        (None, Some(o)) => *slot = Some(o),
        (_, None) => {}
    }
}

/// Field-level overlay: each `Some` field in `other` replaces `self`'s.
pub trait Overlay {
    fn overlay(&mut self, other: Self);
}

/// Replace `$slot.$field` with `$other.$field` when the latter is `Some`.
macro_rules! overlay_fields {
    ($self:ident, $other:ident, $($field:ident),* $(,)?) => {
        $( if $other.$field.is_some() { $self.$field = $other.$field; } )*
    };
}

impl Overlay for JobsSection {
    fn overlay(&mut self, other: Self) {
        overlay_fields!(self, other, backend, connection, url, queue_url, dead_letter_url);
    }
}
impl Overlay for PubsubSection {
    fn overlay(&mut self, other: Self) {
        overlay_fields!(self, other, backend, connection, url);
    }
}
impl Overlay for EmailSection {
    fn overlay(&mut self, other: Self) {
        overlay_fields!(self, other, provider, connection, from, region, profile, configuration_set);
    }
}
impl Overlay for CacheSection {
    fn overlay(&mut self, other: Self) {
        overlay_fields!(self, other, backend, connection, url, namespace);
    }
}

/// Deserialize `extends` as either a single string or a list of strings.
fn string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    match OneOrMany::deserialize(deserializer).map_err(de::Error::custom)? {
        OneOrMany::One(s) => Ok(vec![s]),
        OneOrMany::Many(v) => Ok(v),
    }
}
