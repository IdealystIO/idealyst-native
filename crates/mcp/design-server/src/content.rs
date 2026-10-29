//! The served content: design skills (`content/skills/*.md`), recipes
//! (`content/recipes/*.rs`) and the server instructions, parsed from the
//! files `build.rs` embeds.
//!
//! **Skill** — markdown with a `+++` front-matter block:
//!
//! ```text
//! +++
//! title = "Screens and navigation"
//! summary = "One line: what the skill decides."
//! when = "One line: the request that should reach for it."
//! order = 20
//! tags = ["navigation", "shell"]
//! +++
//! # Body…
//! ```
//!
//! Its `## ` headings are the sections `read_design_skill(section=…)` and
//! `search_design` address.
//!
//! **Recipe** — a whole app module. Its leading `//!` block is the header:
//! a `# Title` line, then `summary:`, `tags:` and any number of
//! `render: <path> [@phone|@tablet|@desktop] => <text>, !<text>` lines,
//! then free prose. The render
//! lines are the recipe's contract: `mcp-design-recipes-check` renders the
//! module's `app()` at each path and asserts every listed text is on the
//! page — and every `!`-prefixed one is not.

use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/content_generated.rs"));

/// The idea-ui version the recipes are compiled against
/// (`mcp-design-recipes-check` depends on the workspace idea-ui; a test there
/// fails when this falls out of step with it).
pub const CHECKED_AGAINST_IDEA_UI: &str = "13";

pub struct Skill {
    pub slug: &'static str,
    pub title: String,
    pub summary: String,
    pub when: String,
    pub order: u32,
    pub tags: Vec<String>,
    pub body: &'static str,
}

pub struct Section<'a> {
    /// The `## ` heading text; empty for the part above the first heading.
    pub heading: &'a str,
    /// The section, heading line included.
    pub text: &'a str,
}

impl Skill {
    pub fn sections(&self) -> Vec<Section<'static>> {
        split_sections(self.body)
    }
}

pub struct Render {
    pub path: String,
    /// The viewport the check renders at: `phone`, `tablet` or `desktop`
    /// (the default). Written `render: /path @phone => …`.
    pub viewport: Viewport,
    pub expect: Vec<String>,
}

/// The form factors a recipe is checked at. Every screen recipe declares at
/// least a phone and a desktop render — responsiveness is part of the
/// contract, not a follow-up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Viewport {
    Phone,
    Tablet,
    Desktop,
}

impl Viewport {
    /// Logical size in dp: an iPhone-class phone, an iPad-class tablet in
    /// portrait, a laptop.
    pub fn size(self) -> (f32, f32) {
        match self {
            Viewport::Phone => (390.0, 844.0),
            Viewport::Tablet => (820.0, 1180.0),
            Viewport::Desktop => (1280.0, 800.0),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Viewport::Phone => "phone",
            Viewport::Tablet => "tablet",
            Viewport::Desktop => "desktop",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "phone" => Some(Viewport::Phone),
            "tablet" => Some(Viewport::Tablet),
            "desktop" => Some(Viewport::Desktop),
            _ => None,
        }
    }
}

pub struct Recipe {
    pub slug: &'static str,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub renders: Vec<Render>,
    /// The header prose after the metadata lines, `//! ` stripped.
    pub notes: String,
    /// The whole file, as an app would paste it.
    pub source: &'static str,
}

pub fn instructions() -> &'static str {
    INSTRUCTIONS.trim()
}

pub fn skills() -> &'static [Skill] {
    static SKILLS_PARSED: OnceLock<Vec<Skill>> = OnceLock::new();
    SKILLS_PARSED.get_or_init(|| {
        let mut v: Vec<Skill> = SKILLS.iter().map(|(slug, raw)| parse_skill(slug, raw)).collect();
        v.sort_by(|a, b| a.order.cmp(&b.order).then(a.slug.cmp(b.slug)));
        v
    })
}

pub fn recipes() -> &'static [Recipe] {
    static RECIPES_PARSED: OnceLock<Vec<Recipe>> = OnceLock::new();
    RECIPES_PARSED.get_or_init(|| RECIPES.iter().map(|(slug, raw)| parse_recipe(slug, raw)).collect())
}

pub fn skill(slug: &str) -> Option<&'static Skill> {
    skills().iter().find(|s| s.slug == slug)
}

pub fn recipe(slug: &str) -> Option<&'static Recipe> {
    recipes().iter().find(|r| r.slug == slug)
}

pub(crate) fn parse_skill(slug: &'static str, raw: &'static str) -> Skill {
    let (front, body) = split_front_matter(raw);
    let get = |key: &str| front_value(front, key).unwrap_or_default();
    Skill {
        slug,
        title: front_value(front, "title").unwrap_or_else(|| slug.to_string()),
        summary: get("summary"),
        when: get("when"),
        order: get("order").parse().unwrap_or(999),
        tags: front_list(front, "tags"),
        body: body.trim_start_matches('\n'),
    }
}

