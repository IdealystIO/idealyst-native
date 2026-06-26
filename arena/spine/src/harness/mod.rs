//! The live half of the arena: turning a scenario into scored runs by driving
//! real agents. Where [`crate::verify`] and [`crate::score`] are pure and
//! deterministic, this module supplies the seams the *orchestrator* uses.
//!
//! The agent roles (implementation, locator, feedback) are **subagents** the
//! orchestrating Claude Code session runs — see `.claude/agents/arena-*.md` and
//! `.claude/skills/arena-bench`. Driving them as subagents keeps the run on the
//! Claude subscription instead of pay-as-you-go API billing (the cost of
//! `claude --print`), and lets the implementation agent be hard-isolated to the
//! idealyst MCP via the subagent's `mcpServers:` frontmatter.
//!
//! So this module no longer spawns `claude`. It provides:
//!   * [`scaffold`] — build the isolated project + best-effort web build;
//!   * [`agent`] — parse a subagent transcript into a tool-call log + tokens;
//!   * [`locate`] — the deterministic Playwright-tier contract (prompt/verdict);
//!   * [`feedback`] — the (pure) feedback-reviewer prompt;
//!   * [`robot_web`] — relay + headless host so the robot tier works on a web
//!     build (used by the fast-follow `live` step).
//!
//! The CLI (`bin/arena.rs`) exposes the deterministic steps —
//! `scaffold` / `build` / `score` — that the skill stitches together around the
//! subagent spawns.

pub mod agent;
pub mod feedback;
pub mod locate;
pub mod robot_web;
pub mod scaffold;
