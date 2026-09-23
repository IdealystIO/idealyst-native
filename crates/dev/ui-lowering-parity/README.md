# ui-lowering-parity — the `ui!` emission, and the overlay that edits it

Two contracts live here, and both are about things that cannot be
checked from one side alone.

**1. The emission does not drift.** Every fixture is recorded against a
frozen golden captured from the emitter as it was BEFORE the slot-list
rewrite. Self-consistency is not the property that matters: a suite
comparing the emitter only against itself is satisfied by a uniformly
wrong one.

**2. The overlay addresses what the emission built.** The compiled app
carries only numbers — a site key and a node index per node. The
descriptor that gives those numbers meaning is produced from SOURCE at
build time by a different code path. If the two ever disagree, every
patch after the first divergence edits the wrong element, **and nothing
else fails**: both halves stay internally consistent and patches simply
never match. This crate is the only place both run on one input.

```
cargo test -p ui-lowering-parity                    # the emission contract
cargo test -p ui-lowering-parity --features ui-overlay   # + the overlay's
```

The goldens are the SAME files either way. That is the byte-identity
gate: turning the feature on must not change what a site builds.

## The suites

| test | what it pins |
|------|--------------|
| `goldens.rs :: direct_lowering_matches_the_frozen_reference` | the emission still builds what the pre-rewrite emitter did — 49 fixtures × 2 structural modes × 3 projections |
| `goldens.rs :: recordings_are_deterministic` | the same fixture recorded twice is byte-identical (otherwise every other assertion here is flaky, not false) |
| `goldens.rs :: every_fixture_has_a_golden_and_every_golden_a_fixture` | the corpus cannot silently shrink |
| `goldens.rs :: a_site_numbers_a_node_before_everything_under_it` | the feature is DOING something when on, and the one relation a flat node array with `u32` child indices depends on |
| `descriptor.rs` | every tag lands on the node the build-time descriptor gives that number — and the corpus interleaves control flow with elements, so that check is not vacuous |
| `site_key.rs` | the macro and the source scanner name the same SITE |
| `apply.rs` | the applier, on mounted scenes: what it changes and — six of twelve cases — what it refuses |
| `round_trip.rs` | `Element(original) + apply(diff(desc(original), desc(edited))) == Element(edited)` |
| `apply_literal.rs` | `#[component]`/`#[props]`' generated `__apply_literal`: application through the `Reactive` wrap, integer narrowing, refusal reporting, and the blanket fallback for a props type the macro never touched |
| `double_eval.rs` | props the emitter used to splice twice stay spliced once |
| `overlay_props.rs` | `overlay(click_through = …)` reaches the portal (it used to compile and be dropped) |

## How a fixture is recorded

Each fixture is mounted against a [`host_mock::Harness`] (the repo's
canonical recording `runtime_scene::Host` + all-30-caps mock), driven
through every signal it exposes, and recorded. Three projections must
match, checked in this order because that is the order in which a
divergence is diagnosable:

| projection   | what it pins                                                     |
|--------------|------------------------------------------------------------------|
| `structural` | node creation + the 7-method `Host` seam (`insert`, `insert_at`, `insert_many`, `remove_child`, `clear_children`, `create_anchor`) |
| `full`       | every recorded capability call — props, styles, text updates, handler installs, lifecycle |
| `scene`      | the final tree (kind + captured text per node), after the last drive |

Every fixture is recorded in **both** structural modes — **anchored**
(`supports_splice() == false`: reactive regions nest under an anchor,
swaps are `clear_children` + `insert`) and **spliced** (regions splice
into the real parent via `remove_child` + `insert_at`). The two take
different code through the scene drivers, so a bug that only shows in
one of them would otherwise hide. Same split `scene-parity` makes.

The recording has one step per drive, labelled, so a divergence is
attributed to the mutation that caused it rather than to the mount.

## Why the corpora are `macro_rules!`

A fixture's tokens must be available to the compiler AND, for the
overlay half, to the parser library as source. `stringify!` inside the
generating macro is how one authoring reaches both — there is no way to
write the body twice and still claim the two halves saw the same input.

`src/fixtures.rs` holds `fixture! { … }` (emission contract); each
invocation generates `St` + `make()`, the labelled `DRIVES` the harness
replays, the `ui!` expansion, and the body as a string. All four
sections (`state`, `locals`, `drive`, `body`) are mandatory, empty braces
when unused: an optional-section macro would have to guess, and a fixture
that silently dropped its drive list would assert nothing.

`src/edits.rs` holds `pair! { … }` (overlay contract): one site written
TWICE, `original` and `edited`, both compiled and both kept as source,
plus whether the edit is expected to be patchable at all.

## What the emission corpus covers

Node kinds: `view`, `text` (literal / closure / f-string / `content`
prop), `button` (literal label, closure handler, and the
`on_click = f(sig) => out` arrow shape), `image`, `activity_indicator`,
`link`, `scroll_view` (both a minimal and a full-prop form),
`text_input`, `toggle`, `slider` (controlled and uncontrolled),
`icon` (bare, reactive `color`/`stroke`, `draw_in`, `animate`),
`graphics`, `overlay` (modal and non-modal), `anchored_overlay`,
`presence`, `flat_list`, `when`, and the full a11y surface
(`accessibility` / `a11y_role` / `a11y_traits` / `live_region`).

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
and contains **no `ui!` at all**. The equivalent coverage lives here:
three fixtures re-author its structural corpus in `ui!` —
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

## What the edit corpus covers

Patchable: a text literal, a nested one beside an untouched sibling, an
a11y literal, a child appended, a child removed, children reordered, and
a literal edited next to a reactive sibling that must be left alone.

Refused, and this half matters as much — an edit that must be refused
silently becoming accepted-and-wrong is the failure the whole design
exists to make impossible: a changed `if` condition, a changed reactive
text body, a prop that went from a literal to a closure, and a static
child added beside control flow.

`round_trip.rs` also pins that diffing a descriptor against ITSELF
produces nothing. Without it, a differ that emitted a spurious `SetProp`
for every node would still pass — the patch would just happen to write
the values that were already there.

## The frozen goldens

`goldens/<fixture>.<mode>.golden` is a recording of the
**pre-slot-rewrite** emitter, captured before `ui.rs` was touched.

```
UPDATE_UI_PARITY_GOLDENS=1 cargo test -p ui-lowering-parity
```

writes them. Re-baselining discards the pre-rewrite reference
permanently, so do it only after reviewing the diff — and record the
reason in the divergence list below.

**Every** golden is a pre-rewrite recording, including fixtures added
after the rewrite landed. To capture one, restore the pre-rewrite
emitter over the macro crate, write the goldens, and put the current one
back:

```
git checkout <pre-rewrite sha> -- crates/runtime/macros/
UPDATE_UI_PARITY_GOLDENS=1 cargo test -p ui-lowering-parity --test goldens
# the diff must show ONLY the new fixture's files
git checkout HEAD -- crates/runtime/macros/
```

The ceremony is worth it: a golden captured from the CURRENT emitter
proves only self-consistency, which is the property the corpus exists to
go beyond.

### Sanctioned divergences from the frozen reference

None. Every fixture reproduces the pre-rewrite recording byte for byte
in both modes, with `ui-overlay` on and off.

## One prop the emitter drops

This suite is how `overlay(click_through = …)` surfaced: `emit_overlay`
never lowered it. The `overlay_non_modal` fixture pins the drop rather
than hiding it — fixing it is a behaviour change for the emitter and
belongs in its own commit.
