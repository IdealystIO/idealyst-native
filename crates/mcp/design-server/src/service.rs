//! The design MCP's handler: five read-only tools over the content, four
//! prompts that start a design task with the right skill already loaded,
//! and every skill and recipe as a resource.

use std::sync::Arc;

use rmcp::{
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::*,
    prompt, prompt_handler, prompt_router, schemars,
    service::RequestContext,
    tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler,
};
use serde::Deserialize;

use crate::content::{self, Recipe, Skill, CHECKED_AGAINST_IDEA_UI};

const SKILL_URI: &str = "idea-ui-design://skill/";
const RECIPE_URI: &str = "idea-ui-design://recipe/";

#[derive(Clone)]
pub struct DesignService {
    tool_router: ToolRouter<DesignService>,
    prompt_router: PromptRouter<DesignService>,
}

impl Default for DesignService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ReadSkillRequest {
    /// Skill name from list_design_skills.
    pub name: String,
    /// Only this `##` section (case-insensitive prefix match).
    pub section: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListRecipesRequest {
    /// Keep recipes carrying this tag.
    pub tag: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NameRequest {
    /// Recipe name from list_design_recipes.
    pub name: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchRequest {
    /// Words to find in skills and recipes.
    pub query: String,
    /// Max hits (default 8).
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ScreenPrompt {
    /// The screen to design, e.g. "project settings with members and billing".
    pub screen: String,
    /// What the app is and who uses it.
    pub app: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct BrandPrompt {
    /// The brand: colors, fonts, personality, references — whatever the user has.
    pub brand: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NavigationPrompt {
    /// The app and its main jobs, e.g. "field-crew timesheets: log hours, approve, report".
    pub app: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ReviewPrompt {
    /// Files, screens or a description of what to review.
    pub target: String,
}

fn ok(text: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(text.into())]))
}

fn not_found(kind: &str, name: &str, known: impl Iterator<Item = &'static str>) -> Result<CallToolResult, McpError> {
    let known: Vec<&str> = known.collect();
    Ok(CallToolResult::error(vec![Content::text(format!(
        "No {kind} named `{name}`. Known: {}.",
        known.join(", ")
    ))]))
}

fn skill_line(s: &Skill) -> String {
    format!("- {} — {}: {} Use when: {}", s.slug, s.title, s.summary, s.when)
}

fn recipe_line(r: &Recipe) -> String {
    format!("- {} — {} [{}]", r.slug, r.summary, r.tags.join(", "))
}

pub(crate) fn render_recipe(r: &Recipe) -> String {
    let checks: Vec<String> = r
        .renders
        .iter()
        .map(|c| format!("`{}` shows {}", c.path, c.expect.join(", ")))
        .collect();
    format!(
        "# {}\n\n{}\n\nCompiled and server-rendered against idea-ui {} ({}). \
         The file is a whole module: `pub fn app() -> Element` is the demo entry — \
         in an app, keep the screens/components and wire them into your own `app()`.\n\n\
         ```rust\n{}```\n",
        r.title,
        r.notes,
        CHECKED_AGAINST_IDEA_UI,
        checks.join("; "),
        r.source
    )
}

pub(crate) fn render_skill(s: &Skill, section: Option<&str>) -> Option<String> {
    let Some(want) = section else { return Some(s.body.to_string()) };
    let want = want.trim().to_lowercase();
    s.sections()
        .into_iter()
        .find(|sec| !sec.heading.is_empty() && sec.heading.to_lowercase().starts_with(&want))
        .map(|sec| sec.text.to_string())
}

#[tool_router]
impl DesignService {
    pub fn new() -> Self {
        Self { tool_router: Self::advertised_router(), prompt_router: Self::prompt_router() }
    }

    /// The router with every input schema minified the way `mcp-server`'s
    /// are, so both servers cost the client the same per parameter.
    fn advertised_router() -> ToolRouter<Self> {
        let mut router = Self::tool_router();
        for route in router.map.values_mut() {
            mcp_server::schema_minify::minify(Arc::make_mut(&mut route.attr.input_schema));
        }
        router
    }

    #[tool(description = "List the idea-ui design skills: what each decides and when to use it. Start here.")]
    async fn list_design_skills(&self) -> Result<CallToolResult, McpError> {
        let lines: Vec<String> = content::skills().iter().map(skill_line).collect();
        ok(format!(
            "{}\n\nRead one with read_design_skill(name). Start with `principles` if you haven't.",
            lines.join("\n")
        ))
    }

    #[tool(description = "Read a design skill (rules, workflow, idea-ui wiring), or one `##` section of it.")]
    async fn read_design_skill(&self, Parameters(req): Parameters<ReadSkillRequest>) -> Result<CallToolResult, McpError> {
        let Some(s) = content::skill(&req.name) else {
            return not_found("skill", &req.name, content::skills().iter().map(|s| s.slug));
        };
        match render_skill(s, req.section.as_deref()) {
            Some(text) => ok(text),
            None => {
                let heads: Vec<&str> = s.sections().iter().map(|x| x.heading).filter(|h| !h.is_empty()).collect();
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "`{}` has no section starting `{}`. Sections: {}.",
                    s.slug,
                    req.section.unwrap_or_default(),
                    heads.join(" | ")
                ))]))
            }
        }
    }

    #[tool(description = "List whole-screen idea-ui recipes (shells, pages, forms, states, brand), optionally by tag.")]
    async fn list_design_recipes(&self, Parameters(req): Parameters<ListRecipesRequest>) -> Result<CallToolResult, McpError> {
        let tag = req.tag.map(|t| t.to_lowercase());
        let lines: Vec<String> = content::recipes()
            .iter()
            .filter(|r| tag.as_ref().is_none_or(|t| r.tags.iter().any(|x| x.to_lowercase() == *t)))
            .map(recipe_line)
            .collect();
        if lines.is_empty() {
            let mut tags: Vec<&str> = content::recipes().iter().flat_map(|r| r.tags.iter().map(String::as_str)).collect();
            tags.sort();
            tags.dedup();
            return ok(format!("No recipe has that tag. Tags: {}.", tags.join(", ")));
        }
        ok(format!("{}\n\nget_design_recipe(name) returns the full compiled source.", lines.join("\n")))
    }

    #[tool(description = "Get a recipe: rationale plus a complete module compiled against idea-ui.")]
    async fn get_design_recipe(&self, Parameters(req): Parameters<NameRequest>) -> Result<CallToolResult, McpError> {
        match content::recipe(&req.name) {
            Some(r) => ok(render_recipe(r)),
            None => not_found("recipe", &req.name, content::recipes().iter().map(|r| r.slug)),
        }
    }

    #[tool(description = "Search skills and recipes by words; returns ranked sections to read next.")]
    async fn search_design(&self, Parameters(req): Parameters<SearchRequest>) -> Result<CallToolResult, McpError> {
        let hits = content::search(&req.query, req.limit.unwrap_or(8).clamp(1, 25));
        if hits.is_empty() {
            return ok("No matches. Try fewer words, or list_design_skills.");
        }
        let lines: Vec<String> = hits
            .iter()
            .map(|h| match h.kind {
                "skill" => format!("- skill {} § {} — {}", h.name, h.label, h.snippet),
                _ => format!("- recipe {} — {}", h.name, h.snippet),
            })
            .collect();
        ok(format!(
            "{}\n\nOpen a skill section with read_design_skill(name, section), a recipe with get_design_recipe(name).",
            lines.join("\n")
        ))
    }
}

