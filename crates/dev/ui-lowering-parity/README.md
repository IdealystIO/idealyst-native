# ui-lowering-parity — the two `ui!` lowerings must be interchangeable

`ui!` has two lowerings:

- **direct** — what `ui!` has always emitted, and still emits: builder
  calls inline at the call site.
- **template** — the same parsed tree, split into a `static` descriptor
  (data) plus an ordered list of dynamic expressions ("slots"), with
  `runtime_vocabulary`'s template builder constructing the `Element` from
  the pair.

They must produce **identical scenes**. This crate is the gate that
proves it, and — through phase 1's goldens — also proves that teaching
the direct lowering to read its dynamic values out of the slot list did
not change what it builds.

```
cargo test -p ui-lowering-parity                    # everything
cargo test -p ui-lowering-parity --no-default-features   # direct-only goldens
```

Both halves run in ONE process, which is the point: the two lowerings
are compared as two expansions of the same tokens inside the same
binary, not as two builds whose differences could be anything. The
`template` feature is on by default; turning it off leaves the
direct-only golden check, which is what you want while bisecting a
divergence.

## The contract

Every fixture in `src/fixtures.rs` is **authored once** and expanded
**twice**, by `ui_lowered!(direct { … })` and `ui_lowered!(template { … })`
over the same token trees. Each expansion is mounted against a
[`host_mock::Harness`] (the repo's canonical recording `runtime_scene::Host`
+ all-30-caps mock), driven through every signal the fixture exposes, and
recorded. Three projections of that recording must match, and they are
checked in this order because that is the order in which a divergence is
diagnosable:

| projection   | what it pins                                                     |
|--------------|------------------------------------------------------------------|
| `structural` | node creation + the 7-method `Host` seam (`insert`, `insert_at`, `insert_many`, `remove_child`, `clear_children`, `create_anchor`) |
| `full`       | every recorded capability call — props, styles, text updates, handler installs, lifecycle |
| `scene`      | the final tree (kind + captured text per node), after the last drive |

Each fixture is recorded in **both** structural modes:

- **anchored** (`supports_splice() == false`) — reactive regions nest
  under an anchor; swaps are `clear_children` + `insert`.
- **spliced** (`supports_splice() == true`) — regions splice into the
  real parent via `remove_child` + `insert_at`.

The two take different code through the scene drivers, so a lowering bug
that only shows in one of them would otherwise hide. Same split
`scene-parity` makes.

Assertion 3 in the task's phrasing ("identical op streams after driving
every signal the fixture exposes") is the per-step structure of
`structural`/`full`: the recording has one step per drive, labelled, so a
divergence is attributed to the mutation that caused it rather than to
the mount.

## Why the corpus is a `macro_rules!`

A fixture must be written once and expanded twice, from the same tokens.
That is a macro-level constraint — a data file or a `fn` taking a
lowering parameter cannot express it, because the lowering is chosen at
expansion time. So the corpus is `fixture! { … }` invocations, each
generating a module with:

- `St` — the signals the fixture exposes, plus `make()`,
- `DRIVES` — the labelled mutations the harness replays,
- `direct(&St) -> Element` and (under `--features template`)
  `template(&St) -> Element`, expanded from the same `body { … }` tokens,
- `record_direct(Mode)` / `record_template(Mode)`.

All four sections (`state`, `locals`, `drive`, `body`) are mandatory;
empty braces when unused. An optional-section macro would have to guess,
and a fixture that silently dropped its drive list would assert nothing.

## What the corpus covers

Node kinds: `view`, `text` (literal / closure / f-string / `content`
prop), `button` (literal label, closure handler, and the
`on_click = f(sig) => out` arrow shape), `image`, `activity_indicator`,
`link`, `scroll_view`, `text_input`, `toggle`, `slider`, `overlay`,
`anchored_overlay`, `presence`, `flat_list`, `when`.

Composition: components with all-literal props, with defaulted props,
with dynamic (signal / `Option<Rc<dyn Fn()>>`) props, with a `children`
splat, and nested inside each other; bare-expression children (a helper
`fn` call and a `Vec<Element>` splat); trailing method chains (`.bind(…)`).

Control flow: reactive `if` (with and without `else`, and an `else if`
chain), static `if`, `if let`, static `match`, reactive `match`, a
guarded arm, `for` over a `Vec`, over a static range (the batched
`Repeat` path), over a reactive range, keyed over a reactive collection
(single- and multi-node rows), and a reactive `if` wrapping a keyed
`for`.

Styling / identity / a11y: `stylesheet!` application, a variant
selection, `test_id`, and the a11y attribute set.

### The `scene-parity` shapes, re-authored

