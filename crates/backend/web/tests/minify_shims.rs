//! Pins for the build.rs JS shim minifier (`build_support/js_min.rs`).
//!
//! The minifier runs at build time over `runtime/js/*.js`; a bug there
//! ships broken JS inside the wasm and only surfaces as a runtime shim
//! failure in the browser. These tests fail the build first.

include!("../build_support/js_min.rs");

use std::path::Path;

fn shim_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/js"))
}

#[test]
fn strips_comments_but_preserves_code_and_strings() {
    let src = r#"
// a leading comment
var url = 'https://example.com/x'; // trailing comment
var s = "not // a comment";
var t = `template ${a /* keep code, drop me */ + b} with // text`;
/* block */ var x = 1;
/* multi
   line */
var y = 2;
"#;
    let min = minify_js(src);
    assert!(min.contains("var url = 'https://example.com/x';"), "got:\n{min}");
    assert!(min.contains(r#"var s = "not // a comment";"#));
    // Template literal text survives verbatim; the comment inside the
    // ${} expression (code context) is stripped.
    assert!(min.contains("with // text"));
    assert!(min.contains("${a") && min.contains("+ b}") && !min.contains("drop me"));
    assert!(min.contains("var x = 1;"));
    assert!(min.contains("var y = 2;"));
    assert!(!min.contains("leading comment") && !min.contains("trailing comment"));
    assert!(!min.contains("block") && !min.contains("multi"));
}

/// ECMA-262: a block comment containing a line terminator IS a line
/// terminator for automatic semicolon insertion. The minifier must
/// replace such a comment with a newline (and a single-line one with a
/// space), or `a = b /* \n */ ++c` style code would change meaning.
#[test]
fn multiline_block_comment_stays_a_line_terminator() {
    let min = minify_js("let a = b /* x\ny */ + c\n");
    // The comment became a newline — the two halves must be on
    // separate lines, not joined.
    assert!(
        min.contains("let a = b\n+ c"),
        "multi-line comment must collapse to a LineTerminator, got:\n{min}"
    );
    let single = minify_js("let a = b /* x */ + c\n");
    // One line (the comment became a space, not a newline); interior
    // runs of spaces are irrelevant to JS.
    assert_eq!(single.lines().count(), 1, "got:\n{single}");
    assert_eq!(
        single.split_whitespace().collect::<Vec<_>>(),
        ["let", "a", "=", "b", "+", "c"],
        "got:\n{single}"
    );
}

#[test]
fn blank_lines_and_indentation_are_dropped_but_newlines_kept() {
    let min = minify_js("    let a = 1\n\n\n    return a\n");
    assert_eq!(min, "let a = 1\nreturn a\n");
}

/// The scanner has one documented blind spot: regex literals containing
/// `//` or `/*`. The shims must therefore contain no regex literals at
/// all — if you add one, teach the scanner first (see js_min.rs docs).
#[test]
fn shims_contain_no_regex_literals() {
    for entry in std::fs::read_dir(shim_dir()).expect("runtime/js") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            assert!(
                !code.contains(".match(/")
                    && !code.contains(".replace(/")
                    && !code.contains(".split(/")
                    && !code.contains("= /")
                    && !code.contains("new RegExp"),
                "{}:{}: looks like a regex literal — the shim minifier can't \
                 tokenize those; rework the JS or extend js_min.rs first",
                path.display(),
                i + 1,
            );
        }
    }
}

/// Every real shim, minified, must still be syntactically valid JS.
/// Uses `node --check` when node is on PATH (developer machines, CI);
/// silently skipped otherwise — the semantics pins above still run.
#[test]
fn minified_shims_parse_with_node_when_available() {
    let node_ok = std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !node_ok {
        eprintln!("node not found — skipping syntax check");
        return;
    }
    let tmp = std::env::temp_dir().join("idealyst-js-min-check");
    std::fs::create_dir_all(&tmp).unwrap();
    for entry in std::fs::read_dir(shim_dir()).expect("runtime/js") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let min = minify_js(&src);
        assert!(
            min.len() < src.len(),
            "{}: minification should shrink a commented shim",
            path.display()
        );
        let out = tmp.join(path.file_name().unwrap());
        std::fs::write(&out, &min).unwrap();
        let check = std::process::Command::new("node")
            .args(["--check", out.to_str().unwrap()])
            .output()
            .expect("run node --check");
        assert!(
            check.status.success(),
            "{}: minified shim fails to parse:\n{}",
            path.display(),
            String::from_utf8_lossy(&check.stderr),
        );
    }
}
