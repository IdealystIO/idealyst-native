//! Error type for loading and applying configuration.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    /// A config file couldn't be read.
    #[error("reading config file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// A config file couldn't be parsed as TOML.
    #[error("parsing config file {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    /// An `extends` chain referenced a file in a cycle.
    #[error("`extends` cycle detected at {0}")]
    ExtendsCycle(String),
    /// A section referenced a connection that couldn't be resolved.
    #[error("resolving `[{section}]`: {reason}")]
    Resolve { section: String, reason: String },
    /// A section selected a backend whose cargo feature isn't compiled in.
    #[error("{0}")]
    FeatureOff(String),
    /// A configured SDK backend reported an error while being wired up.
    #[error("configuring `[{section}]`: {reason}")]
    Backend { section: String, reason: String },
}