/// A prompt that opens with the named skill's full text and then states
/// the task — so the client starts the conversation with the rules in
/// context instead of having to discover them.
fn skill_prompt(skill: &str, task: String) -> Vec<PromptMessage> {
    let body = content::skill(skill).map(|s| s.body).unwrap_or("");
    let principles = content::skill("principles").map(|s| s.body).unwrap_or("");
    vec![PromptMessage::new_text(
        PromptMessageRole::User,
        format!(
            "You are designing with idea-ui. These are the house rules — follow them.\n\n{principles}\n\n---\n\n{body}\n\n---\n\n{task}"
        ),
    )]
}

#[prompt_router]
impl DesignService {
    #[prompt(name = "design_screen", description = "Design and build one idea-ui screen the house way.")]
    async fn design_screen(&self, Parameters(p): Parameters<ScreenPrompt>) -> Vec<PromptMessage> {
        let app = p.app.map(|a| format!("\nThe app: {a}")).unwrap_or_default();
        skill_prompt(
            "screens",
            format!(
                "Task: design and implement this screen: {}.{app}\n\n\
                 1. Name the screen's one primary job and its primary action.\n\
                 2. Pick the archetype from the skill above and fetch its recipe (get_design_recipe).\n\
                 3. Sketch the hierarchy (header, sections, actions, states) in a short list before writing code.\n\
                 4. Build it with idea-ui components and theme tokens only — no literal colors or pixel spacing.\n\
                 5. Cover loading, empty and error states (read_design_skill(\"states\")).\n\
                 6. Finish with the checklist in read_design_skill(\"review\").",
                p.screen
            ),
        )
    }

