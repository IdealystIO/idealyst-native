//! Svelte SFC → porter IR.
//!
//! Pipeline:
//!
//! 1. `sfc::split` extracts `<script>` and `<style>`; the rest is markup.
//! 2. The script block is parsed with `port_tsx::parse(..., tsx=false)`
//!    and walked by `script::walk` — collects `export let` (props),
//!    plain `let` (reactive state via signal!), `$:` statements
//!    (derived / effect), and `function` decls for handler refs.
//! 3. The markup is walked by `markup::parse` with the script's
//!    `handler_fns` map threaded in, so `on:click={increment}`
//!    can substitute the captured function body inline.
//! 4. `<style>` becomes an `Unsupported` hole — CSS porting is a
//!    separate concern.

use port_core::ir::*;
use port_core::{ParseError, Parser};

use crate::{markup, script, sfc};

pub struct SvelteParser;

impl SvelteParser {
    pub fn new() -> Self { Self }
}

impl Default for SvelteParser {
    fn default() -> Self { Self::new() }
}

impl Parser for SvelteParser {
    fn parse(&self, source: &str) -> Result<(Module, PortReport), ParseError> {
        let mut report = PortReport::default();
        let blocks = sfc::split(source);

        let script_src = blocks.script.ok_or_else(|| {
            ParseError::new("Svelte file: no <script> block")
        })?;

        let (script_ast, _cm) = port_tsx::parse(script_src, false)?;
        let script_result = script::walk(&script_ast, &mut report);

        // Markup → JsxNode tree.
        let template_root = markup::parse(&blocks.markup, &script_result.handler_fns);

        if let Some(_style) = &blocks.style {
            report.record(Hole {
                kind: HoleKind::Unsupported,
                reason: "<style> block — pending port-css pass".into(),
                original: SourceSnippet::new(""),
            });
        }

        let component = Component {
            name: "Counter".into(),
            props: script_result.props,
            body: ComponentBody {
                preamble: script_result.preamble,
                returns: ReturnExpr::Jsx(template_root),
            },
        };

        let module = Module {
            source_tool: "port-svelte".into(),
            imports: vec![
                "component".into(),
                "jsx".into(),
                "signal".into(),
                "effect".into(),
                "on_cleanup".into(),
                "provide".into(),
                "inject".into(),
                "Primitive".into(),
                "Signal".into(),
            ],
            components: vec![component],
            passthroughs: vec![],
            local_interfaces: script_result.local_interfaces,
            unresolved_context_aliases: vec![],
        };

        report.component_count = module.components.len();
        Ok((module, report))
    }
}

pub type StubParser = SvelteParser;
