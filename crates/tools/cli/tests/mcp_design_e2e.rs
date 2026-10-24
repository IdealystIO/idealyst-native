//! End-to-end: `idealyst mcp design` serves the idea-ui design MCP.
//!
//! Spawns the real CLI binary and talks to it as an MCP client over stdio:
//! the handshake carries the design instructions, the tools/prompts/
//! resources are all listed, and a recipe, a skill section and a prompt
//! come back with their content. Fast (no project, no cargo build), so it
//! is not `#[ignore]`d — it guards the CLI wiring of the subcommand, which
//! the design crate's own tests can't see.

use anyhow::{Context, Result};
use rmcp::model::{CallToolRequestParams, CallToolResult, GetPromptRequestParams, ReadResourceRequestParams};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;

fn text(r: &CallToolResult) -> String {
    r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect()
}

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_design_serves_skills_recipes_and_prompts() -> Result<()> {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_idealyst"));
    cmd.args(["mcp", "design"]);
    let client = ()
        .serve(TokioChildProcess::new(cmd).context("spawn idealyst mcp design")?)
        .await
        .context("MCP handshake with idealyst mcp design")?;

    let info = client.peer_info().context("server info")?;
    let instructions = info.instructions.clone().unwrap_or_default();
    assert!(instructions.contains("list_design_skills"), "instructions: {instructions}");

    let tools: Vec<String> = client.list_all_tools().await?.into_iter().map(|t| t.name.to_string()).collect();
    for t in ["list_design_skills", "read_design_skill", "list_design_recipes", "get_design_recipe", "search_design"] {
        assert!(tools.iter().any(|n| n == t), "missing tool {t}: {tools:?}");
    }

    let recipe = client
        .call_tool(CallToolRequestParams::new("get_design_recipe").with_arguments(args(serde_json::json!({"name": "list_detail_adaptive"}))))
        .await?;
    assert!(text(&recipe).contains("pub fn app() -> Element"), "{}", text(&recipe));

    let section = client
        .call_tool(
            CallToolRequestParams::new("read_design_skill")
                .with_arguments(args(serde_json::json!({"name": "adaptive", "section": "patterns that change"}))),
        )
        .await?;
    assert!(text(&section).starts_with("## Patterns that change kind"), "{}", text(&section));

    let prompts: Vec<String> = client.list_all_prompts().await?.into_iter().map(|p| p.name).collect();
    assert_eq!(prompts.len(), 4, "{prompts:?}");
    let prompt = client
        .get_prompt(GetPromptRequestParams::new("design_screen").with_arguments(args(serde_json::json!({"screen": "team billing"}))))
        .await?;
    assert!(format!("{:?}", prompt.messages).contains("team billing"));

    let resources = client.list_all_resources().await?;
    assert!(resources.iter().any(|r| r.uri == "idea-ui-design://recipe/brand_module"));
    let read = client
        .read_resource(ReadResourceRequestParams::new("idea-ui-design://skill/principles"))
        .await?;
    assert!(format!("{:?}", read.contents).contains("House rules"));

    client.cancel().await?;
    Ok(())
}
