//! The live half of the arena: turning a scenario into scored runs by driving
//! real agents. Where [`crate::verify`] and [`crate::score`] are pure and
//! deterministic, this module supplies the seams the *orchestrator* uses.
//!
//! The agent roles are run by the orchestrating Claude Code session, never
//! spawned from here — see `.claude/skills/arena-bench`: the implementer is a
//! headless `claude` session inside the arena container (naive, isolated to the
//! idealyst MCP), and the reviewer roles (locator, feedback) are subagents
//! (`.claude/agents/arena-*.md`).
//!
//! This module provides:
//!   * [`scaffold`] — build the isolated project + best-effort web build;
//!   * [`agent`] — parse a subagent transcript into a tool-call log + tokens;
//!   * [`locate`] — the deterministic Playwright-tier contract (prompt/verdict);
//!   * [`feedback`] — the (pure) feedback-reviewer prompt;
//!   * [`robot_web`] — relay + headless host so the robot tier works on a web
//!     build (used by the `live` step).
//!
//! The CLI (`bin/arena.rs`) exposes the deterministic steps —
//! `scaffold` / `build` / `score` — that the skill stitches together around the
//! subagent spawns.

pub mod agent;
pub mod feedback;
pub mod locate;
pub mod robot_web;
pub mod scaffold;
