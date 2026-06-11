//! ESLint-style configuration: every rule has a default level, and a
//! project `idealyst-lint.toml` can override any rule to `off` / `warn` /
//! `error`. Inline `// idealyst-lint-disable…` directives suppress
//! findings on specific lines or for a whole file.
//!
//! Config file shape (all keys optional):
//!
//! ```toml
//! # idealyst-lint.toml
//! [rules]
//! component-pascal-case = "error"
//! prefer-signal-macro   = "warn"
//! prefer-ui-macro       = "off"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::rules;

/// A rule's configured level. `Off` disables it entirely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Warn,
    Error,
}

/// The on-disk file shape (`[rules]` table of id → level).
#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    rules: BTreeMap<String, Level>,
}

/// Resolved configuration: per-rule levels, defaulted from the registry
/// and overlaid with any `idealyst-lint.toml` overrides.
#[derive(Debug, Clone)]
pub struct Config {
    levels: BTreeMap<String, Level>,
}

impl Default for Config {
    fn default() -> Self {
        // Seed every known rule at its registry default so an absent
        // config behaves identically to an empty `[rules]` table.
        let levels = rules::all_rules()
            .iter()
            .map(|r| (r.id.to_string(), r.default_level))
            .collect();
        Config { levels }
    }
}

impl Config {
    /// The effective level for a rule id. Unknown ids (e.g. a future rule
    /// named in a config the running binary doesn't have yet) default to
    /// `Warn` so a typo'd override never silently disables enforcement.
    pub fn level_for(&self, rule: &str) -> Level {
        self.levels.get(rule).copied().unwrap_or(Level::Warn)
    }

    /// Apply a parsed config file's overrides on top of the defaults,
    /// returning the unknown rule ids encountered (for a warning).
    fn apply_overrides(&mut self, file: ConfigFile) -> Vec<String> {
        let known: std::collections::BTreeSet<&'static str> =
            rules::all_rules().iter().map(|r| r.id).collect();
        let mut unknown = Vec::new();
        for (id, level) in file.rules {
            if !known.contains(id.as_str()) {
                unknown.push(id.clone());
            }
            self.levels.insert(id, level);
        }
        unknown
    }

    /// Load config by searching upward from `start` for `idealyst-lint.toml`.
    /// Returns the resolved config, the file path that was used (if any),
    /// and any unknown rule ids the caller may want to warn about.
    pub fn discover(start: &Path) -> anyhow::Result<Loaded> {
        let mut dir = if start.is_file() {
            start.parent().map(Path::to_path_buf)
        } else {
            Some(start.to_path_buf())
        };
        while let Some(d) = dir {
            let candidate = d.join(CONFIG_FILE_NAME);
            if candidate.is_file() {
                return Self::load_file(&candidate);
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        Ok(Loaded { config: Config::default(), path: None, unknown_rules: Vec::new() })
    }

    /// Load config from an explicit path.
    pub fn load_file(path: &Path) -> anyhow::Result<Loaded> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let file: ConfigFile = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
        let mut config = Config::default();
        let unknown_rules = config.apply_overrides(file);
        Ok(Loaded { config, path: Some(path.to_path_buf()), unknown_rules })
    }
}

/// The canonical config file name searched for during discovery.
pub const CONFIG_FILE_NAME: &str = "idealyst-lint.toml";

/// Result of loading config — the resolved [`Config`] plus provenance.
pub struct Loaded {
    pub config: Config,
    /// The config file actually used, or `None` if defaults were applied.
    pub path: Option<PathBuf>,
    /// Rule ids named in the file that the running binary doesn't know.
    pub unknown_rules: Vec<String>,
}

// ---------------------------------------------------------------------------
// Inline suppression directives
// ---------------------------------------------------------------------------

/// What an inline directive suppresses — every rule, or a named subset.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    All,
    Rules(Vec<String>),
}

impl Scope {
    fn covers(&self, rule: &str) -> bool {
        match self {
            Scope::All => true,
            Scope::Rules(ids) => ids.iter().any(|r| r == rule),
        }
    }
}

/// Per-file inline suppressions, parsed from the raw source text.
///
/// `syn` discards ordinary `//` comments, so directives can't come from
/// the AST — they're scanned out of the source string here. Three forms,
/// mirroring ESLint:
///
/// ```ignore
/// // idealyst-lint-disable-file                     (whole file)
/// // idealyst-lint-disable-file prefer-signal-macro (whole file, one rule)
/// let s = Signal::new(0); // idealyst-lint-disable-line
/// // idealyst-lint-disable-next-line prefer-signal-macro
/// let s = Signal::new(0);
/// ```
///
/// A directive with no rule ids after it suppresses *all* rules.
#[derive(Debug, Default)]
pub struct Suppressions {
    file: Option<Scope>,
    /// 1-based line number → scope suppressed on that line.
    lines: BTreeMap<usize, Scope>,
}

