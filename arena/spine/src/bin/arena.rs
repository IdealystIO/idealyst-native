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

use arena_spine::harness::{agent, locate, robot_web, scaffold};
use arena_spine::{
    metrics, quality, report, rubric::Rubric, rubric::Tier, score::score_run, score::ScoredRun,
    verify::RunContext, Scenario,
};
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
            // Same parser as `score`: handles both subagent logs and
            // `claude -p --output-format stream-json` captures (JSONL).
            let run = agent::load_session_jsonl(&transcript)?;
            print!("{}", report::render_pathologies(&metrics::analyze(&run.transcript)));
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
            // Scenario starting state (debug-fix / perf scenarios ship code).
            let assets = scenario_dir.join("assets");
            if assets.is_dir() {
                let n =
                    scaffold::overlay_assets(&assets, &scaffolded.project_dir, &framework_path)?;
                eprintln!("overlaid {n} scenario asset file(s)");
            }
            // Sidecar services are container-level: they must be enabled (and
            // the container restarted) BEFORE the run, so surface the exact
            // command on stderr rather than trying to apply it from inside.
            if !scenario.services.is_empty() {
                let flags: Vec<String> = scenario
                    .services
                    .iter()
                    .map(|s| format!("--service {s}"))
                    .collect();
                eprintln!(
                    "NOTE: scenario `{}` needs sidecar service(s) [{}] running in the arena \
                     container. If missing, enable them and restart the container:\n  \
                     idealyst configure devcontainer --config arena --non-interactive {}",
                    scenario.id,
                    scenario.services.join(", "),
                    flags.join(" "),
                );
            }
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
            let (total_tokens, mcp_payload, transcript_path, cache_read_tokens) = match &agent_run {
                Some(a) => (
                    a.total_tokens(),
                    a.mcp_payload_tokens,
                    Some(a.transcript_path.clone()),
                    Some(a.cache_read_tokens),
                ),
                None => (0, 0, None, None),
            };

            let locate_dir = opt(&args, "--locate-dir").map(PathBuf::from);
            let transcript_path_str = transcript_path
                .as_ref()
                .map(|p| p.display().to_string());
            let ctx = RunContext {
                project_dir: project_dir.clone(),
                transcript_path,
                locate_dir,
            };
            let mut scored = score_run(&rubric, &ctx, total_tokens, mcp_payload, scenario.token_budget);
            scored.model = agent_run.as_ref().and_then(|a| a.model.clone());

            // Pathologies + doc-bypass come from the parsed transcript (empty
            // when none was supplied).
            let transcript = agent_run.map(|a| a.transcript).unwrap_or_default();
            let pathologies = metrics::analyze(&transcript);
            let doc_bypass = metrics::doc_bypass_reads(&transcript, &project_dir);

            let mut md = report::render_markdown(&scenario.id, &scored);
            if let Some(model) = &scored.model {
                md.push_str(&format!("\n**Model:** `{model}`\n"));
            }
            if let Some(cache_read) = cache_read_tokens {
                md.push_str(&format!(
                    "\n**Context pressure:** {cache_read} cache-read tokens across the run (not part of the total).\n"
                ));
            }
            md.push_str(&report::render_pathologies(&pathologies));
            if doc_bypass > 0 {
                md.push_str(&format!(
                    "\n> ⚠️ {doc_bypass} doc-bypass read(s): the agent read framework source instead of asking the MCP.\n"
                ));
            }
            std::fs::write(run_dir.join("report.md"), &md)?;
            std::fs::write(run_dir.join("scored.json"), serde_json::to_string_pretty(&scored)?)?;
            // process.json: the model-assessment layer's deterministic core —
            // consumed by `arena feedback-prompt` and the aggregate view.
            let process = serde_json::json!({
                "pathologies": pathologies,
                "doc_bypass_reads": doc_bypass,
                "cache_read_tokens": cache_read_tokens,
                "model": scored.model,
                "transcript_path": transcript_path_str,
            });
            std::fs::write(run_dir.join("process.json"), serde_json::to_string_pretty(&process)?)?;

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
        // ── Outcome-tier support (Phase 2) ────────────────────────────────
        "live" => {
            // Serve a robot-enabled web build + relay + headless client, and
            // stay up so `arena score` (robot tier) and the locator (playwright
            // tier) have a running app. Blocks until killed; run it in the
            // background and tear it down (or the container) after scoring.
            let project_dir = PathBuf::from(pos(&args, 0, "project_dir")?);
            let port: u16 = opt(&args, "--port").and_then(|s| s.parse().ok()).unwrap_or(8095);
            let dist = project_dir.join("dist").join("web");
            anyhow::ensure!(
                dist.join("index.html").is_file(),
                "{} has no dist/web/index.html — run `arena build <project> --robot` first",
                project_dir.display()
            );
            let relay = robot_web::start_relay(&project_dir, &dist)?;
            let bridge = relay.tcp_addr;
            let mut server = std::process::Command::new("python3")
                .arg("-m")
                .arg("http.server")
                .arg(port.to_string())
                .arg("--bind")
                .arg("0.0.0.0")
                .arg("--directory")
                .arg(&dist)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| anyhow::anyhow!("starting python3 http.server: {e}"))?;
            let profile = project_dir.join(".idealyst").join("arena-live-profile");
            let host = match robot_web::host(relay, &format!("http://127.0.0.1:{port}"), &profile) {
                Ok(h) => h,
                Err(e) => {
                    let _ = server.kill();
                    return Err(e);
                }
            };
            // stdout is the machine-readable handle for the orchestrator.
            println!("LIVE bridge={bridge} url=http://127.0.0.1:{port}");
            // Hold everything until killed; guard's Drop tears the browser
            // down, and we take the http server with us.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
                let _ = &host;
                let _ = &server;
            }
        }
        "locate-prompts" => {
            // Per playwright-tier item: the exact prompt for the arena-locator
            // subagent. JSON on stdout; the orchestrator feeds each prompt to
            // the locator and pipes its reply into `arena verdict`.
            let scenario_dir = PathBuf::from(pos(&args, 0, "scenario_dir")?);
            let base_url = opt(&args, "--base-url")
                .ok_or_else(|| anyhow::anyhow!("missing --base-url"))?;
            let (_scenario, rubric) = load(&scenario_dir)?;
            let prompts: Vec<serde_json::Value> = rubric
                .items
                .iter()
                .filter(|i| i.tier == Tier::Playwright)
                .map(|i| {
                    serde_json::json!({
                        "item_id": i.id,
                        "prompt": locate::build_prompt(&base_url, i),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&prompts)?);
            Ok(())
        }
        "verdict" => {
            // Persist one locator reply (read from stdin, prose-tolerant) as
            // the deterministic verdict file the playwright verifier reads.
            let locate_dir = PathBuf::from(pos(&args, 0, "locate_dir")?);
            let item_id = pos(&args, 1, "item_id")?.clone();
            let mut raw = String::new();
            use std::io::Read as _;
            std::io::stdin().read_to_string(&mut raw)?;
            let verdict = locate::parse_verdict(&raw);
            let path = locate::write_verdict(&locate_dir, &item_id, &verdict)?;
            println!("{} passed={}", path.display(), verdict.passed);
            Ok(())
        }
        // ── Model-assessment support (Phase 3) ────────────────────────────
        "feedback-prompt" => {
            // Rebuild the two-pass feedback prompt from persisted artifacts.
            let scenario_dir = PathBuf::from(pos(&args, 0, "scenario_dir")?);
            let run_dir = PathBuf::from(pos(&args, 1, "run_dir")?);
            let (scenario, _rubric) = load(&scenario_dir)?;
            let scored: ScoredRun =
                serde_json::from_str(&std::fs::read_to_string(run_dir.join("scored.json"))?)?;
            let process: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(run_dir.join("process.json"))?)?;
            let pathologies: metrics::Pathologies =
                serde_json::from_value(process.get("pathologies").cloned().unwrap_or_default())?;
            let doc_bypass = process
                .get("doc_bypass_reads")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let transcript_path = process
                .get("transcript_path")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| run_dir.join("impl-transcript.jsonl"));
            let prompt = arena_spine::harness::feedback::build_feedback_prompt(
                &arena_spine::harness::feedback::FeedbackInputs {
                    scenario_id: &scenario.id,
                    scenario_prompt: &scenario.prompt,
                    scored: &scored,
                    pathologies: &pathologies,
                    doc_bypass_reads: doc_bypass,
                    transcript_path: &transcript_path,
                },
            );
            print!("{prompt}");
            Ok(())
        }
        // ── Qualitative layer (Phase 4) ───────────────────────────────────
        "quality" => {
            let project_dir = PathBuf::from(pos(&args, 0, "project_dir")?);
            let run_dir = PathBuf::from(
                opt(&args, "--run-dir").ok_or_else(|| anyhow::anyhow!("missing --run-dir"))?,
            );
            std::fs::create_dir_all(&run_dir)?;
            let lint = quality::run_lint(&project_dir)?;
            let judge = match opt(&args, "--judge-file") {
                Some(p) => {
                    let raw = std::fs::read_to_string(&p)?;
                    // Prose-tolerant like the locator path: take the outermost
                    // JSON object if the judge wrapped it in text.
                    let json_span = match (raw.find('{'), raw.rfind('}')) {
                        (Some(s), Some(e)) if e > s => &raw[s..=e],
                        _ => raw.trim(),
                    };
                    let v: quality::JudgeVerdict = serde_json::from_str(json_span)
                        .map_err(|e| anyhow::anyhow!("malformed judge verdict {p}: {e}"))?;
                    quality::validate_judge(&v)?;
                    Some(v)
                }
                None => None,
            };
            let q = quality::QualityReport { lint, judge };
            std::fs::write(run_dir.join("quality.json"), serde_json::to_string_pretty(&q)?)?;
            std::fs::write(run_dir.join("quality.md"), quality::render_markdown(&q))?;
            println!(
                "quality: lint {} ({} diagnostics), judge: {} — artifacts: {}",
                if q.lint.clean { "clean" } else { "ISSUES" },
                q.lint.diagnostics,
                q.judge
                    .as_ref()
                    .map(|j| format!("{} dimension(s)", j.dimensions.len()))
                    .unwrap_or_else(|| "none".into()),
                run_dir.display()
            );
            Ok(())
        }
        "judge-prompt" => {
            let project_dir = PathBuf::from(pos(&args, 0, "project_dir")?);
            let screenshots = opt(&args, "--screenshots").map(PathBuf::from);
            print!("{}", quality::build_judge_prompt(&project_dir, screenshots.as_deref()));
            Ok(())
        }
        // ── Adversarial review (non-scoring) ──────────────────────────────
        "adversary-prompt" => {
            let project_dir = PathBuf::from(pos(&args, 0, "project_dir")?);
            let framework_dir = PathBuf::from(
                opt(&args, "--framework").unwrap_or_else(|| "/workspaces/idealyst-native".into()),
            );
            print!(
                "{}",
                arena_spine::adversary::build_prompt(&project_dir, &framework_dir)
            );
            Ok(())
        }
        "adversary" => {
            // Validate + persist the adversary's findings (prose-tolerant read
            // of --findings-file), writing adversary.json / adversary.md.
            let _project_dir = PathBuf::from(pos(&args, 0, "project_dir")?);
            let run_dir = PathBuf::from(
                opt(&args, "--run-dir").ok_or_else(|| anyhow::anyhow!("missing --run-dir"))?,
            );
            let file = opt(&args, "--findings-file")
                .ok_or_else(|| anyhow::anyhow!("missing --findings-file"))?;
            let raw = std::fs::read_to_string(&file)?;
            let json_span = match (raw.find('{'), raw.rfind('}')) {
                (Some(s), Some(e)) if e > s => &raw[s..=e],
                _ => raw.trim(),
            };
            let report: arena_spine::adversary::AdversaryReport = serde_json::from_str(json_span)
                .map_err(|e| anyhow::anyhow!("malformed adversary report {file}: {e}"))?;
            arena_spine::adversary::validate(&report)?;
            std::fs::create_dir_all(&run_dir)?;
            std::fs::write(
                run_dir.join("adversary.json"),
                serde_json::to_string_pretty(&report)?,
            )?;
            std::fs::write(
                run_dir.join("adversary.md"),
                arena_spine::adversary::render_markdown(&report),
            )?;
            let (c, m, mi) = report.findings.iter().fold((0, 0, 0), |acc, f| {
                use arena_spine::adversary::Severity::*;
                match f.severity {
                    Critical => (acc.0 + 1, acc.1, acc.2),
                    Major => (acc.0, acc.1 + 1, acc.2),
                    Minor => (acc.0, acc.1, acc.2 + 1),
                }
            });
            println!(
                "adversary: {c} critical, {m} major, {mi} minor — artifacts: {}",
                run_dir.display()
            );
            Ok(())
        }
        // ── Longitudinal ledger ───────────────────────────────────────────
        "record" => {
            // Append one run's results to the scenario's CSV pair (summary +
            // per-item long format) under arena/results/ — the
            // tracked-over-time record, shaped to lift into a database later.
            let scenario_dir = PathBuf::from(pos(&args, 0, "scenario_dir")?);
            let run_dir = PathBuf::from(pos(&args, 1, "run_dir")?);
            let (scenario, _rubric) = load(&scenario_dir)?;
            let scored: ScoredRun =
                serde_json::from_str(&std::fs::read_to_string(run_dir.join("scored.json"))?)?;
            let process: Option<serde_json::Value> = std::fs::read_to_string(run_dir.join("process.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());
            let quality: Option<arena_spine::quality::QualityReport> =
                std::fs::read_to_string(run_dir.join("quality.json"))
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok());

            let run_id = run_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("run-unknown")
                .to_string();
            let commit = opt(&args, "--commit").unwrap_or_else(|| {
                std::process::Command::new("git")
                    .args(["rev-parse", "--short", "HEAD"])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_else(|| "unknown".into())
            });
            let epoch = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let out_dir = opt(&args, "--out").map(PathBuf::from).unwrap_or_else(|| {
                scenario_dir
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("results"))
                    .unwrap_or_else(|| PathBuf::from("results"))
            });
            let force = args.iter().any(|a| a == "--force");

            let rec = arena_spine::record::RunRecord {
                scenario_id: &scenario.id,
                run_id: &run_id,
                framework_commit: &commit,
                recorded_at_epoch: epoch,
                scored: &scored,
                process: process.as_ref(),
                quality: quality.as_ref(),
            };
            let row = arena_spine::record::summary_row(&rec);
            let items = arena_spine::record::item_rows(&rec);
            let appended =
                arena_spine::record::append(&out_dir, &scenario.id, &run_id, &row, &items, force)?;
            if appended {
                println!(
                    "recorded {}/{} → {}",
                    scenario.id,
                    run_id,
                    out_dir.join(format!("{}.csv", scenario.id)).display()
                );
            } else {
                println!(
                    "{}/{} already recorded — pass --force to append a re-record",
                    scenario.id, run_id
                );
            }
            Ok(())
        }
        // ── Aggregation (Phase 5) ─────────────────────────────────────────
        "aggregate" => {
            let scenario_id = pos(&args, 0, "scenario_id")?.clone();
            let files = positionals(&args);
            anyhow::ensure!(files.len() > 1, "need at least one scored.json after the scenario id");
            let mut runs = Vec::new();
            for f in &files[1..] {
                let scored: ScoredRun = serde_json::from_str(&std::fs::read_to_string(f)?)
                    .map_err(|e| anyhow::anyhow!("parsing {f}: {e}"))?;
                runs.push(scored);
            }
            let agg = arena_spine::aggregate(&runs);
            print!("{}", report::render_aggregate(&scenario_id, &agg));
            Ok(())
        }
        other => anyhow::bail!(
            "unknown command `{other}`\nusage:\n  \
             arena verify          <scenario_dir> <project_dir>\n  \
             arena metrics         <transcript.jsonl>\n  \
             arena scaffold        <scenario_dir> <framework_path> --run-dir <dir> [--index <n>]\n  \
             arena build           <project_dir> [--robot]\n  \
             arena live            <project_dir> [--port <p>]           (blocks; serve+relay+headless client)\n  \
             arena locate-prompts  <scenario_dir> --base-url <url>      (playwright-tier prompts, JSON)\n  \
             arena verdict         <locate_dir> <item_id>  < reply.txt  (persist a locator reply)\n  \
             arena score           <scenario_dir> <project_dir> --run-dir <dir> [--impl-transcript <jsonl>] [--locate-dir <dir>]\n  \
             arena feedback-prompt <scenario_dir> <run_dir>             (two-pass reviewer prompt)\n  \
             arena quality         <project_dir> --run-dir <dir> [--judge-file <json>]\n  \
             arena judge-prompt    <project_dir> [--screenshots <dir>]\n  \
             arena record          <scenario_dir> <run_dir> [--out <dir>] [--commit <sha>] [--force]\n  \
             arena aggregate       <scenario_id> <scored.json>..."
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
const VALUE_FLAGS: &[&str] = &[
    "--run-dir",
    "--index",
    "--impl-transcript",
    "--locate-dir",
    "--base-url",
    "--port",
    "--judge-file",
    "--screenshots",
    "--out",
    "--commit",
    "--framework",
    "--findings-file",
];

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
