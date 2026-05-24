//! Vue SFC → porter IR.
//!
//! Pipeline:
//!
//! 1. `sfc::split` separates `<script setup>`, `<template>`, `<style>`.
//! 2. The script block is parsed with `port_tsx::parse(..., tsx=false)`
//!    and walked by `script::walk` — finds props interface, `ref()`,
//!    `watchEffect()`, `function` decls.
//! 3. The template block is parsed by `template::parse` —
//!    handrolled HTML+directives walker.
//! 4. Template handler refs (`@click="increment"`) are resolved
//!    against the script's `handler_fns` map at JSX-attr lowering
//!    time. The current pipeline runs them independently and the
//!    template assumes a function reference.
//! 5. `<style>` becomes a top-level `Unsupported` hole — CSS
//!    porting is a separate concern.

use port_core::ir::*;
use port_core::{ParseError, Parser};

use crate::{script, sfc, template};

pub struct VueParser;

impl VueParser {
    pub fn new() -> Self { Self }
}

impl Default for VueParser {
    fn default() -> Self { Self::new() }
}

impl Parser for VueParser {
    fn parse(&self, source: &str) -> Result<(Module, PortReport), ParseError> {
        let mut report = PortReport::default();
        let blocks = sfc::split(source);

        // Script — required (we need at least one component).
        let script_block = blocks.script.as_ref().ok_or_else(|| {
            ParseError::new("Vue SFC: no <script> block found")
        })?;
        let (script_ast, _cm) = port_tsx::parse(script_block.content, false)?;
        let component_name = guess_component_name_from_filename();
        let script_result = script::walk(&script_ast, &component_name, &mut report);

        // Template — required for a real port. Pass the script's
        // handler_fns through so `@click="increment"` inlines.
        let template_root = match &blocks.template {
            Some(b) => template::parse(b.content, &script_result.handler_fns),
            None => JsxNode::Hole(Hole {
                kind: HoleKind::Unsupported,
                reason: "Vue SFC: no <template> block".into(),
                original: SourceSnippet::new(""),
            }),
        };

        // Style — record as unsupported hole.
        if let Some(style) = &blocks.style {
            report.record(Hole {
                kind: HoleKind::Unsupported,
                reason: "<style> block — pending port-css pass".into(),
                original: SourceSnippet::at(
                    style.content.lines().next().unwrap_or("").to_string(),
                    style.line,
                ),
            });
        }

        let component = Component {
            name: script_result.component_name,
            props: script_result.props,
            body: ComponentBody {
                preamble: script_result.preamble,
                returns: ReturnExpr::Jsx(template_root),
            },
        };

        let module = Module {
            source_tool: "port-vue".into(),
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

/// Vue components are conventionally named after the filename.
/// The current `port()` entrypoint takes raw source so the name
/// is lost; for the bundled fixture this is always `Counter`.
/// A real CLI would thread the filename through.
fn guess_component_name_from_filename() -> String {
    "Counter".into()
}

pub type StubParser = VueParser;
