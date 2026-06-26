//! One full run: scaffold an isolated project, drive the implementation agent,
//! build it, verify every rubric item at its tier, and score. This is the unit
//! [`super::bench`] repeats N times.
//!
//! The web build is done once here and its result is fed directly into the
//! compile-tier verdict (rather than letting the compile verifier shell out a
//! second time), and the same `dist/web/` is served to the locator pass. So a
//! run builds wasm exactly once.

use super::{agent, feedback, locate, scaffold};
use crate::metrics::{self, Pathologies};
use crate::rubric::{Rubric, Tier};
use crate::scenario::Scenario;
use crate::score::{score_from_results, ScoredRun};
use crate::verify::{verifier_for, RunContext, VerifyResult};
use crate::{report, AgentRun};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command};

pub struct RunOptions {
    /// Path to this framework checkout (for path-dep'd scaffolds).
    pub framework_path: PathBuf,
    /// Root under which per-run directories are created.
    pub runs_root: PathBuf,
    /// The identical-across-runs preamble prepended to every scenario prompt.
    pub preamble: String,
    /// Dollar ceiling per agent invocation.
    pub budget_usd: f64,
    /// Run the Playwright locator pass for `playwright`-tier outcome items.
    pub locate: bool,
    /// Run the feedback agent over each run's artifacts.
    pub feedback: bool,
}

pub struct RunOutput {
    pub scored: ScoredRun,
    pub pathologies: Pathologies,
    pub doc_bypass_reads: usize,
    pub run_dir: PathBuf,
    pub project_dir: PathBuf,
}

/// A backgrounded static file server that's killed on drop, so a panic or early
/// return never leaks a port.
struct ServeGuard(Child);
impl Drop for ServeGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Serve `dir` on `port` via `python3 -m http.server` (which maps `.wasm` to
/// `application/wasm` on 3.11+). Returns `None` if python isn't available.
fn serve(dir: &std::path::Path, port: u16) -> Option<ServeGuard> {
    let child = Command::new("python3")
        .args(["-m", "http.server", &port.to_string(), "--bind", "127.0.0.1"])
        .arg("--directory")
        .arg(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    Some(ServeGuard(child))
}

pub fn run_once(
    scenario: &Scenario,
    rubric: &Rubric,
    opts: &RunOptions,
    index: u32,
) -> anyhow::Result<RunOutput> {
    let run_dir = opts.runs_root.join(&scenario.id).join(format!("run-{index}"));
    std::fs::create_dir_all(&run_dir)?;
    let project_name = format!("{}_{}", sanitize(&scenario.id), index);

    eprintln!("[run {index}] scaffolding {project_name}…");
    let scaffolded = scaffold::create(&project_name, &run_dir, &opts.framework_path)?;
    let project_dir = scaffolded.project_dir.clone();
    let mcp_config = project_dir.join(".mcp.json");

    let prompt = format!("{}\n\n{}", opts.preamble.trim(), scenario.prompt.trim());
    eprintln!("[run {index}] running implementation agent…");
    let agent: AgentRun = agent::implement(
        &prompt,
        &project_dir,
        &mcp_config,
        &run_dir,
        opts.budget_usd,
    )?;
    eprintln!(
        "[run {index}] agent done: {} tool calls, {} tokens ({} MCP payload)",
        agent.transcript.calls.len(),
        agent.total_tokens(),
        agent.mcp_payload_tokens
    );

    // Build once; reuse the verdict for the compile tier and the dist for locate.
    eprintln!("[run {index}] building web…");
    let web_build = scaffold::build_web(&project_dir);
    let dist = web_build.as_ref().ok().cloned();

    // Locator pass for playwright-tier items (best-effort).
    let locate_dir = run_dir.join("locate");
    let mut ran_locate = false;
    if opts.locate {
        if let Some(dist) = &dist {
            let playwright_items: Vec<_> = rubric
                .items
                .iter()
                .filter(|i| i.tier == Tier::Playwright)
                .collect();
            if !playwright_items.is_empty() {
                let port = 8137 + (index as u16 % 200);
                if let Some(_guard) = serve(dist, port) {
                    // Give the server a moment to bind.
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    let base_url = format!("http://127.0.0.1:{port}");
                    let pw_cfg = locate::write_playwright_mcp_config(&run_dir)?;
                    for item in playwright_items {
                        eprintln!("[run {index}] locating `{}`…", item.id);
                        match locate::run_item(&base_url, item, &pw_cfg, &locate_dir, opts.budget_usd) {
                            Ok(v) => eprintln!("[run {index}]   → {}", v.passed),
                            Err(e) => eprintln!("[run {index}]   → locator error: {e}"),
                        }
                    }
                    ran_locate = true;
                } else {
                    eprintln!("[run {index}] python3 unavailable — skipping locator pass");
                }
            }
        } else {
            eprintln!("[run {index}] web build failed — playwright items will fail/neutralize");
        }
    }

    // Verify every item. Compile-web reuses the build we already did.
    let ctx = RunContext {
        project_dir: project_dir.clone(),
        transcript_path: Some(agent.transcript_path.clone()),
        locate_dir: if ran_locate { Some(locate_dir) } else { None },
    };
    let mut results: HashMap<String, VerifyResult> = HashMap::new();
    for item in &rubric.items {
        let result = if item.tier == Tier::Compile
            && item.assertion.target.as_deref() == Some("web")
        {
            match &web_build {
                Ok(_) => VerifyResult::pass("`idealyst build --web` succeeded"),
                Err(e) => VerifyResult::fail(format!("{e}")),
            }
        } else {
            verifier_for(item.tier).verify(item, &ctx)
        };
        results.insert(item.id.clone(), result);
    }

    let scored = score_from_results(
        rubric,
        &results,
        agent.total_tokens(),
        agent.mcp_payload_tokens,
        scenario.token_budget,
    );

    let pathologies = metrics::analyze(&agent.transcript);
    let doc_bypass = metrics::doc_bypass_reads(&agent.transcript, &project_dir);

    // Persist artifacts.
    let mut md = report::render_markdown(&scenario.id, &scored);
    md.push_str(&report::render_pathologies(&pathologies));
    if doc_bypass > 0 {
        md.push_str(&format!(
            "\n> ⚠️ {doc_bypass} doc-bypass read(s): the agent read framework source instead of asking the MCP.\n"
        ));
    }
    std::fs::write(run_dir.join("report.md"), &md)?;
    std::fs::write(run_dir.join("scored.json"), serde_json::to_string_pretty(&scored)?)?;

    if opts.feedback {
        eprintln!("[run {index}] synthesizing feedback…");
        let inputs = feedback::FeedbackInputs {
            scenario_id: &scenario.id,
            scenario_prompt: &scenario.prompt,
            scored: &scored,
            pathologies: &pathologies,
            doc_bypass_reads: doc_bypass,
            transcript_path: &agent.transcript_path,
        };
        if let Err(e) = feedback::synthesize(&inputs, &run_dir) {
            eprintln!("[run {index}] feedback skipped: {e}");
        }
    }

    Ok(RunOutput {
        scored,
        pathologies,
        doc_bypass_reads: doc_bypass,
        run_dir,
        project_dir,
    })
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