    #[prompt(name = "brand_theme", description = "Turn the user's brand into an idea-ui theme + restyle module.")]
    async fn brand_theme(&self, Parameters(p): Parameters<BrandPrompt>) -> Vec<PromptMessage> {
        skill_prompt(
            "brand",
            format!(
                "Task: build this app's brand module from what the user gave: {}\n\n\
                 Use only what the brand specifies; where it is silent, keep idea-ui's default \
                 and list the open questions for the user at the end. Follow the `brand_module` recipe \
                 (get_design_recipe(\"brand_module\")): theme tokens first, restyles second, \
                 per-instance overrides never.",
                p.brand
            ),
        )
    }

    #[prompt(name = "plan_navigation", description = "Choose the shell and navigator structure for an idea-ui app.")]
    async fn plan_navigation(&self, Parameters(p): Parameters<NavigationPrompt>) -> Vec<PromptMessage> {
        skill_prompt(
            "navigation",
            format!(
                "Task: plan the navigation for: {}\n\n\
                 Produce (a) the section list in priority order, (b) the shell choice with its reason, \
                 (c) the navigator tree with every route name and path, (d) which screens push detail \
                 views, then implement the shell from the matching recipe.",
                p.app
            ),
        )
    }

    #[prompt(name = "design_review", description = "Review idea-ui screens against the house design rules.")]
    async fn design_review(&self, Parameters(p): Parameters<ReviewPrompt>) -> Vec<PromptMessage> {
        skill_prompt(
            "review",
            format!(
                "Task: review {} against the checklist above. Report each finding as \
                 `file:line — rule — what to change`, most severe first, then fix the ones the user approves.",
                p.target
            ),
        )
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for DesignService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder().enable_tools().enable_prompts().enable_resources().build(),
        )
        .with_server_info(Implementation::from_build_env())
        .with_instructions(content::instructions().to_string())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut resources: Vec<Resource> = content::skills()
            .iter()
            .map(|s| {
                let mut r = RawResource::new(format!("{SKILL_URI}{}", s.slug), s.slug.to_string());
                r.description = Some(s.summary.clone());
                r.mime_type = Some("text/markdown".into());
                r.no_annotation()
            })
            .collect();
        resources.extend(content::recipes().iter().map(|rc| {
            let mut r = RawResource::new(format!("{RECIPE_URI}{}", rc.slug), rc.slug.to_string());
            r.description = Some(rc.summary.clone());
            r.mime_type = Some("text/markdown".into());
            r.no_annotation()
        }));
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();
        let text = if let Some(slug) = uri.strip_prefix(SKILL_URI) {
            content::skill(slug).map(|s| s.body.to_string())
        } else if let Some(slug) = uri.strip_prefix(RECIPE_URI) {
            content::recipe(slug).map(render_recipe)
        } else {
            None
        };
        match text {
            Some(t) => Ok(ReadResourceResult::new(vec![ResourceContents::text(t, uri.to_string())])),
            None => Err(McpError::resource_not_found(format!("no resource {uri}"), None)),
        }
    }
}

#[cfg(test)]
mod tests {
    //! The tool list is resent to the model for the whole session, so its
    //! size is a contract — the same limits `mcp-server` holds itself to.

    use super::*;

    const SURFACE_BUDGET_CHARS: usize = 3_000;
    const DESCRIPTION_MAX_CHARS: usize = 120;
    const PARAM_DOC_MAX_CHARS: usize = 100;
    /// Instructions are injected into every session that connects, so they
    /// carry only the rules that change the first move, not the skills.
    const INSTRUCTIONS_MAX_CHARS: usize = 1_600;

    fn advertised(svc: &DesignService) -> Vec<Tool> {
        DesignService::tool_router()
            .list_all()
            .iter()
            .map(|t| ServerHandler::get_tool(svc, &t.name).expect("served"))
            .collect()
    }

    fn text(r: &CallToolResult) -> String {
        r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect()
    }

