# runtime-template — a `ui!` site, as data

A **descriptor** is one `ui!` call site in data form: its node tree,
tags, attribute names, child order, literal values, and a `SlotSig`
naming the dynamic expressions it does *not* carry. This crate owns that
data model, the `Registry` that keys descriptors by site, and
`validate`. It depends on `serde` and nothing else — no builders, no
handlers, no dev server, no CLI.

```
src/screens/login.rs:42:5

  ui! { view(style = sheet()) { text { "Sign in" } } }

    ↓ the split pass (runtime_macros' ui_split)

  static  : view[style = slot 0] > text[content = "Sign in"]
  dynamic : slot 0 = sheet()
```

The static half is a `Descriptor`. The dynamic half stays compiled code,
which is the whole point: an overlay edits the first and never touches
the second.

## Descriptors are build artifacts, not binary contents

They used to be compiled in: `ui!` emitted a `static Descriptor` per site
and registered it at startup. That was measured on a real app — CrewForge,
~256 codegen units — and it cost **+1.4 s on every one-edit rebuild**:

| one-edit rebuild | overlay off | tags only | tags + in-binary descriptor |
|---|---|---|---|
| a string literal in a screen | 5.04–5.30 s | 5.63–5.64 s | 6.83–6.97 s |
| a trailing comment | 4.96–5.02 s | 4.85–5.02 s | 6.47–6.64 s |
| `macro_expand_crate` | 1.64–1.65 s | 1.69–1.79 s | 2.51–2.60 s |
| `serialize_dep_graph` | 0.37–0.60 s | 0.39–0.60 s | 0.61–0.78 s |

A 27% tax on exactly the loop the overlay exists to shorten, all of it in
expansion and dep-graph serialization and none of it in codegen — while
keeping only the node TAGS measured within noise of the feature being off
entirely.

So a descriptor is produced from SOURCE at build time, by the same split
pass the macro runs, and written beside the build. The compiled program
carries only what cannot be recovered from source:

- a `site_key` and a node index on each `Element` a site builds
  (`runtime_scene::NodeTag` — two integers), and
- one `SPLIT_VERSION` marker per program.

Three things follow, and each is better than the old arrangement:

1. **Validation moves to where the evidence is.** "Does this edit disturb
   a slot the compiled code supplies?" needs BOTH source versions. The
   differ has them; a running app never did.
2. **An over-the-air path archives each build's descriptor set** keyed by
   build id, rather than reading it back out of a shipped binary.
3. **The dev loop pays nothing** for a feature that exists to make the dev
   loop faster.

## Site identity

A site is named by where it is written, because that is the one thing both
halves can see — the proc macro reads it off its own call span, the
build-time producer off the file it is parsing. Exactly four things feed
it:

| part | value |
|---|---|
| `package` | `CARGO_PKG_NAME` of the crate being compiled |
| `file` | source path relative to that package's `CARGO_MANIFEST_DIR`, `/`-separated |
| `line`, `col` | 1-based position of the `ui!` invocation, from its call span |

`site_key(package, file, line, col)` folds those into the `u64` the
compiled code carries. It is FNV-1a with the parts separated by `0x1f`,
and it is a `const fn` — an addressing key, not a signature.

Position being part of the identity means inserting a line above a site
re-keys it. That is deliberate. The alternative, hashing the site's
tokens, re-keys on exactly the edits the overlay exists to serve. A moved
site looks to the differ like one site gone and another arrived, and the
answer is an ordinary rebuild; a literal edit, the case that matters,
moves nothing.

## The model

| type | what it is |
|------|------------|
| `Descriptor` | one site: a `SiteId`, a `SlotSig`, a FLAT `nodes` array and the `roots` into it |
| `Node::Prim` | a builtin primitive, named by its canonical snake_case string |
| `Node::Component` | a `#[component]` tag, named by the PascalCase path that is also its props type |
| `Node::Opaque` | addressable as a unit, not patchable inside — a `for`, an `if let`, a binding `match` arm, a node with a trailing `.method(…)` chain |
| `PropEntry` | `name = Lit(LiteralValue) \| Slot(index)` |
| `SlotSig` | one `SlotInfo` per slot: its role, its syntactic kind, and its prop name |
| `SiteId` | package + package-relative file + line + column |
| `Registry` | the descriptors a tool knows about, keyed by site |
| `Patch` | the edits to apply to one site |
| `SPLIT_VERSION` | the node-numbering version; a differ refuses a mismatched pair |

`Node::Prim`'s `kind` is a string and not a closed enum on purpose. The
overlay never CONSTRUCTS a tree from a descriptor wholesale, it addresses
one that already exists — so every primitive has to be nameable, including
ones no applier knows how to build. A closed enum would make "addressable"
and "constructible" the same set, and they are not.

`Node::Opaque::children` is not always empty: a `for`'s row body keeps its
nodes under the opaque node, so a row's literals stay patchable even though
the iteration is not.

## Node indices

`nodes` is flat and children are `u32` indices into it. Nesting `Node`
inside `Node` would need a `Box` per level; a flat array also lets a nested
template (an `if` branch's body) live in the same descriptor as its parent,
addressed by root index.

The indices are assigned by the split pass in emission order — the same
order the macro numbers its tags. That correspondence is the entire
addressing scheme. `SPLIT_VERSION` exists so a differ can refuse a binary
numbered by a walk it does not know, and the parity suite asserts the two
numberings agree over the whole fixture corpus.

One (site, node) pair addresses a SET of elements, not one: every row of a
`for` builds the same node of the same site. An applier edits every match.

## What `validate` checks

The slots are **code**. They were compiled into the binary from the
author's expressions and cannot be patched; only the descriptor can. So a
patch is accepted only when

- it is internally consistent — every child/root index in range, every slot
  reference declared;
- and its slot signature matches the compiled site's **shape for shape**.
  A descriptor that used slot 3 as a condition where the binary supplies a
  text value would address the wrong thing.

`SlotInfo::name` is deliberately NOT compared: it is reserved for a later
name-matched protocol, and comparing it now would reject harmless edits.

`kind` is a *syntactic* label (`"closure"`, `"path"`, `"call"`, …), not a
Rust type — a proc macro has tokens, never resolved types. It is a drift
detector, not a type check.

## Tests

`cargo test -p runtime-template` covers serde round-tripping, the site-key
fold, registration, and every `validate` rejection.

The proof that the numbering actually addresses a built tree lives in
`crates/dev/ui-lowering-parity`, which mounts every fixture with tags on
and checks the relation the flat node array depends on: within one site, a
node is numbered before everything beneath it.
