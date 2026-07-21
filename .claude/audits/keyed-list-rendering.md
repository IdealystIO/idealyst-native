---
name: keyed-list-rendering
description: Dynamic child lists must be rendered through the macro's keyed `for … , key = …`, never assembled as a `Vec<Element>` in plain Rust and splatted.
targets:
  - crates/ui/idea-ui
  - crates/ui/idea-ui-nav
  - crates/ui/idea-ui-mail
  - websites/website
  - websites/idea-ui-docs
  - websites/docs
severity: high
---

# Keyed list rendering

## Background

The framework enforces reconciliation keys **only** at the `for`-loop
lowering in `ui!`/`jsx!`. A reactive `for row in signal { … }` without a
`, key = …` is a hard compile error (the `ReactiveListKeyed` diagnostic in
`crates/runtime/core/src/builder.rs`), and a keyed `for` lowers to
`Element::Each`, the one primitive that carries per-row keys and reconciles
by identity.

Every other child is reconciled **positionally, by slot index**
(`crates/runtime/core/src/walker/view.rs` — a row's `Identity::node` is its
iteration index). So the moment you build a `Vec<Element>` in plain Rust and
splat it as children, the key information is *already erased* before the
macro ever sees it. If that list is dynamic — its source data reorders,
grows, or shrinks across renders — the elements re-diff against the same
positional slots and per-row state (component-local signals, text-input
focus, scroll position) silently attaches to the **wrong** row. This is the
classic "index as key" bug, and it is unreachable by the compile-time key
enforcement because it never went through `for`.

This bit us with a real call site of the shape:

```rust
// WRONG — keys erased, positional reconciliation, state misattribution
page.rows.iter().map(|c| gallery_card(c.clone(), …)).collect()
```

`CLAUDE.md` §9.3 / §9.4 already forbid this stylistically ("Do not assemble
a `Vec<Element>` outside the macro and splat it in just to populate a
parent" / conditional-and-iterative rendering belongs inside the macro).
This audit gives that rule teeth: it finds the out-of-macro list
construction so it can be rewritten as `for item in …, key = item.id { … }`
inside the macro, where the keyed path is enforced.

## The distinction that matters

There is exactly **one** legitimate `Vec<Element>` shape, and the audit must
not flag it: a container component that **receives** `children: Vec<Element>`
as a prop and flattens incoming fragments via `ChildList::append_to` before
splatting the bare `children` identifier. Canonical examples:
`crates/ui/idea-ui/src/components/center.rs` and `card.rs`:

```rust
// LEGIT — flattening a RECEIVED children param, not authoring new rows
let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
for c in props.children {
    ChildList::append_to(c, &mut children);
}
ui! { view(style = style) { children } }
```

The discriminator: **is the loop iterating a `children`/fragment parameter it
was handed, or is it iterating source DATA and building NEW elements** (a
`#[component]` call, a builder, `ui!{…}` per item) in the map/push body? The
former is flattening and is fine. The latter is authoring rows outside the
macro and is the bug.

## Checklist

For each targeted crate, inspect author-facing render code (component
bodies, page/section helpers, shell/layout helpers):

- [ ] **`.map(...).collect()` into children** — an iterator adaptor chain that
      maps items to `Element` (via a `#[component]` call, a builder, or
      `ui!{…}`/`jsx!{…}` in the closure) and `.collect()`s a `Vec<Element>`
      that is then splatted as children or passed as a `children:` prop.
      Rough grep: `\.map(` … `\.collect()` near an `Element`/`ui!`/`jsx!` /
      component-name closure body. FLAG it.
- [ ] **`Vec::new()`/`Vec::with_capacity(...)` + `.push(ui!{…})` /
      `.push(jsx!{…})` loops** that build sibling rows, then splat the vec.
      Rough grep: `let mut .* = Vec::` followed by `.push(ui!` / `.push(jsx!`
      / `children.push(`. FLAG it — this is §9.3's forbidden shape.
- [ ] **Conditional push** — `if cond { children.push(…) }` / `if let Some(x)
      = … { v.push(ui!{…}) }` building a child vec out of macro. FLAG it
      (§9.4 says use `if` *inside* `ui!`).
- [ ] **`extend()` of built elements** into a child vec in a loop — same
      family as push. FLAG it.
- [ ] For each flagged site, determine whether the source list is **dynamic**
      (derived from a signal, a prop that changes, paged/filtered data) or
      **provably static** (a fixed literal array, a `const`, one-shot page
      construction that never re-renders). Dynamic → **high**; static-but-
      out-of-macro → **medium** (still a §9.3 style violation and a latent
      trap if the source later becomes dynamic).
- [ ] **Do NOT flag** the received-children flatten pattern: a loop over a
      `children` / `Vec<Element>` *parameter* that calls
      `ChildList::append_to` (or pushes the received elements unchanged),
      then splats the bare `children` identifier. This is the one canonical
      `Vec<Element>` shape (`center.rs` / `card.rs`).
- [ ] **Do NOT flag** `.collect()` that does not produce `Vec<Element>`
      (collecting strings, tuples, style rules, non-UI data).

## How to fix a finding

Rewrite the out-of-macro construction as macro-internal iteration:

```rust
// before
ui! { view { { page.rows.iter().map(|c| gallery_card(c.clone())).collect() } } }

// after — keyed when the list is dynamic
ui! {
    view {
        for c in page.rows.clone(), key = c.id {
            { gallery_card(c.clone()) }
        }
    }
}
```

If the list is reactive, iterate the **signal itself** (`for c in rows_signal,
key = c.id`) so it lowers to `Element::Each` — do not `.get()` it into a
static `Vec` first, which re-enters the static positional path.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: high (dynamic list) / medium (static, out-of-macro) / low
- **Location**: `crate/src/file.rs:line`
- **Issue**: one-line description of the out-of-macro list construction
- **Why**: whether the source is dynamic (state-misattribution risk) or a
  §9.3 style violation, and which slot the erased key would have protected.
- **Suggested fix**: the `for … , key = …` rewrite, or "needs design
  discussion" if the key source is unclear.

End with a one-line summary: `Result: N high, M medium, K low findings.`