pub(crate) fn parse_recipe(slug: &'static str, raw: &'static str) -> Recipe {
    let mut title = slug.to_string();
    let mut summary = String::new();
    let mut tags = Vec::new();
    let mut renders = Vec::new();
    let mut notes = Vec::new();
    for line in raw.lines() {
        let Some(rest) = line.strip_prefix("//!") else { break };
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        if let Some(t) = rest.strip_prefix("# ") {
            title = t.trim().to_string();
        } else if let Some(s) = rest.strip_prefix("summary:") {
            summary = s.trim().to_string();
        } else if let Some(t) = rest.strip_prefix("tags:") {
            tags = t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        } else if let Some(r) = rest.strip_prefix("render:") {
            let (target, expect) = r.split_once("=>").unwrap_or((r, ""));
            let (path, viewport) = match target.trim().split_once(" @") {
                Some((p, v)) => (
                    p,
                    Viewport::parse(v.trim())
                        .unwrap_or_else(|| panic!("recipe `{slug}`: unknown viewport `@{}`", v.trim())),
                ),
                None => (target.trim(), Viewport::Desktop),
            };
            renders.push(Render {
                path: path.trim().to_string(),
                viewport,
                expect: expect.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            });
        } else {
            notes.push(rest);
        }
    }
    Recipe {
        slug,
        title,
        summary,
        tags,
        renders,
        notes: notes.join("\n").trim().to_string(),
        source: raw,
    }
}

fn split_front_matter(raw: &str) -> (&str, &str) {
    let Some(rest) = raw.strip_prefix("+++") else { return ("", raw) };
    match rest.find("\n+++") {
        Some(end) => (&rest[..end], &rest[end + 4..]),
        None => ("", raw),
    }
}

fn front_line<'a>(front: &'a str, key: &str) -> Option<&'a str> {
    front.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim())
    })
}

fn front_value(front: &str, key: &str) -> Option<String> {
    front_line(front, key).map(|v| v.trim_matches('"').to_string())
}

fn front_list(front: &str, key: &str) -> Vec<String> {
    front_line(front, key)
        .map(|v| {
            v.trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|s| s.trim().trim_matches('"').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Split markdown on `## ` headings (outside fenced code).
pub(crate) fn split_sections(body: &str) -> Vec<Section<'_>> {
    let mut starts = vec![(0usize, "")];
    let mut in_fence = false;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence {
            if let Some(h) = line.strip_prefix("## ") {
                starts.push((offset, h.trim()));
            }
        }
        offset += line.len();
    }
    let mut out = Vec::new();
    for (i, &(start, heading)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map(|s| s.0).unwrap_or(body.len());
        let text = &body[start..end];
        if !text.trim().is_empty() {
            out.push(Section { heading, text });
        }
    }
    out
}

/// A `search_design` hit.
pub struct Hit {
    pub score: usize,
    /// `skill` or `recipe`.
    pub kind: &'static str,
    pub name: &'static str,
    /// The matching section's heading (skills) or the recipe title.
    pub label: String,
    pub snippet: String,
}

/// Rank skill sections and recipes against the query's words. Title and
/// tag matches weigh more than body matches; a hit must match every word
/// somewhere, so a narrower query returns fewer, better results.
pub fn search(query: &str, limit: usize) -> Vec<Hit> {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 1)
        .map(|w| w.to_lowercase())
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let score_text = |strong: &str, body: &str| -> Option<usize> {
        let (strong, body) = (strong.to_lowercase(), body.to_lowercase());
        let mut total = 0;
        for w in &words {
            let s = strong.matches(w.as_str()).count();
            let b = body.matches(w.as_str()).count();
            if s + b == 0 {
                return None;
            }
            total += s * 5 + b.min(5);
        }
        Some(total)
    };

    let mut hits = Vec::new();
    for sk in skills() {
        for sec in sk.sections() {
            let strong = format!("{} {} {}", sk.title, sec.heading, sk.tags.join(" "));
            if let Some(score) = score_text(&strong, sec.text) {
                hits.push(Hit {
                    score,
                    kind: "skill",
                    name: sk.slug,
                    label: if sec.heading.is_empty() { sk.title.clone() } else { sec.heading.to_string() },
                    snippet: snippet(sec.text, &words),
                });
            }
        }
    }
    for r in recipes() {
        let strong = format!("{} {} {}", r.title, r.summary, r.tags.join(" "));
        if let Some(score) = score_text(&strong, r.source) {
            hits.push(Hit {
                score: score + 2,
                kind: "recipe",
                name: r.slug,
                label: r.title.clone(),
                snippet: r.summary.clone(),
            });
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.cmp(b.name)));
    hits.truncate(limit);
    hits
}

