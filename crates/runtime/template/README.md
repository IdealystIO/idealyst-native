# runtime-template — a `ui!` site, as data

`ui!` has two lowerings:

- **direct** — what `ui!` emits: builder calls inline at the call site.
- **template** — the same parsed tree split into a `Descriptor` (data)
  plus an ordered list of dynamic expressions ("slots"), with
  `runtime_vocabulary::template::build` constructing the `Element` from
  the pair.

This crate owns the descriptor. It depends on `runtime-scene` and
`serde` and nothing else — no builders, no handlers, no dev-server, no
CLI.

```
ui_lowered!(template { view(style = sheet()) { text { "hello" } } })

  ↓ runtime_macros::ui_split      (shared with the direct lowering)

  static  : view[style=slot 0] > text[content="hello"]
  dynamic : slot 0 = sheet()

  ↓ runtime_macros::ui_template

  {
      let __ui_s0; __ui_s0 = sheet();
      static __UI_DESC: Descriptor = …;
      runtime_core::__template::build(&__UI_DESC, &mut [
          SlotValue::style(__ui_s0),
      ])
  }
```

## Why the split is worth having

Most of a UI tree is not code. Tags, attribute names, child order,
string and number and bool literals, enum-like paths, style-token
accessors — all of it is data that happens to be spelled in Rust. The
direct lowering compiles it into machine code, so changing a label means
a rebuild.

A descriptor separates the two halves, which buys three things. Only the
first is being built:

1. **Hot reload of static edits** — a changed literal or a reordered
   child is a new descriptor for the same slot signature, which a
   running program can swap in. That is what [`validate`] checks.
2. **Possibly, over-the-air UI patches** — the same swap, from a
   descriptor received at runtime rather than from a recompile. The
   [`TemplateSource`] trait is the seam; **the path is not built**, and
   `CompiledIn` is the only implementation.
3. **Possibly, smaller output** — one builder-driving function instead
   of N inlined builder chains. Unmeasured.

Nothing about (2) or (3) is assumed by anything here. What keeps them
reachable is the dependency list: a descriptor that only needs
`runtime-scene` and `serde` can be serialized, shipped, and validated
without a renderer in the graph.

## The model

| type | what it is |
|------|------------|
| `Descriptor` | one `ui!` site: a `SiteId`, a `SlotSig`, a FLAT `nodes` array and the `roots` into it |
| `Node` | `Prim` (a builtin the builder constructs), `Component` (a `#[component]` tag + a site-supplied constructor), `Dyn` (a reactive `if`), `Escape` (a subtree the site built itself) |
| `PropEntry` | `name = Lit(LiteralValue) \| Slot(index)` |
| `SlotSig` | one `SlotInfo` per slot: its role, its syntactic kind, and its prop name |
| `SiteId` | `module_path!()` + a digest of the site's body tokens |
| `Registry` | the descriptors a program knows about, keyed by site |
| `Patch` | a replacement descriptor for one site |
| `TemplateSource` | where a site's descriptor comes from; `CompiledIn` is the only one |

Everything is `Cow`-backed, so the compiled-in form is a plain `static`
and the deserialized form is the same type:

```rust
static DESC: Descriptor = Descriptor {
    site: SiteId { module: Cow::Borrowed("app::screen"), hash: Cow::Borrowed("0a1b2c3d") },
    slots: SlotSig { slots: Cow::Borrowed(&[]) },
    nodes: Cow::Borrowed(&[Node::Prim {
        kind: PrimKind::Text,
        props: Cow::Borrowed(&[PropEntry {
            name: Cow::Borrowed("content"),
            value: PropValue::Lit(LiteralValue::Str(Cow::Borrowed("hello"))),
        }]),
        children: Cow::Borrowed(&[]),
    }]),
    roots: Cow::Borrowed(&[0]),
};
```

`nodes` is flat and children are `u32` indices. Nesting `Node` inside
`Node` would need a `Box` per level, which is not const-constructible —
and a flat array lets a nested template (an `if` branch's body) live in
the same descriptor as its parent, addressed by root index.

## What `validate` actually checks

The slots are **code**. They were compiled into the binary from the
author's expressions and cannot be patched; only the descriptor can. So
a patch is accepted only when

- it is internally consistent — every child/root index in range, every
  slot reference declared;
- and its slot signature matches the compiled site's **shape for
  shape**. A descriptor that used slot 3 as a condition where the binary
  supplies a text value would hand the builder the wrong type.

`SlotInfo::name` is deliberately NOT compared: it is reserved for a
later name-matched protocol, and comparing it now would reject harmless
descriptor edits.

`kind` is a *syntactic* label (`"closure"`, `"path"`, `"call"`, …), not
a Rust type — a proc macro has tokens, never resolved types. It is a
drift detector, not a type check.

## What is descriptor-native, and what escapes

`Node::Escape` is the completeness escape hatch: the site built the
subtree with the direct lowering and left the finished `Element`(s) in a
slot. Every `ui!` construct is expressible that way, which is what lets
the template lowering be *complete* while the descriptor-native set
grows independently. An escape is correct but opaque — its literals are
compiled in, so a static edit inside one still needs a rebuild.

Descriptor-native today (`PrimKind`, plus `Component` and `Dyn`):

- `view`, `text`, `button`, `image`, `activity_indicator`,
  `scroll_view`, with the props
  `runtime_vocabulary::template::build_prim` models — `style`,
  `content`/`label`, `on_click`, `disabled`, `src`, `alt`, `test_id`,
  `a11y_label`, `a11y_hint`, `a11y_hidden`, `horizontal`, `bounces`;
- a `#[component]` whose props are all descriptor literals, plus its
  children;
- a reactive `if`, as `Dyn` — condition and branch thunks in slots, each
  branch its own nested template.

Escaped today: every other primitive (`icon`, `text_input`, `toggle`,
`slider`, `link`, `overlay`, `anchored_overlay`, `presence`,
`flat_list`, `graphics`, `when`), a component with any dynamic prop, any
node with a trailing `.method(…)` chain, `for`, `match`, static `if`,
`if let`, and bare expression children. An escaped node's *bodies* are
still nested templates, so the lowering does not stop at the first
escape.

Widening the native set is additive: a `PrimKind` variant plus its arm
in the builder plus its entry in the macro's table. The macro's unit
tests pin the current boundary node kind by node kind, so a widening is
visible in the diff.

## Tests

`cargo test -p runtime-template` covers serde round-tripping,
registration, and every `validate` rejection. The emission's own
descriptors are checked by a `debug_assert!` in
`runtime_vocabulary::template::build`, so the whole
`ui-lowering-parity` suite (and every debug-built app under the template
lowering) runs `check_well_formed` against real output rather than only
hand-written fixtures.

The proof that a descriptor builds the *right* tree lives in
`crates/dev/ui-lowering-parity`: every fixture is authored once, expanded
through both lowerings, and compared on three projections of a real
mount.
