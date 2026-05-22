# Animation benchmark spec

Head-to-head animation benchmarks across UI frameworks. Same simulation,
same per-frame work, same instrumentation. Companion to [the rebuild
spec](../spec.md) — that one measures one-shot mount cost; this one
measures **steady-state per-frame cost** at scale.

## What this measures and why

Most frameworks ship some form of `requestAnimationFrame`-driven update
path. The interesting question is what each one *costs per node per
frame* once a few hundred to a few thousand things are moving at once.
The differentiating axes:

- **Per-write FFI / DOM cost** — when N nodes all need a `transform`
  update every frame, how cheap is one write?
- **Author-side compute cost** — does the framework's choice of host
  language (JS vs Wasm) actually matter once the workload becomes
  compute-bound? Naively, "Wasm is faster" is wrong for animation —
  you almost always cross the boundary once per write, and that's
  where the time lives. This benchmark surfaces where the crossover
  sits.

## Tests

Three tests are planned. Initial cut ships **`bounce`** only; the others
land on the same harness as follow-ups.

| Test          | Per-frame author work          | Writes/node/frame | N sweep                |
| ------------- | ------------------------------ | ----------------- | ---------------------- |
| `bounce`      | ~10 flops × N                  | 2 (transforms)    | 100, 1k, 5k, 10k, 25k  |
| `nbody`       | O(N²) gravity + collisions     | 2 (transforms)    | 50, 100, 200, 500      |
| `springstorm` | framework's spring × N         | 2 (transforms)    | 100, 1k, 5k, 10k       |

### `bounce` — lightweight, scale to break

N circles bouncing inside a fixed viewport. Each frame: integrate
position with constant velocity, reflect on edges. Author math is
trivial — the whole point is to measure the **framework's per-write
overhead** with the compute factored out.

Physics (frozen — changes here invalidate reference vectors):

- Viewport: `800 × 600` dp
- Ball radius: `4` dp
- Initial positions: seeded `mulberry32(seed)`; uniform in
  `[r, W-r] × [r, H-r]`
- Initial velocities: each component uniform in `[-200, +200]` dp/sec
- Update: `pos += vel * dt`, then for each axis: if `pos < r` or
  `pos > side - r`, reflect velocity and clamp position to the wall
- Fixed dt for determinism: `1/60 s` (16.666… ms)

### `nbody` — computationally heavy (future)

N bodies, all-pairs gravitational attraction (`G * m1 * m2 / r²`,
softened) plus elastic collision response. Verlet would be more stable
at large dt but for short windows semi-implicit Euler is fine and
matches what the rest of the framework's animation system uses.

### `springstorm` — framework interpolator under load

N nodes, each with an independent spring driving `translateY` between two
random targets. On `start()`, every spring is kicked toward a fresh
target; every ~500ms the suite re-kicks them to a new target so the
springs are continuously running rather than settling and going idle.

**This is the test for idealyst's intended animation API.** Bounce
measures the framework as a *write surface* (author owns rAF, calls
`AV.set()` imperatively). Springstorm measures the framework's
*scheduler + interpolator* — `AV.animate(SpringTo::new(target))` per
ball — driving N independent timelines through the clock's per-frame
tick loop.

What this exposes that bounce doesn't:
- The clock's `HashMap<TickId, TickFn>` iteration cost at high N.
- Per-AV spring integration cost (semi-implicit Euler — ~10 flops per
  AV per frame).
- Whether the per-AV scheduler is the bottleneck before the FFI write
  path is.

#### Asymmetric `apply` measurement — documented and intentional

The `apply` metric is captured by bracketing the *variant's own
per-frame closure*. For springstorm this differs across variants:

- **Vanilla springstorm**: variant owns the rAF closure that
  integrates N springs and writes N transforms. `apply` captures
  everything — comparable to bounce.
- **Idealyst springstorm**: the framework's clock owns the rAF that
  drives the N springs; the variant's own rAF closure just re-kicks
  targets periodically. `apply` for idealyst is near zero — it does
  *not* capture the framework's per-frame work.

