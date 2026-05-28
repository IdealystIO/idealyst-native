---
name: docs-accuracy
description: Documentation under examples/docs must accurately reflect the current framework, UI, and primitive surface. Accuracy only — not style, depth, or completeness.
targets:
  - examples/docs
severity: high
---

# Documentation accuracy

## Background

Docs live at `examples/docs/`:

- **Live docs pages** at `examples/docs/src/pages/*.rs` — Rust source that
  renders the documentation site. Prose is embedded as string literals,
  and code samples sit inside `CodeBlock(code = "...")` calls.
- **Content plans** at `examples/docs/docs-content-plan/*.md` — markdown
  source plans that run ahead of the live pages.

The documentation is **a user manual**. It is meant to be approachable
and short. This audit exists for one reason only: **to catch places where
what a doc tells the reader does not match how the framework actually
behaves.** A renamed primitive, a removed prop, a refactored macro, or
a wrong crate name can leave the docs technically wrong even though
they read well.

Two specific failure modes have already bitten us:

1. The workspace rule that crate names have **no `idealyst-` prefix**
   (auto-memory `feedback_no_idealyst_prefix`). Doc samples that say
   `idealyst-wire` or `idealyst-dev-client` are wrong on sight.
2. Behavioral claims like "every backend implements X" must be checked
   against the actual `Backend` trait and its impls — backends can
   diverge, and runtime-server in particular is a placeholder for some primitives
   (auto-memory `project_aas_graphics_unsupported`).

## Out of scope — do not flag

The agent must not produce findings about any of the following. They
are not bugs and "fixing" them would degrade the manual:

- Tone, voice, friendliness, or marketing-y phrasing.
- Whether a topic is explained in "enough depth" or could be expanded.
- Missing examples, missing diagrams, or missing edge-case coverage.
- Sections the agent thinks "should exist" but don't.
- Stylistic word choices ("use" vs. "leverage", buzzword density, etc.).
- Pages still being drafted or marked incomplete — drafts are expected.

If a doc is short and accurate, that is the desired state. Only
**inaccuracies** are findings.

## Checklist

Apply each item to **every page** under `examples/docs/src/pages/` and
**every plan** under `examples/docs/docs-content-plan/`. Flag findings
at the doc location, not the framework location.

- [ ] **Type / trait / enum-variant names** — for each capitalized
      identifier the docs claim is real (e.g. `Signal<T>`, `Element`,
      `Backend`, `BodyTone`, `HeadingKind`, `Effect`, `Scope`), grep
      the framework + UI crates to confirm it exists with the spelling
      the doc uses. Renames count as findings.
- [ ] **Function / macro names** — for every code-sample function call
      (`signal!`, `ui!`, `component!`, `Signal::new`, `count.update`,
      `count.get`, `on_cleanup`, etc.), confirm the symbol exists. Macro
      arms used in the docs (e.g. `signal!(value)`) must match a real
      macro arm in `framework/macros`.
- [ ] **Element prop names** — DSL examples like
      `Button(label = ..., on_click = ...)` or
      `Text(style = title_style)` must use the prop names the primitive
      / component actually accepts. Cross-reference against the
      `#[component]` definitions in `crates/ui/idea-ui` and the
      primitive constructors in `framework_core`.
- [ ] **Crate names in import lines and prose** — every `use foo::...`
      or "the `foo` crate" reference must match a real workspace member.
      **Flag any `idealyst-` prefix** as a high-severity finding (see
      auto-memory `feedback_no_idealyst_prefix`).
- [ ] **Feature-flag claims** — mentions of features like
      `--features robot` must match a real feature in the relevant
      `Cargo.toml`. Flag stale or invented feature names.
- [ ] **Behavioral universals** — statements of the form "every backend
      …", "the framework always …", "every primitive accepts …" must
      be checked against the `Backend` trait and its impls in
      `crates/backend/*`. Flag claims that hold for one backend but not
      all (e.g. runtime-server placeholder primitives, web-only escape hatches).
- [ ] **CLI command surface** — pages that document `idealyst <cmd>`
      (e.g. `build`, `dev`) must match the actual subcommands in
      `crates/cli/src/cmd/`. Flag removed commands (e.g. `link_patch`,
      `rebuild_patch` were recently deleted) and missing new ones.
- [ ] **File / module paths cited in prose** — when docs say "lives in
      `crates/foo/bar.rs`", confirm the path exists. Stale paths after
      refactors are common.
- [ ] **Cross-references between doc pages** — internal links like
      `[Styles](#)` or "see the Reactivity page" should resolve to a
      real page in `examples/docs/src/pages/mod.rs`'s registered routes.
      Flag placeholder `(#)` links that ship as the live target.
- [ ] **Plan vs. live contradictions** — when both a content plan
      (`docs-content-plan/NN-topic.md`) and a live page (`pages/topic.rs`)
      exist for the same topic and they make **factually contradictory**
      claims about the framework (different prop lists, opposite
      behavior claims), flag the one that disagrees with the code.
      Differences in wording, length, or which examples appear are
      **not** findings.
- [ ] **Code samples reference real API** — every `CodeBlock(code = "...")`
      should use imports, types, and function calls that exist in the
      current framework. Don't critique style or whether the example is
      "good"; only flag samples that name something the framework no
      longer has (or never had).

## Output format

Report findings as a Markdown list. **Every finding must be a concrete
inaccuracy** — a statement in the docs that disagrees with the code.
If your only complaint is about wording, depth, or style, drop it.

For each finding include:

- **Severity**: low / medium / high
  - high: wrong crate name with `idealyst-` prefix, removed/renamed API
    still recommended as the way to do something, universally-false
    behavioral claim.
  - medium: prop name drift, stale file path, broken cross-reference
    that ships in the live page.
  - low: factually wrong detail with low blast radius (e.g. stale path
    cited only in passing, plan-vs-live contradiction in a low-traffic
    plan).
- **Location**: `examples/docs/src/pages/file.rs:line` or
  `examples/docs/docs-content-plan/file.md:line`. Include the exact
  string from the doc as a short quote.
- **Issue**: one-line description of the inaccuracy.
- **Why**: cite the framework source of truth — the file/symbol that
  contradicts the doc (e.g. "no such variant in `runtime_core::Element`
  at `crates/runtime/core/src/lib.rs:NN`").
- **Suggested fix**: the corrected wording, symbol, or path — or
  "needs design discussion" if the doc reflects an intent that the
  framework hasn't caught up to.

End with a one-line summary: `Result: N high, M medium, K low findings.`
