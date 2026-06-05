//! End-to-end: `idealyst mcp` live-reloads its catalog when project
//! source changes — without restarting the server.
//!
//! This is the real thing, not a unit test of a seam:
//!
//! 1. Materialize a throwaway project (one `#[component]`, plus an
//!    `idea-ui` dependency) in a temp dir, path-pinned to the in-tree
//!    framework crates so it resolves in **workspace mode** (hermetic,
//!    no network / git fetch).
//! 2. Spawn the actual `idealyst mcp` **binary** and talk to it as an
//!    MCP client over stdio (default watch is ON — `src/` + `Cargo.toml`).
//! 3. Assert the project's own component AND a dependency's components
//!    (force-linked into the catalog wrapper) are listed.
//! 4. **Add a component** to the project's source, then poll until it
//!    appears in `list_components` — proving the watcher rebuilt the
//!    catalog and swapped it into the live server.
//! 5. **Change a doc comment** and poll until `describe_component`
//!    reflects the new text — proving documentation edits propagate too.
//!
//! Marked `#[ignore]`: it drives real `cargo` builds of the catalog
//! wrapper (compiles `runtime-core` with the `catalog` feature + the
//! fixture + `idea-ui`), so it's minutes-slow and toolchain-dependent.
//! Run it explicitly:
//!
//! ```text
//! cargo test -p idealyst-cli --test mcp_live_reload_e2e -- --ignored --nocapture
//! ```
//!
//! The first connect blocks on the server's initial catalog build; rmcp
//! applies no request timeout by default, so the handshake simply waits
//! it out.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;

/// Workspace root, derived from this crate's manifest dir
/// (`<root>/crates/tools/cli`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crates/tools/cli has a workspace root 3 levels up")
        .to_path_buf()
}

/// The fixture's `src/lib.rs`, parameterized so the test can rewrite it
/// to (a) add a component and (b) change a doc comment.
fn fixture_lib(widget_doc: &str, include_gadget: bool) -> String {
    // Gadget needs its OWN props type: `#[component]` emits one
    // `BuildElement` impl per props struct, so two components sharing a
    // props type would collide (E0119).
    let gadget = if include_gadget {
        r#"
#[derive(Default)]
pub struct GadgetProps {}

/// A second component, added while the server is running, to prove the
/// catalog live-reloads on a source edit.
#[component]
pub fn Gadget(props: &GadgetProps) -> Element {
    let _ = props;
    ui! { view() {} }
}
"#
    } else {
        ""
    };
    format!(
        r#"//! Throwaway fixture crate for the MCP live-reload e2e test.

use runtime_core::{{component, ui, Element}};

#[derive(Default)]
pub struct WidgetProps {{}}

/// {widget_doc}
#[component]
pub fn Widget(props: &WidgetProps) -> Element {{
    let _ = props;
    ui! {{ view() {{}} }}
}}
{gadget}"#,
        widget_doc = widget_doc,
        gadget = gadget,
    )
}

/// Lay down the fixture project. Path-deps the in-tree `runtime-core` +
/// `idea-ui` (absolute paths) so `FrameworkSource::detect` resolves
/// workspace mode and the build is hermetic.
fn materialize_fixture(dir: &Path, repo: &Path) -> Result<()> {
    std::fs::create_dir_all(dir.join("src")).context("create fixture src/")?;
    let cargo_toml = format!(
        r#"[package]
name = "mcp_e2e_fixture"
version = "0.0.1"
edition = "2021"
publish = false

[lib]
path = "src/lib.rs"

[dependencies]
runtime-core = {{ path = "{core}" }}
idea-ui = {{ path = "{ui}" }}
"#,
        core = repo.join("crates/runtime/core").display(),
        ui = repo.join("crates/ui/idea-ui").display(),
    );
    std::fs::write(dir.join("Cargo.toml"), cargo_toml).context("write fixture Cargo.toml")?;
    std::fs::write(
        dir.join("src/lib.rs"),
        fixture_lib("DOC_V1 the original widget documentation.", false),
    )
    .context("write fixture src/lib.rs")?;
    Ok(())
}

/// Pull the catalog out of a `list_components` / `describe_component`
/// tool result. The handlers return the JSON as a single text content.
fn result_text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default()
}