const DISABLE_FILE: &str = "idealyst-lint-disable-file";
const DISABLE_NEXT_LINE: &str = "idealyst-lint-disable-next-line";
const DISABLE_LINE: &str = "idealyst-lint-disable-line";

impl Suppressions {
    /// Scan a file's source for inline directives.
    pub fn parse(source: &str) -> Self {
        let mut sup = Suppressions::default();
        for (idx, line) in source.lines().enumerate() {
            let lineno = idx + 1; // 1-based
            let Some(comment) = line.split("//").nth(1) else { continue };
            let comment = comment.trim();
            // Order matters: `-next-line` and `-line` both start with the
            // `disable` stem, and `-file` is distinct. Check the longest
            // (most specific) keyword first so `-line` doesn't shadow
            // `-next-line`.
            if let Some(rest) = comment.strip_prefix(DISABLE_NEXT_LINE) {
                sup.lines.insert(lineno + 1, parse_scope(rest));
            } else if let Some(rest) = comment.strip_prefix(DISABLE_LINE) {
                sup.lines.insert(lineno, parse_scope(rest));
            } else if let Some(rest) = comment.strip_prefix(DISABLE_FILE) {
                sup.file = Some(parse_scope(rest));
            }
        }
        sup
    }

    /// True when a finding for `rule` on `line` should be dropped.
    pub fn suppresses(&self, rule: &str, line: usize) -> bool {
        if let Some(scope) = &self.file {
            if scope.covers(rule) {
                return true;
            }
        }
        if let Some(scope) = self.lines.get(&line) {
            if scope.covers(rule) {
                return true;
            }
        }
        false
    }
}

/// Parse the rule-id list trailing a directive. Empty → all rules.
/// Accepts comma- and/or whitespace-separated ids.
fn parse_scope(rest: &str) -> Scope {
    let ids: Vec<String> = rest
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        Scope::All
    } else {
        Scope::Rules(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_seed_every_rule() {
        let cfg = Config::default();
        for r in rules::all_rules() {
            assert_eq!(cfg.level_for(r.id), r.default_level, "rule {}", r.id);
        }
    }

    #[test]
    fn override_changes_level() {
        let file: ConfigFile = toml::from_str(
            r#"
            [rules]
            component-pascal-case = "off"
            prefer-signal-macro = "error"
            "#,
        )
        .unwrap();
        let mut cfg = Config::default();
        let unknown = cfg.apply_overrides(file);
        assert!(unknown.is_empty());
        assert_eq!(cfg.level_for("component-pascal-case"), Level::Off);
        assert_eq!(cfg.level_for("prefer-signal-macro"), Level::Error);
    }

    #[test]
    fn unknown_rule_is_reported() {
        let file: ConfigFile = toml::from_str(
            r#"
            [rules]
            no-such-rule = "warn"
            "#,
        )
        .unwrap();
        let mut cfg = Config::default();
        let unknown = cfg.apply_overrides(file);
        assert_eq!(unknown, vec!["no-such-rule".to_string()]);
    }

    #[test]
    fn disable_next_line_targets_following_line() {
        let src = "// idealyst-lint-disable-next-line prefer-signal-macro\nlet s = Signal::new(0);\n";
        let sup = Suppressions::parse(src);
        assert!(sup.suppresses("prefer-signal-macro", 2));
        assert!(!sup.suppresses("prefer-signal-macro", 1));
        assert!(!sup.suppresses("prefer-ui-macro", 2), "other rules unaffected");
    }

    #[test]
    fn disable_line_targets_same_line() {
        let src = "let s = Signal::new(0); // idealyst-lint-disable-line\n";
        let sup = Suppressions::parse(src);
        assert!(sup.suppresses("prefer-signal-macro", 1));
    }

    #[test]
    fn bare_disable_line_covers_all_rules() {
        let src = "let s = Signal::new(0); // idealyst-lint-disable-line\n";
        let sup = Suppressions::parse(src);
        assert!(sup.suppresses("anything-at-all", 1));
    }

    #[test]
    fn disable_file_covers_whole_file() {
        let src = "// idealyst-lint-disable-file\nfn x() {}\nfn y() {}\n";
        let sup = Suppressions::parse(src);
        assert!(sup.suppresses("prefer-ui-macro", 2));
        assert!(sup.suppresses("prefer-ui-macro", 3));
    }

    #[test]
    fn disable_file_scoped_to_one_rule() {
        let src = "// idealyst-lint-disable-file prefer-signal-macro\n";
        let sup = Suppressions::parse(src);
        assert!(sup.suppresses("prefer-signal-macro", 5));
        assert!(!sup.suppresses("component-pascal-case", 5));
    }
}
