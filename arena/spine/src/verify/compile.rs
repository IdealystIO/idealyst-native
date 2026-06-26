//! Compile tier: the project must build. The binary, objective floor — if it
//! doesn't compile, no outcome item can possibly pass.
//!
//! `target = "check"` (default) runs `cargo check` for a fast type-level gate.
//! Any other target runs `idealyst build --<target>` so a web/macos/… build is
//! exercised end to end. If the build tool isn't on PATH the item is
//! **skipped**, not failed — a missing toolchain is the harness's problem, not
//! the agent's.

use super::{RunContext, VerifyResult, Verifier};
use crate::rubric::RubricItem;
use std::process::Command;

pub struct CompileVerifier;

impl Verifier for CompileVerifier {
    fn verify(&self, item: &RubricItem, ctx: &RunContext) -> VerifyResult {
        let target = item.assertion.target.as_deref().unwrap_or("check");
        let (program, args): (&str, Vec<&str>) = match target {
            "check" => ("cargo", vec!["check", "--quiet", "--locked"]),
            other => ("idealyst", vec!["build", "--", other]),
        };
        // `idealyst build --<target>`: rebuild the arg vec with the flag form.
        let args: Vec<String> = if program == "idealyst" {
            vec!["build".into(), format!("--{target}")]
        } else {
            args.into_iter().map(String::from).collect()
        };

        let output = Command::new(program)
            .args(&args)
            .current_dir(&ctx.project_dir)
            .output();

        match output {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                VerifyResult::skip(format!("`{program}` not on PATH — compile tier skipped"))
            }
            Err(e) => VerifyResult::fail(format!("spawning `{program}` failed: {e}")),
            Ok(out) if out.status.success() => {
                VerifyResult::pass(format!("`{program} {}` succeeded", args.join(" ")))
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let tail: String = stderr.lines().rev().take(8).collect::<Vec<_>>()
                    .into_iter().rev().collect::<Vec<_>>().join("\n");
                VerifyResult::fail(format!(
                    "`{program} {}` failed (code {:?}):\n{tail}",
                    args.join(" "),
                    out.status.code()
                ))
            }
        }
    }
}
