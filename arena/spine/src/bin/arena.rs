//! `arena` CLI — the deterministic spine, exposed as granular steps the
//! `arena-bench` skill stitches together around its subagent spawns.
//!
//!   arena verify   <scenario_dir> <project_dir>     score an existing tree (source tiers)
//!   arena metrics  <transcript.jsonl>               transcript pathology report
//!   arena scaffold <scenario_dir> <framework_path> --run-dir <dir> [--index <n>]
//!                                                   isolated project + idealyst-only .mcp.json
//!   arena build    <project_dir> [--robot]          best-effort `idealyst build --web`
//!   arena score    <scenario_dir> <project_dir> --run-dir <dir> [--impl-transcript <jsonl>]
//!                                                   verify every tier, score, write report.md + scored.json
//!
//! The agent roles are subagents the orchestrating session runs (see
//! `.claude/skills/arena-bench`); this binary never spawns `claude`.

use arena_spine::harness::{agent, scaffold};
use arena_spine::{metrics, report, rubric::Rubric, score::score_run, verify::RunContext, Scenario};
use std::path::PathBuf;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = if args.is_empty() {
        String::new()
    } else {
        args.remove(0)
    };

    match cmd.as_str() {
        "verify" => {
            let scenario_dir = PathBuf::from(pos(&args, 0, "scenario_dir")?);
            let project_dir = PathBuf::from(pos(&args, 1, "project_dir")?);
            let (scenario, rubric) = load(&scenario_dir)?;
            let ctx = RunContext::source_only(project_dir);
            let scored = score_run(&rubric, &ctx, 0, 0, scenario.token_budget);
            print!("{}", report::render_markdown(&scenario.id, &scored));
            Ok(())
        }
        "metrics" => {
            let transcript = PathBuf::from(pos(&args, 0, "transcript.jsonl")?);
            let t = metrics::Transcript::load(&transcript)?;
            print!("{}", report::render_pathologies(&metrics::analyze(&t)));
            Ok(())
        }
        "scaffold" => {
            let scenario_dir = PathBuf::from(pos(&args, 0, "scenario_dir")?);
            let framework_path = PathBuf::from(pos(&args, 1, "framework_path")?);
            let (scenario, _rubric) = load(&scenario_dir)?;
            let run_dir = PathBuf::from(
                opt(&args, "--run-dir").ok_or_else(|| anyhow::anyhow!("missing --run-dir"))?,
            );
            let index: u32 = opt(&args, "--index").and_then(|s| s.parse().ok()).unwrap_or(0);
            let name = format!("{}_{}", sanitize(&scenario.id), index);
            std::fs::create_dir_all(&run_dir)?;
            let scaffolded = scaffold::create(&name, &run_dir, &framework_path)?;
            // The project dir on stdout is the skill's handle for every later step.
            println!("{}", scaffolded.project_dir.display());
            Ok(())
        }
        "build" => {
            let project_dir = PathBuf::from(pos(&args, 0, "project_dir")?);
            let robot = args.iter().any(|a| a == "--robot");
            let dist = scaffold::build_web(&project_dir, robot)?;
            println!("{}", dist.display());
            Ok(())
        }
        "score" => {
            let scenario_dir = PathBuf::from(pos(&args, 0, "scenario_dir")?);
            let project_dir = PathBuf::from(pos(&args, 1, "project_dir")?);
            let (scenario, rubric) = load(&scenario_dir)?;
            let run_dir = PathBuf::from(
                opt(&args, "--run-dir").ok_or_else(|| anyhow::anyhow!("missing --run-dir"))?,
            );
            std::fs::create_dir_all(&run_dir)?;

            // Parse the implementation subagent's transcript, if the skill
            // captured one. Without it, the source tiers still score; only the
            // token bonus + pathologies go to zero.
            let agent_run = match opt(&args, "--impl-transcript") {
                Some(p) => Some(agent::load_session_jsonl(&PathBuf::from(p))?),
                None => None,
            };
            let (total_tokens, mcp_payload, transcript_path) = match &agent_run {
                Some(a) => (a.total_tokens(), a.mcp_payload_tokens, Some(a.transcript_path.clone())),
                None => (0, 0, None),
            };

            let locate_dir = opt(&args, "--locate-dir").map(PathBuf::from);
            let ctx = RunContext {
                project_dir: project_dir.clone(),
                transcript_path,
                locate_dir,
            };
            let scored = score_run(&rubric, &ctx, total_tokens, mcp_payload, scenario.token_budget);

            // Pathologies + doc-bypass come from the parsed transcript (empty
            // when none was supplied).
            let transcript = agent_run.map(|a| a.transcript).unwrap_or_default();
            let pathologies = metrics::analyze(&transcript);
            let doc_bypass = metrics::doc_bypass_reads(&transcript, &project_dir);

            let mut md = report::render_markdown(&scenario.id, &scored);
            md.push_str(&report::render_pathologies(&pathologies));
            if doc_bypass > 0 {
                md.push_str(&format!(
                    "\n> ⚠️ {doc_bypass} doc-bypass read(s): the agent read framework source instead of asking the MCP.\n"
                ));
            }
            std::fs::write(run_dir.join("report.md"), &md)?;
            std::fs::write(run_dir.join("scored.json"), serde_json::to_string_pretty(&scored)?)?;

            println!(
                "{} → {} / {} pts (final {:.3}) — artifacts: {}",
                scenario.id,
                scored.rubric_points,
                scored.max_points,
                scored.final_score,
                run_dir.display()
            );
            Ok(())
        }
        other => anyhow::bail!(
            "unknown command `{other}`\nusage:\n  \
             arena verify   <scenario_dir> <project_dir>\n  \
             arena metrics  <transcript.jsonl>\n  \
             arena scaffold <scenario_dir> <framework_path> --run-dir <dir> [--index <n>]\n  \
             arena build    <project_dir> [--robot]\n  \
             arena score    <scenario_dir> <project_dir> --run-dir <dir> [--impl-transcript <jsonl>] [--locate-dir <dir>]"
        ),
    }
}

fn load(scenario_dir: &std::path::Path) -> anyhow::Result<(Scenario, Rubric)> {
    let scenario = Scenario::load(&scenario_dir.join("scenario.toml"))?;
    let rubric = Rubric::load(&scenario_dir.join("rubric.toml"))?;
    anyhow::ensure!(
        rubric.scenario_id == scenario.id,
        "rubric.scenario_id `{}` != scenario.id `{}`",
        rubric.scenario_id,
        scenario.id
    );
    Ok((scenario, rubric))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn pos<'a>(args: &'a [String], i: usize, what: &str) -> anyhow::Result<&'a String> {
    // Positionals are the non-flag args (flags start with `--` and may consume
    // the following value).
    positionals(args)
        .get(i)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("missing argument: {what}"))
}

/// Flags that take a value (so the value isn't mistaken for a positional).
const VALUE_FLAGS: &[&str] = &["--run-dir", "--index", "--impl-transcript", "--locate-dir"];

fn positionals(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for (i, a) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a.starts_with("--") {
            if VALUE_FLAGS.contains(&a.as_str()) {
                skip_next = i + 1 < args.len();
            }
            continue;
        }
        out.push(a);
    }
    out
}

fn opt(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