This is intentional. We measure framework cost from the *outside* in
springstorm via `FPS` and `MAX ms` — both observed identically across
variants regardless of who owns the rAF. A variant with a fast
framework holds high FPS at high N; a slow one drops to low FPS or
shows large MAX ms spikes.

When reading the table: **for springstorm, use `FPS` and `MAX ms` as
the headline metrics, not `µs/FRAME`.** The `µs/FRAME` column is still
meaningful for vanilla (it's the JS spring integrator's cost) but
should be ignored for idealyst (it's the re-kick rAF, which doesn't
reflect the framework's actual per-frame cost).

Determinism check is skipped for springstorm — spring math depends on
the per-framework integrator implementation, so cross-variant state
won't match bit-for-bit even when both are correct.

## Variant contract

Each variant exposes four hooks via `window`-level globals OR the
suite-import path used by other benchmark variants:

```js
// Set up the screen for `test` at population `n` using `seed`. Mounts
// the nodes but does not begin animating. Resolves when the DOM is
// ready and getState() will return valid initial state.
async setupAnim(test, n, seed)

// Drive the sim deterministically to frame `frameN` using fixed
// dt = 1/60 s. Synchronous; no rAF involvement. Used by the
// cross-variant determinism check.
stepTo(frameN)

// Return the current canonical sim state as a Float32Array of
// [x, y, vx, vy] per body, ordered by body id. Length = 4 * n.
// Used both by the determinism check and as ground truth for
// verifying the variant's draw matched its physics.
getState()

// Begin perf-mode animation: real rAF, real wall-clock dt. Runs
// until stopAnim() is called. Returns a Promise that resolves when
// stopAnim() has fired AND the variant has captured its sample
// window — payload is { jsPerFrame: number[], frameDt: number[] }.
async startAnim()
async stopAnim()
```

### Two modes, by design

- **Determinism mode** — `setupAnim` then `stepTo(60)`, `stepTo(120)`,
  …, comparing `getState()` against a frozen reference vector embedded
  in the suite. Validates the variant is actually computing the
  physics correctly (or, equivalently, that it's not silently skipping
  writes). Tolerance: `|Δ| < 1e-3` per float (slack for FP order-of-
  operations between V8 and wasm).
- **Perf mode** — `setupAnim` then `startAnim()`, sample for W
  seconds, `stopAnim()` returns the per-frame logs. The suite computes
  percentiles and posts them as the variant's result.

A variant must pass determinism before perf numbers are accepted.
Determinism failures fail the suite loud with a `bench-error` posting.

### Why expose state as `[x, y, vx, vy]` instead of pixel-diffing

CSS subpixel rounding, `transform: translate3d` vs `translate`, and
DPR-aware GPU sampling all produce legitimately-different rendered
output across variants — pixel-diffing would force every variant to
the same renderer path. Diffing the canonical sim state is what we
actually want: it asserts "your physics matches the reference," which
is the cross-framework invariant. Each variant is then trusted to
draw what its state says.

## Instrumentation captured by the variant

Each frame in perf mode, the variant records:

- `jsPerFrame` — `performance.now()` delta bracketing the variant's
  per-frame update (compute + per-node writes). This is the
  framework's contribution.
- `frameDt` — `performance.now()` delta between consecutive rAF
  callback fires. This is what the user actually sees.

Captured into preallocated ring buffers (length = 600, ≈ 10s at
60Hz). The suite reads them at `stopAnim()` time and computes:

- `js_p50`, `js_p95` (ms)
- `frame_p50`, `frame_p95`, `frame_max` (ms)
- `dropped_frames` — count of `frameDt > 18` (ms)

## How animation results map onto the runner table

The runner's existing per-iteration record shape is `{ bucket, apply,
firstPaint, worstFrame }`. We reuse it without changing the runner —
**column meanings shift** for animation suites. The runner's per-suite
`SUITE_COLUMN_LABELS` override relabels the header so the table is
self-describing; the contract here is what the underlying fields hold.

| Field        | Header label  | Meaning                                              |
| ------------ | ------------- | ---------------------------------------------------- |
| `bucket`     | column group  | population N (special: `0` = idle-rAF floor)        |
| `apply`      | `µs/FRAME`    | mean per-frame variant JS work, in microseconds      |
| `firstPaint` | `FPS`         | mean frame rate over the sample window               |
| `worstFrame` | `MAX ms`      | worst single frame interval, in milliseconds         |
| (derived)    | `TOTAL`       | sum of `µs/FRAME` across N tiers (rough complexity)  |

### Why these specific metrics

These three measure complementary aspects of framework cost:

- **`µs/FRAME`** (mean per-frame JS work). Measures the framework's
  controllable cost. Sweep across N: subtract `µs/FRAME` at N=0 from
  `µs/FRAME` at N=X to get the per-write framework cost (slope).
- **`FPS`** (mean rate over the window). The user-visible smoothness
  number. A framework can have low `µs/FRAME` yet still tank FPS at
  scale if too much GPU/compositor work was queued; FPS reveals that.
- **`MAX ms`** (worst single frame). Surfaces hitches that the mean
  smooths over. A GC pause, a layout thrash, a stop-the-world wasm
  module instantiation.

### The N=0 floor row

`bucket=0` is special: the suite calls `setupAnim('bounce', 0, seed)`,
which mounts zero balls. The rAF loop still fires every frame; the
variant's per-frame closure runs but iterates over zero work. So
`µs/FRAME` at N=0 measures the **framework's fixed per-frame floor** —
for wasm-backed frameworks like idealyst-native this includes the
JS→wasm boundary crossing, the framework's rAF wrapper, and any
per-frame bookkeeping. For pure-JS variants it's the bare
`requestAnimationFrame` callback dispatch.

Subtract `µs/FRAME @ N=0` from `µs/FRAME @ N=X` and divide by X to
get the per-write cost. The two together separate fixed overhead
from per-write overhead — both are framework-attributable.

Determinism check is skipped for N=0 (nothing to verify).

## Variants in v1

| Variant id              | Idiom                                        |
| ----------------------- | -------------------------------------------- |
| `vanilla-anim`          | Author rAF + `el.style.transform`            |
| `idealyst-native-anim`  | `raf_loop_scoped` + `AnimatedValue::set()`   |

Later: react-anim (`useRef` + manual rAF, no reconciliation), rn-web-anim
(`Animated.timing` parallel). Reanimated explicitly skipped — its web
fallback is the JS-driver shim and benchmarking it would mislead.

## Why no Reanimated, no SwiftUI, no Compose

- Reanimated's worklets run on the UI thread on **native** — on web they
  fall back to JS/CSS. Benchmarking that here would attribute the
  framework's *non-strength* and read as a hit piece.
- SwiftUI / Compose run on native; the existing benchmark harness is
  web-only. Native animation benchmarks are a separate slot.

## Adding a new variant

1. `mkdir benchmark/<name>-anim/` and implement the four-hook contract.
2. Call `autoRunIfRequested({ setupAnim, stepTo, getState, startAnim, stopAnim })`
   from `../instrument.js` after setup is done.
3. Add a `VariantInfo` entry in [src/lib.rs](../src/lib.rs) with
   `supports: &["anim-bounce", ...]`.
4. Run the determinism check (it's automatic — runs as part of the
   suite at iteration 0). Fix until it passes.
5. Submit perf numbers.

## Honesty rules

- Use the framework's idiomatic per-frame surface. A React variant
  should use refs + rAF (no `useState` per ball — that's a strawman).
  An Idealyst variant should use `AnimatedValue::set()` driven from a
  single `raf_loop_scoped`, not bind N independent tweens (also a
  strawman, and the welcome example already establishes this pattern
  as the framework's idiomatic shape).
- A variant must not skip writes during sample windows. The
  determinism check guards against accidental skips; **deliberate**
  cheating (only writing every other frame) is detectable by
  comparing `getState()` against the reference at the end of the
  perf window and is disqualifying.
- Variants are free to choose their representation of a "ball" (DOM
  div, SVG circle, framework primitive) — but pick the one that's
  idiomatic for the framework, not the fastest one you can find.
