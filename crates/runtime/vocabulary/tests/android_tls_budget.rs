//! Android's bionic libc caps a process at 128 pthread TLS keys, and with
//! the Android target's emulated TLS every `thread_local!` that holds a
//! value with a destructor spends one when first touched. Past the cap the
//! next one aborts the app (`LazyKey::lazy_init`) — on whichever code path
//! reaches it first: a real touch's `user_activity` listener, then the
//! async executor's task table, the day the robot's native tap started
//! sending real touches to the robot-e2e demo.
//!
//! A `const`-initialised thread-local of a destructor-free type (a `Cell`
//! of a number, a `bool`, an `Option<&'static _>` / `Option<fn>`) is plain
//! TLS and spends none. This pins how many key-spending thread-locals the
//! vocabulary declares: a new one must replace an old one (fold it into an
//! existing slot struct, or lift it to a `const` no-drop slot) or raise the
//! pin knowingly.

use std::path::Path;

/// Key-spending `thread_local!`s in `src/` (cfg-gated ones included).
const PINNED: usize = 52;

fn count(dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            count(&path, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let mut rest = src.as_str();
        while let Some(at) = rest.find("thread_local!") {
            rest = &rest[at + "thread_local!".len()..];
            let Some(open) = rest.find('{') else { break };
            // The macro body: up to the matching brace.
            let (mut depth, mut end) = (0usize, 0usize);
            for (i, c) in rest[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body = &rest[open + 1..end];
            for decl in body.split("static ").skip(1) {
                let Some((head, init)) = decl.split_once('=') else { continue };
                let ty = head.split_once(':').map(|(_, t)| t.trim()).unwrap_or("");
                let free = init.trim_start().starts_with("const")
                    && ["Cell<u", "Cell<i", "Cell<f", "Cell<bool", "Cell<usize", "Cell<Option<&", "Cell<Option<fn", "Cell<*"]
                        .iter()
                        .any(|p| ty.trim_start_matches("std::cell::").trim_start_matches("core::cell::").starts_with(p));
                if !free {
                    let name = head.split(':').next().unwrap_or("").trim();
                    out.push(format!("{}: {name}", path.display()));
                }
            }
            rest = &rest[end..];
        }
    }
}

#[test]
fn the_vocabulary_spends_no_more_android_tls_keys() {
    let mut keyed = Vec::new();
    count(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut keyed);
    assert!(
        keyed.len() <= PINNED,
        "{} key-spending thread_local!s (pinned {PINNED}) — each costs one of Android's 128 pthread keys; \
         fold the new one into an existing slot or make it a const no-drop Cell:\n{}",
        keyed.len(),
        keyed.join("\n")
    );
}