`crates/dev/scene-parity` freezes the op sequences for 13 reactive
scenarios — but it builds them through `runtime_scene`'s constructors
and contains **no `ui!` at all**, so there is nothing there for a
*lowering* to apply to; adding a mode to its matrix would be adding a
mode that changes nothing. Its goldens stay green as a separate
invariant.

The equivalent coverage lives here instead: three fixtures re-author its
structural corpus in `ui!` —
`for_keyed_reorder_and_insert` (`each_reverse` +
`each_insert_middle_survivors` + a removal),
`reactive_if_in_keyed_row` (`nested_when_in_each_row`), and
`reactive_match_rotation` (`switch_rotation`) — alongside the
`reactive_if` / `for_keyed_reactive` / `for_keyed_multi_node_rows`
fixtures that already mirror `when_toggle`, `each_append` and
`each_multi_node_rows`.

Not carried over: `dispose_order_when` / `dispose_order_each`, which pin
cleanup ordering relative to the structural ops via `on_cleanup` markers.
`host-mock`'s op log has no cleanup line to interleave, so the shape is
covered only by the `unmount` step's op sequence.

## Phase-1 goldens

`goldens/<fixture>.<mode>.golden` is a recording of the **pre-slot-rewrite**
direct emitter, captured before `ui.rs` was touched and committed as the
reference. They exist because self-consistency is not the property that
matters: a suite comparing the rewritten lowering only against itself
would be satisfied by a uniformly-wrong emitter. Comparing against a
frozen recording of the old one makes "behavior-preserving" falsifiable.

```
UPDATE_UI_PARITY_GOLDENS=1 cargo test -p ui-lowering-parity
```

writes them. Re-baselining discards the pre-rewrite reference
permanently, so do it only after reviewing the diff — and record the
reason in the divergence list below.

**Every** golden in the corpus is a pre-rewrite recording, including the
fixtures added after the rewrite landed. To capture one, restore the
pre-rewrite emitter over the macro crate, write the goldens, and put the
current one back:

```
git checkout <pre-rewrite sha> -- crates/runtime/macros/
UPDATE_UI_PARITY_GOLDENS=1 cargo test -p ui-lowering-parity \
    --no-default-features --test goldens
# the diff must show ONLY the new fixture's files
git checkout HEAD -- crates/runtime/macros/
```

`--no-default-features` because the pre-rewrite crate has no template
emitter. The step is worth the ceremony: a golden captured from the
CURRENT emitter only proves self-consistency, which is the property the
corpus exists to go beyond. (It doubles as a re-verification — the
rewrite is confirmed byte-for-byte whenever the rewrite of every existing
golden comes back empty.)

### Sanctioned divergences from the frozen reference

None. Every fixture reproduces the pre-rewrite recording byte for byte
in both modes, under BOTH lowerings.

## The suites

| test | what it pins |
|------|--------------|
| `goldens.rs :: direct_lowering_matches_the_frozen_reference` | the slot-list rewrite changed nothing the pre-rewrite emitter did |
| `goldens.rs :: recordings_are_deterministic` | the same fixture recorded twice is byte-identical (otherwise every other assertion here is flaky, not false) |
| `goldens.rs :: every_fixture_has_a_golden_and_every_golden_a_fixture` | the corpus cannot silently shrink |
| `parity.rs :: every_fixture_builds_the_same_scene_under_both_lowerings` | the cross-lowering contract, all three projections, both modes |
| `parity.rs :: template_lowering_matches_the_frozen_reference` | the template lowering against the FROZEN goldens too — so a regression that moved both lowerings the same way still fails |
| `apply_literal.rs` | `#[component]`/`#[props]`' generated `__apply_literal`: literal application through the `Reactive` wrap, integer narrowing, refusal reporting, and the blanket fallback for a props type the macro never touched |

## Descriptor-native vs escaped

The template lowering does not need every node to be descriptor-native
to be correct: a node the descriptor cannot model becomes a
`Node::Escape`, built by the direct emitter and handed to a slot as a
finished `Element`. So the template half of this suite passes from the
first commit, and what grows over time is how much of each fixture is
DATA rather than code.

That boundary is pinned where it is decided — in `runtime-macros`'
`ui_template` unit tests, node kind by node kind (`view`/`text`/`button`
are `Prim`; a reactive `if` is `Dyn`; a `for`, a `match`, a chained node,
an unmodelled primitive, a component with a dynamic prop all `Escape`).
A widening of the native set therefore shows up in that diff, and this
suite is what proves the widening did not change any scene.

An escaped node's BODIES stay template-lowered (the macro keeps an
ambient-lowering flag for exactly this), so a fixture like
`for_keyed_reactive` still exercises the template builder on its row
bodies even though the `for` itself escapes.