    #[test]
    fn tool_surface_stays_within_budget() {
        let tools = advertised(&DesignService::new());
        let total: usize = tools.iter().map(|t| serde_json::to_string(t).unwrap().len()).sum();
        assert!(total <= SURFACE_BUDGET_CHARS, "tool surface is {total} chars (budget {SURFACE_BUDGET_CHARS})");
        for t in &tools {
            let d = t.description.as_deref().unwrap_or("");
            assert!(!d.is_empty() && d.len() <= DESCRIPTION_MAX_CHARS, "`{}` description: {} chars", t.name, d.len());
            let schema = serde_json::Value::Object((*t.input_schema).clone());
            assert!(schema.get("$schema").is_none() && schema.get("title").is_none(), "`{}` schema not minified", t.name);
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                for (k, v) in props {
                    let pd = v.get("description").and_then(|d| d.as_str()).unwrap_or("");
                    assert!(!pd.is_empty() && pd.len() <= PARAM_DOC_MAX_CHARS, "`{}.{k}` param doc: {} chars", t.name, pd.len());
                }
            }
        }
    }

    #[test]
    fn instructions_are_short_and_point_at_the_tools() {
        let i = content::instructions();
        assert!(i.len() <= INSTRUCTIONS_MAX_CHARS, "instructions are {} chars", i.len());
        assert!(i.contains("list_design_skills") && i.contains("get_design_recipe"), "{i}");
    }

    #[tokio::test]
    async fn every_skill_and_recipe_is_readable_through_the_tools() {
        let svc = DesignService::new();
        for s in content::skills() {
            let r = svc
                .read_design_skill(Parameters(ReadSkillRequest { name: s.slug.into(), section: None }))
                .await
                .unwrap();
            assert_ne!(r.is_error, Some(true));
            assert!(text(&r).starts_with("# "));
        }
        for rc in content::recipes() {
            let r = svc.get_design_recipe(Parameters(NameRequest { name: rc.slug.into() })).await.unwrap();
            assert!(text(&r).contains("pub fn app() -> Element"));
        }
    }

    #[tokio::test]
    async fn unknown_names_list_the_known_ones() {
        let svc = DesignService::new();
        let r = svc.get_design_recipe(Parameters(NameRequest { name: "nope".into() })).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("app_shell_sidebar"));
        let r = svc
            .read_design_skill(Parameters(ReadSkillRequest { name: "principles".into(), section: Some("zzz".into()) }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("Sections:"));
    }

    #[tokio::test]
    async fn section_lookup_is_a_case_insensitive_prefix() {
        let svc = DesignService::new();
        let s = &content::skills()[0];
        let heading = s.sections().into_iter().find(|x| !x.heading.is_empty()).expect("a section").heading;
        let prefix: String = heading.chars().take(4).collect::<String>().to_uppercase();
        let r = svc
            .read_design_skill(Parameters(ReadSkillRequest { name: s.slug.into(), section: Some(prefix) }))
            .await
            .unwrap();
        assert!(text(&r).starts_with(&format!("## {heading}")), "{}", text(&r));
    }

    #[tokio::test]
    async fn tag_filter_narrows_and_misses_list_tags() {
        let svc = DesignService::new();
        let all = text(&svc.list_design_recipes(Parameters(ListRecipesRequest { tag: None })).await.unwrap());
        let nav = text(&svc.list_design_recipes(Parameters(ListRecipesRequest { tag: Some("Navigation".into()) })).await.unwrap());
        assert!(nav.lines().count() < all.lines().count(), "{nav}");
        assert!(nav.contains("app_shell_sidebar"));
        let miss = text(&svc.list_design_recipes(Parameters(ListRecipesRequest { tag: Some("zzz".into()) })).await.unwrap());
        assert!(miss.starts_with("No recipe has that tag. Tags:"), "{miss}");
    }

    #[tokio::test]
    async fn prompts_carry_the_principles_and_their_skill() {
        let svc = DesignService::new();
        let msgs = svc
            .design_screen(Parameters(ScreenPrompt { screen: "team settings".into(), app: None }))
            .await;
        let PromptMessageContent::Text { text } = &msgs[0].content else { panic!("text prompt") };
        assert!(text.contains(content::skill("principles").unwrap().title.as_str()));
        assert!(text.contains(content::skill("screens").unwrap().title.as_str()));
        assert!(text.contains("team settings"));
        assert_eq!(DesignService::prompt_router().list_all().len(), 4);
    }
}