/// Names returned by `list_components`.
async fn list_component_names<S>(client: &S) -> Result<Vec<String>>
where
    S: std::ops::Deref<Target = rmcp::service::Peer<rmcp::RoleClient>>,
{
    let res = client
        .call_tool(CallToolRequestParams::new("list_components"))
        .await
        .context("call list_components")?;
    let json: serde_json::Value =
        serde_json::from_str(&result_text(&res)).context("parse list_components json")?;
    Ok(json
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

/// Raw `describe_component` text for `name` (the pretty JSON record).
async fn describe_component<S>(client: &S, name: &str) -> Result<String>
where
    S: std::ops::Deref<Target = rmcp::service::Peer<rmcp::RoleClient>>,
{
    let mut params = CallToolRequestParams::new("describe_component");
    params.arguments = serde_json::json!({ "name": name })
        .as_object()
        .cloned();
    let res = client
        .call_tool(params)
        .await
        .context("call describe_component")?;
    Ok(result_text(&res))
}

/// Poll `cond` every 2s until it returns true or `budget` elapses.
async fn poll_until<F, Fut>(budget: Duration, mut cond: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + budget;
    loop {
        if cond().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow: drives real cargo builds of the catalog wrapper; run with --ignored"]
async fn mcp_server_live_reloads_catalog_on_source_and_doc_edits() -> Result<()> {
    let repo = repo_root();
    let tmp = tempfile::tempdir().context("make temp fixture dir")?;
    let proj = tmp.path();
    materialize_fixture(proj, &repo)?;

    // Spawn the real `idealyst mcp` server against the fixture. No
    // `--no-watch`, so the default watcher (src/ + Cargo.toml) is on and
    // the managed wrapper rebuilds on change. Inherit stderr so a build
    // failure in the fixture/wrapper is visible when the test fails.
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_idealyst"));
    cmd.arg("mcp")
        .arg("--project-root")
        .arg(proj)
        .arg("--no-robot")
        .env("RUST_LOG", "warn")
        .stderr(std::process::Stdio::inherit());

    // The connect handshake blocks on the server's first catalog build.
    let client = ()
        .serve(TokioChildProcess::new(cmd).context("spawn idealyst mcp")?)
        .await
        .context("MCP handshake with idealyst mcp")?;

    // --- 1. Initial catalog: project component + dependency components.
    let initial = list_component_names(&client).await?;
    assert!(
        initial.iter().any(|n| n == "Widget"),
        "fixture's own `Widget` component should be in the initial catalog; got {initial:?}"
    );
    assert!(
        !initial.iter().any(|n| n == "Gadget"),
        "`Gadget` must not exist before we add it; got {initial:?}"
    );
    // Force-linking: idea-ui is only a *dependency*, never referenced by
    // the fixture's code, yet its components must appear.
    assert!(
        initial.iter().any(|n| n == "Button"),
        "dependency (idea-ui) components should be force-linked into the catalog; got {initial:?}"
    );

    // --- 2. Add a component to the project source → it should appear.
    std::fs::write(
        proj.join("src/lib.rs"),
        fixture_lib("DOC_V1 the original widget documentation.", true),
    )
    .context("rewrite fixture lib.rs with Gadget")?;

    let appeared = poll_until(Duration::from_secs(300), || async {
        list_component_names(&client)
            .await
            .map(|names| names.iter().any(|n| n == "Gadget"))
            .unwrap_or(false)
    })
    .await;
    assert!(
        appeared,
        "`Gadget` did not appear in the live catalog within 300s after the source edit"
    );

    // --- 3. Change a doc comment → describe_component should reflect it.
    std::fs::write(
        proj.join("src/lib.rs"),
        fixture_lib("DOC_V2_UPDATED the revised widget documentation.", true),
    )
    .context("rewrite fixture lib.rs with updated Widget doc")?;

    let doc_updated = poll_until(Duration::from_secs(300), || async {
        describe_component(&client, "Widget")
            .await
            .map(|text| text.contains("DOC_V2_UPDATED"))
            .unwrap_or(false)
    })
    .await;
    assert!(
        doc_updated,
        "describe_component(Widget) did not reflect the updated doc within 300s"
    );

    // Tidy: stop the server, drop the temp project, remove the wrapper
    // crate the CLI generated under the shared target dir.
    let _ = client.cancel().await;
    let _ = std::fs::remove_dir_all(repo.join("target/idealyst/mcp_e2e_fixture"));
    Ok(())
}
