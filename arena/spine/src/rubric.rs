//! The **secret** evaluation sheet. A rubric is a flat list of atomic,
//! objectively-checkable items — no LLM judgement, no subjective scoring.
//! Each item declares the [`Tier`] that verifies it and the [`ItemClass`]
//! that decides whose fault a failure is.

use serde::Deserialize;
use std::path::Path;

/// Whose responsibility a failure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemClass {
    /// The agent chose/wired the right thing. Verifiable from source or the
    /// framework's own Robot self-report. Always the agent's responsibility.
    Decision,
    /// The thing actually renders/behaves on the platform. If this fails but
    /// its `depends_on` decision passed, the agent did its part and the
    /// platform didn't — a framework finding, not a deduction.
    Outcome,
}

/// How an item is checked. The four tiers, cheapest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// `idealyst build <target>` (or `cargo check`) succeeds.
    Compile,
    /// A regex/AST assertion over the produced source.
    Static,
    /// A Robot verb against the running app's self-report (arena tree,
    /// signals, nav stack). Wired when the Robot tier lands.
    Robot,
    /// Platform truth via a locator skill driving Playwright. The skill may
    /// use an LLM to *locate and act*, but returns a binary observable.
    Playwright,
}

/// Tier-specific check parameters. All fields optional; each verifier reads
/// only the ones its tier defines. Kept as one flat struct (rather than a
/// tagged enum) so a TOML inline table deserializes directly.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Assertion {
    // --- static ---
    /// Regex the source must (or, with `absent`, must not) contain.
    pub pattern: Option<String>,
    /// Glob (relative to the project root) of files to search. Default `**/*.rs`.
    #[serde(rename = "in")]
    pub glob: Option<String>,
    /// Invert: the pattern must NOT appear anywhere.
    #[serde(default)]
    pub absent: bool,
    /// Require at least this many matches (default 1).
    pub min_count: Option<usize>,

    // --- compile ---
    /// Build target: `check` (default), `web`, `macos`, … → `idealyst build --<t>`.
    pub target: Option<String>,

    // --- robot / playwright ---
    /// Action the locator performs before asserting (skill-interpreted).
    pub action: Option<String>,
    /// Accessibility role the asserted element must expose.
    pub expect_role: Option<String>,
    /// Accessible name the asserted element must expose.
    pub expect_name: Option<String>,
    /// Robot verb to invoke (arena/signals/nav/…).
    pub verb: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RubricItem {
    pub id: String,
    #[serde(default)]
    pub description: String,
    pub points: u32,
    pub class: ItemClass,
    pub tier: Tier,
    /// Name of the verifier skill/checker that owns this tier.
    pub verifier: String,
    /// For outcome items: the decision item that must have passed for a
    /// failure here to count against the agent. If that decision passed and
    /// this fails, the failure is neutralized into a framework finding.
    pub depends_on: Option<String>,
    #[serde(default)]
    pub assertion: Assertion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rubric {
    pub scenario_id: String,
    #[serde(rename = "item", default)]
    pub items: Vec<RubricItem>,
}

impl Rubric {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading rubric {}: {e}", path.display()))?;
        let rubric: Rubric = toml::from_str(&raw)?;
        rubric.validate()?;
        Ok(rubric)
    }

    pub fn item(&self, id: &str) -> Option<&RubricItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Catch authoring mistakes early: duplicate ids and dangling `depends_on`
    /// references would silently corrupt scoring.
    pub fn validate(&self) -> anyhow::Result<()> {
        for (i, item) in self.items.iter().enumerate() {
            if self.items[..i].iter().any(|p| p.id == item.id) {
                anyhow::bail!("duplicate rubric item id '{}'", item.id);
            }
            if let Some(dep) = &item.depends_on {
                if !self.items.iter().any(|p| &p.id == dep) {
                    anyhow::bail!(
                        "rubric item '{}' depends_on unknown item '{}'",
                        item.id,
                        dep
                    );
                }
            }
        }
        Ok(())
    }
}