/// The first non-heading line that contains a query word, trimmed to a
/// readable length.
fn snippet(text: &str, words: &[String]) -> String {
    let line = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .find(|l| {
            let l = l.to_lowercase();
            words.iter().any(|w| l.contains(w.as_str()))
        })
        .or_else(|| text.lines().find(|l| !l.starts_with('#') && !l.trim().is_empty()))
        .unwrap_or("")
        .trim();
    let mut s: String = line.chars().take(160).collect();
    if line.chars().count() > 160 {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_front_matter_parses() {
        let s = parse_skill(
            "x",
            "+++\ntitle = \"T\"\nsummary = \"S\"\nwhen = \"W\"\norder = 7\ntags = [\"a\", \"b\"]\n+++\n# Body\n\n## One\nx\n",
        );
        assert_eq!((s.title.as_str(), s.summary.as_str(), s.when.as_str(), s.order), ("T", "S", "W", 7));
        assert_eq!(s.tags, vec!["a", "b"]);
        let secs = s.sections();
        assert_eq!(secs.iter().map(|s| s.heading).collect::<Vec<_>>(), vec!["", "One"]);
    }

    #[test]
    fn headings_inside_fences_do_not_split() {
        let secs = split_sections("## A\n```md\n## not a heading\n```\n## B\n");
        assert_eq!(secs.iter().map(|s| s.heading).collect::<Vec<_>>(), vec!["A", "B"]);
    }

    #[test]
    fn recipe_header_parses() {
        let r = parse_recipe(
            "demo",
            "//! # Demo title\n//!\n//! summary: does a thing\n//! tags: a, b\n//! render: / => One, Two\n//! render: /x @phone => Three\n//!\n//! Prose here.\n\nuse x;\n",
        );
        assert_eq!(r.title, "Demo title");
        assert_eq!(r.summary, "does a thing");
        assert_eq!(r.tags, vec!["a", "b"]);
        assert_eq!(r.renders.len(), 2);
        assert_eq!((r.renders[0].viewport, r.renders[1].viewport), (Viewport::Desktop, Viewport::Phone));
        assert_eq!(r.renders[1].path, "/x");
        assert_eq!((r.renders[0].path.as_str(), r.renders[0].expect.clone()), ("/", vec!["One".to_string(), "Two".into()]));
        assert_eq!(r.notes, "Prose here.");
    }

    /// Every shipped skill and recipe carries the metadata the list tools
    /// print — a file missing it would list as a bare slug.
    #[test]
    fn shipped_content_is_complete() {
        assert!(!skills().is_empty() && !recipes().is_empty());
        for s in skills() {
            assert!(!s.summary.is_empty() && !s.when.is_empty(), "skill `{}` needs summary + when", s.slug);
            assert!(s.order != 999, "skill `{}` needs an order", s.slug);
            assert!(s.body.starts_with("# "), "skill `{}` body should open with its # title", s.slug);
        }
        for r in recipes() {
            assert!(r.title != r.slug && !r.summary.is_empty() && !r.tags.is_empty(), "recipe `{}` header", r.slug);
            assert!(!r.renders.is_empty(), "recipe `{}` declares no `render:` check", r.slug);
            // Responsive is part of the contract: every recipe is checked at
            // a phone width and a desktop width.
            for vp in [Viewport::Phone, Viewport::Desktop] {
                assert!(
                    r.renders.iter().any(|c| c.viewport == vp),
                    "recipe `{}` has no `@{}` render check",
                    r.slug,
                    vp.name()
                );
            }
            assert!(r.source.contains("pub fn app() -> Element"), "recipe `{}` must expose `pub fn app() -> Element`", r.slug);
        }
    }

    /// Cross-references inside the content (`recipe \`x\``, `skill \`y\``,
    /// `read_design_skill("z")`, `get_design_recipe("w")`) must name real
    /// entries — a renamed recipe otherwise leaves dangling pointers.
    #[test]
    fn content_cross_references_resolve() {
        let texts: Vec<(&str, &str)> = skills()
            .iter()
            .map(|s| (s.slug, s.body))
            .chain(recipes().iter().map(|r| (r.slug, r.source)))
            .chain(std::iter::once(("instructions", instructions())))
            .collect();
        for (owner, text) in texts {
            for (marker, is_recipe) in [
                ("get_design_recipe(\"", true),
                ("read_design_skill(\"", false),
                ("the `", true),
            ] {
                for piece in text.split(marker).skip(1) {
                    let name = piece.split(['"', '`']).next().unwrap_or("");
                    if marker == "the `" {
                        // "the `x` recipe" / "the `x` skill" only.
                        let after = &piece[name.len()..];
                        let after = after.trim_start_matches('`').trim_start();
                        if after.starts_with("recipe") {
                            assert!(recipe(name).is_some(), "{owner}: dangling recipe ref `{name}`");
                        } else if after.starts_with("skill") {
                            assert!(skill(name).is_some(), "{owner}: dangling skill ref `{name}`");
                        }
                        continue;
                    }
                    let found = if is_recipe { recipe(name).is_some() } else { skill(name).is_some() };
                    assert!(found, "{owner}: dangling ref `{marker}{name}\"`");
                }
            }
        }
    }

    #[test]
    fn search_requires_every_word_and_ranks_titles() {
        let hits = search("sidebar drawer", 5);
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|h| h.name == "app_shell_sidebar"));
        assert!(search("zzzqqq", 5).is_empty());
    }
}
