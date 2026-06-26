//! Static tier: assert (or refute) a regex over the produced source. The
//! cheapest objective check — proves the agent *chose the right API* without
//! building or running anything.

use super::{RunContext, VerifyResult, Verifier};
use crate::rubric::RubricItem;
use regex::Regex;

pub struct StaticAstVerifier;

impl Verifier for StaticAstVerifier {
    fn verify(&self, item: &RubricItem, ctx: &RunContext) -> VerifyResult {
        let Some(pattern) = &item.assertion.pattern else {
            return VerifyResult::fail("static item has no `pattern`");
        };
        let re = match Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => return VerifyResult::fail(format!("bad regex `{pattern}`: {e}")),
        };

        let glob_rel = item.assertion.glob.as_deref().unwrap_or("**/*.rs");
        let glob_abs = ctx.project_dir.join(glob_rel);
        let glob_str = glob_abs.to_string_lossy();

        let mut total = 0usize;
        let mut first_hit: Option<String> = None;
        let mut files_seen = 0usize;

        let entries = match glob::glob(&glob_str) {
            Ok(e) => e,
            Err(e) => return VerifyResult::fail(format!("bad glob `{glob_rel}`: {e}")),
        };
        for entry in entries.flatten() {
            if !entry.is_file() {
                continue;
            }
            files_seen += 1;
            let Ok(content) = std::fs::read_to_string(&entry) else {
                continue; // binary / unreadable — skip
            };
            for m in re.find_iter(&content) {
                total += 1;
                if first_hit.is_none() {
                    let rel = entry
                        .strip_prefix(&ctx.project_dir)
                        .unwrap_or(&entry)
                        .display();
                    first_hit = Some(format!("{rel}: `{}`", m.as_str()));
                }
            }
        }

        if item.assertion.absent {
            // Must NOT appear anywhere.
            if total == 0 {
                VerifyResult::pass(format!("`{pattern}` absent across {files_seen} file(s)"))
            } else {
                VerifyResult::fail(format!(
                    "`{pattern}` found {total}× but must be absent — {}",
                    first_hit.unwrap_or_default()
                ))
            }
        } else {
            let need = item.assertion.min_count.unwrap_or(1);
            if total >= need {
                VerifyResult::pass(format!(
                    "`{pattern}` matched {total}× (need {need}) — {}",
                    first_hit.unwrap_or_default()
                ))
            } else {
                VerifyResult::fail(format!(
                    "`{pattern}` matched {total}× across {files_seen} file(s), need {need}"
                ))
            }
        }
    }
}
