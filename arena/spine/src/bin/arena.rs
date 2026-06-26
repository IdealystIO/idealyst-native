//! `arena` CLI. The deterministic-spine entry point AND the live-run driver.
//!
//!   arena verify <scenario_dir> <project_dir>     score an existing tree (source tiers)
//!   arena metrics <transcript.jsonl>              transcript pathology report
//!   arena run   <scenario_dir> <framework_path> [opts]   one full live run
//!   arena bench <scenario_dir> <framework_path> [opts]   N runs + aggregate + feedback
//!
//! Live-run options:
//!   --runs-root <dir>   where run artifacts go (default ./runs)
//!   --budget <usd>      per-agent dollar cap (default 5)
//!   --index <n>         run index for `run` (default 0)
//!   --no-locate         skip the Playwright locator pass
//!   --no-feedback       skip the feedback agent

use arena_spine::harness::bench::run_bench;
use arena_spine::harness::run::{run_once, RunOptions};
use arena_spine::{metrics, report, rubric::Rubric, score::score_run, verify::RunContext, Scenario};
use std::path::PathBuf;

const DEFAULT_PREAMBLE: &str = "idealyst MCP available, use it.";

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
        "run" => {
            let scenario_dir = PathBuf::from(pos(&args, 0, "scenario_dir")?);
            let framework_path = PathBuf::from(pos(&args, 1, "framework_path")?);
            let (scenario, rubric) = load(&scenario_dir)?;
            let opts = run_options(&scenario_dir, framework_path, &args)?;
            let index = opt(&args, "--index").and_then(|s| s.parse().ok()).unwrap_or(0);
            let out = run_once(&scenario, &rubric, &opts, index)?;
            println!(
                "\n{} → {} / {} pts (final {:.3}) — artifacts: {}",
                scenario.id,
                out.scored.rubric_points,
                out.scored.max_points,
                out.scored.final_score,
                out.run_dir.display()
            );
            Ok(())
        }
        "bench" => {
            let scenario_dir = PathBuf::from(pos(&args, 0, "scenario_dir")?);
            let framework_path = PathBuf::from(pos(&args, 1, "framework_path")?);
            let (scenario, rubric) = load(&scenario_dir)?;
            let opts = run_options(&scenario_dir, framework_path, &args)?;
            let out = run_bench(&scenario, &rubric, &opts)?;
            println!(
                "\n{}: {}/{} runs completed · mean {:.1} pts · report: {}",
                scenario.id,
                out.completed,
                out.requested,
                out.aggregate.mean_points,
                out.bench_report.display()
            );
            Ok(())
        }
        other => anyhow::bail!(
            "unknown command `{other}`\nusage:\n  arena verify <scenario_dir> <project_dir>\n  arena metrics <transcript.jsonl>\n  arena run   <scenario_dir> <framework_path> [opts]\n  arena bench <scenario_dir> <framework_path> [opts]"
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

fn run_options(
    scenario_dir: &std::path::Path,
    framework_path: PathBuf,
    args: &[String],
) -> anyhow::Result<RunOptions> {
    // The preamble is the one identical-across-runs artifact. Load it from
    // <arena>/agents/preamble.md (scenario_dir is <arena>/scenarios/<id>).
    let preamble_path = scenario_dir
        .join("..")
        .join("..")
        .join("agents")
        .join("preamble.md");
    let preamble = std::fs::read_to_string(&preamble_path)
        .map(|s| preamble_body(&s))
        .unwrap_or_else(|_| DEFAULT_PREAMBLE.to_string());

    Ok(RunOptions {
        framework_path,
        runs_root: opt(args, "--runs-root")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("runs")),
        preamble,
        budget_usd: opt(args, "--budget").and_then(|s| s.parse().ok()).unwrap_or(5.0),
        locate: !args.iter().any(|a| a == "--no-locate"),
        feedback: !args.iter().any(|a| a == "--no-feedback"),
    })
}

/// Strip a leading Markdown comment/heading block from the preamble file, using
/// the first non-heading, non-empty line onward — so the file can carry a note
/// for humans without it reaching the agent.
fn preamble_body(s: &str) -> String {
    let body: Vec<&str> = s
        .lines()
        .skip_while(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
        .collect();
    let joined = body.join("\n");
    if joined.trim().is_empty() {
        DEFAULT_PREAMBLE.to_string()
    } else {
        joined.trim().to_string()
    }
}

fn pos<'a>(args: &'a [String], i: usize, what: &str) -> anyhow::Result<&'a String> {
    // Positionals are the non-flag args (flags start with `--` and may consume
    // the following value).
    positionals(args)
        .get(i)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("missing argument: {what}"))
}

fn positionals(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for (i, a) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a.starts_with("--") {
            // Flags that take a value consume the next arg.
            if matches!(a.as_str(), "--runs-root" | "--budget" | "--index") {
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
