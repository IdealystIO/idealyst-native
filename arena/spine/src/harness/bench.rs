//! Run a scenario N times and aggregate. A failed run (the agent crashed, the
//! toolchain was missing) is logged and excluded — it doesn't poison the
//! aggregate, but the count of completed runs is reported so a thin sample is
//! visible.

use super::run::{run_once, RunOptions, RunOutput};
use crate::report;
use crate::rubric::Rubric;
use crate::scenario::Scenario;
use crate::{aggregate, Aggregate};

pub struct BenchOutput {
    pub aggregate: Aggregate,
    pub completed: u32,
    pub requested: u32,
    pub bench_report: std::path::PathBuf,
}

pub fn run_bench(
    scenario: &Scenario,
    rubric: &Rubric,
    opts: &RunOptions,
) -> anyhow::Result<BenchOutput> {
    let mut outputs: Vec<RunOutput> = Vec::new();
    for i in 0..scenario.runs {
        match run_once(scenario, rubric, opts, i) {
            Ok(o) => outputs.push(o),
            Err(e) => eprintln!("[run {i}] FAILED, excluded from aggregate: {e:#}"),
        }
    }

    let scored: Vec<_> = outputs.iter().map(|o| o.scored.clone()).collect();
    let agg = aggregate(&scored);

    let mut md = report::render_aggregate(&scenario.id, &agg);
    if (outputs.len() as u32) < scenario.runs {
        md.push_str(&format!(
            "> ⚠️ only {}/{} runs completed — aggregate is from a thin sample.\n\n",
            outputs.len(),
            scenario.runs
        ));
    }
    let total_bypass: usize = outputs.iter().map(|o| o.doc_bypass_reads).sum();
    if total_bypass > 0 {
        md.push_str(&format!(
            "> ⚠️ {total_bypass} total doc-bypass read(s) across runs (agent read framework source instead of the MCP).\n\n"
        ));
    }
    md.push_str("Per-run reports: `run-*/report.md` · feedback: `run-*/feedback.md`\n");

    let scenario_root = opts.runs_root.join(&scenario.id);
    std::fs::create_dir_all(&scenario_root)?;
    let bench_report = scenario_root.join("bench.md");
    std::fs::write(&bench_report, md)?;

    Ok(BenchOutput {
        aggregate: agg,
        completed: outputs.len() as u32,
        requested: scenario.runs,
        bench_report,
    })
}
