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
cargo test -p ui-lowering-parity                       # phase-1 goldens
cargo test -p ui-lowering-parity --features template   # + cross-lowering parity
```

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

### Sanctioned divergences from the frozen reference

None. Every fixture reproduces the pre-rewrite recording byte for byte
in both modes.
