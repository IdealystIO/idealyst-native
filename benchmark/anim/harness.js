// Shared animation-benchmark helpers. Used by:
//
//   - Suite files (benchmark/suites/anim-*.js) — for driving the
//     determinism check, summarizing per-frame samples, computing
//     percentiles.
//   - Variant files (benchmark/<name>-anim/*) — for the seeded RNG
//     used to generate identical initial conditions across
//     variants. Variants MUST use this RNG; their own physics is
//     the only thing they may implement themselves.
//
// Dependency-free, untranspiled. Loaded as a plain ES module by
// every page that needs it.

// ─────────────────────────────────────────────────────────────────────
// Seeded RNG — mulberry32
//
// Cross-language portable: the same seed and same sequence of `rand()`
// calls produces the same Float32 values in JS, Rust, Swift, anywhere
// that has u32 wrapping arithmetic + IEEE-754 division. That's what
// makes the cross-variant determinism check possible without each
// variant having to ship its own RNG.
//
// State is a u32; output is a Float64 in [0, 1) but only 24 mantissa
// bits are populated — the bit pattern matches what Rust's
// equivalent implementation produces, so comparisons hold.

/**
 * mulberry32 seeded RNG. Returns a function with `.next()` (a u32-ish
 * uniform) and `.nextFloat()` (a [0, 1) Float64 with 24 bits of
 * entropy — matches the Rust impl bit-for-bit at the float-equality
 * boundary, modulo NaN handling which neither side produces).
 *
 * The state is captured in the closure, not exposed — calling sites
 * that need to fork the RNG should make two separate mulberry32(seed)
 * instances with different seeds.
 */
export function mulberry32(seed) {
  let state = seed >>> 0;
  return {
    next() {
      state = (state + 0x6D2B79F5) >>> 0;
      let t = state;
      t = Math.imul(t ^ (t >>> 15), t | 1) >>> 0;
      t ^= (t + Math.imul(t ^ (t >>> 7), t | 61)) >>> 0;
      return (t ^ (t >>> 14)) >>> 0;
    },
    nextFloat() {
      // Match Rust impl: take the high 24 bits and divide by 2^24.
      // Using >>> 8 (zero-fill right shift, 32→24 bits) then / 2^24
      // gives a closed-form [0, 1) distribution that's identical in
      // V8 and Rust's f64 arithmetic.
      const u = this.next();
      return (u >>> 8) / 16777216.0;
    },
    // Uniform on [lo, hi) — same shape as the Rust helper.
    uniform(lo, hi) {
      return lo + (hi - lo) * this.nextFloat();
    },
  };
}

// ─────────────────────────────────────────────────────────────────────
// Bounce initial-condition generator
//
// SHARED between variants. Every variant calls this with the same
// (n, seed) and gets back the same Float32Array of initial state.
// This is the single point of truth — a variant that disagrees with
// the harness on initial conditions cannot pass determinism, period.

/**
 * Build the initial [x, y, vx, vy, x, y, vx, vy, …] vector for the
 * bounce sim. Returns a Float32Array of length 4 * n.
 *
 * Constants here MUST match the spec.md "Physics (frozen)" section.
 * Changing any of these invalidates every reference vector in every
 * suite file.
 */
export function bounceInitial(n, seed) {
  const W = BOUNCE_VIEWPORT.width;
  const H = BOUNCE_VIEWPORT.height;
  const R = BOUNCE_BALL_RADIUS;
  const VMAX = BOUNCE_VELOCITY_RANGE;
  const rng = mulberry32(seed);
  const out = new Float32Array(4 * n);
  for (let i = 0; i < n; i++) {
    out[4*i + 0] = rng.uniform(R, W - R);          // x
    out[4*i + 1] = rng.uniform(R, H - R);          // y
    out[4*i + 2] = rng.uniform(-VMAX, VMAX);       // vx
    out[4*i + 3] = rng.uniform(-VMAX, VMAX);       // vy
  }
  return out;
}

/**
 * Advance the bounce sim by one fixed timestep, in-place. Operates
 * on the same [x, y, vx, vy, …] layout `bounceInitial` returns.
 * Reflects on viewport walls and clamps to the wall edge.
 *
 * Variants don't have to use this function — they can implement the
 * step themselves in their own language — but the math here is
 * authoritative. Use this as the reference when porting.
 */
export function bounceStep(state, dt) {
  const W = BOUNCE_VIEWPORT.width;
  const H = BOUNCE_VIEWPORT.height;
  const R = BOUNCE_BALL_RADIUS;
  for (let i = 0; i < state.length; i += 4) {
    let x = state[i + 0];
    let y = state[i + 1];
    let vx = state[i + 2];
    let vy = state[i + 3];
    x += vx * dt;
    y += vy * dt;
    if (x < R) { x = R; vx = -vx; }
    else if (x > W - R) { x = W - R; vx = -vx; }
    if (y < R) { y = R; vy = -vy; }
    else if (y > H - R) { y = H - R; vy = -vy; }
    state[i + 0] = x;
    state[i + 1] = y;
    state[i + 2] = vx;
    state[i + 3] = vy;
  }
}

export const BOUNCE_VIEWPORT = Object.freeze({ width: 800, height: 600 });
export const BOUNCE_BALL_RADIUS = 4;
export const BOUNCE_VELOCITY_RANGE = 200;  // dp/sec, each component in [-VMAX, +VMAX]
export const FIXED_DT_SEC = 1 / 60;

// ─────────────────────────────────────────────────────────────────────
// N-body sim: O(N²) all-pairs gravity + elastic collisions.
//
// State layout: [x, y, vx, vy, m] × N — five floats per body. Mass
// varies per body for visual interest (heavier bodies dominate the
// pair force). All operations f64-intermediate, f32-stored (same
// pattern as bounceStep — keeps cross-language determinism in
// [[project_cross_language_f64_constants]] territory).
//
// Why this shape and not pure bounce: bounce is O(N) per frame, so
// per-write FFI cost dominates. N-body is O(N²) compute + O(N)
// writes, so above some N the compute dominates and the language
// (JS vs wasm) starts to matter. That's the crossover this test
// surfaces.

export const NBODY_VIEWPORT = Object.freeze({ width: 800, height: 600 });
export const NBODY_BALL_RADIUS = 6;
/// Gravitational constant — tuned so visible motion appears within a
/// few seconds at our viewport / dt / mass-range scales. Not physically
/// real; chosen empirically so the sim looks like a 2D galaxy without
/// requiring tiny dt for stability.
export const NBODY_G = 800;
/// Softening parameter (added to r² before division) to prevent
/// singularity when two bodies are very close. Squared because we
/// compare it to r².
export const NBODY_SOFTENING_SQ = 16; // ε² ≈ (4)²
export const NBODY_MASS_MIN = 1;
export const NBODY_MASS_MAX = 4;
export const NBODY_VELOCITY_RANGE = 30; // small initial push, gravity does the rest

/**
 * Build the initial [x, y, vx, vy, m] × N vector for the N-body sim.
 * Returns Float32Array of length 5 * n.
 */
export function nbodyInitial(n, seed) {
  const W = NBODY_VIEWPORT.width;
  const H = NBODY_VIEWPORT.height;
  const R = NBODY_BALL_RADIUS;
  const V = NBODY_VELOCITY_RANGE;
  const rng = mulberry32(seed);
  const out = new Float32Array(5 * n);
  for (let i = 0; i < n; i++) {
    out[5*i + 0] = rng.uniform(R, W - R);
    out[5*i + 1] = rng.uniform(R, H - R);
    out[5*i + 2] = rng.uniform(-V, V);
    out[5*i + 3] = rng.uniform(-V, V);
    out[5*i + 4] = rng.uniform(NBODY_MASS_MIN, NBODY_MASS_MAX);
  }
  return out;
}

/**
 * Advance the N-body sim by one fixed timestep, in-place. Operates on
 * the same [x, y, vx, vy, m] × N layout `nbodyInitial` returns.
 *
 * Per frame: O(N²) pair-force accumulation + O(N²) collision pass +
 * O(N) integration + O(N) wall-reflect. The compute is dominated by
 * the two O(N²) inner loops.
 *
 * Iteration order is FIXED and PORTED to Rust byte-for-byte. Changing
 * the order (e.g., swapping i/j nesting) changes the FP summation
 * order, which diverges the cross-language determinism check.
 */
export function nbodyStep(state, dt) {
  const W = NBODY_VIEWPORT.width;
  const H = NBODY_VIEWPORT.height;
  const R = NBODY_BALL_RADIUS;
  const G = NBODY_G;
  const eps2 = NBODY_SOFTENING_SQ;
  const n = state.length / 5;

  // Accelerations buffer — reused across iterations is fine since we
  // overwrite per body. Float64Array to keep intermediates in f64;
  // store-back into state[] truncates to f32 at integration time.
  const ax = new Float64Array(n);
  const ay = new Float64Array(n);

  // O(N²) pair-force accumulation. Symmetric (Newton's 3rd law) — we
  // compute each pair once and add the equal-and-opposite force to
  // both bodies. Cuts work in half vs naive double loop.
  for (let i = 0; i < n; i++) {
    const xi = state[5*i];
    const yi = state[5*i + 1];
    const mi = state[5*i + 4];
    for (let j = i + 1; j < n; j++) {
      const dx = state[5*j] - xi;
      const dy = state[5*j + 1] - yi;
      const r2 = dx*dx + dy*dy + eps2;
      const invR3 = 1.0 / (r2 * Math.sqrt(r2));
      const mj = state[5*j + 4];
      const f = G * invR3;        // factor common to both: G / |r|³
      // Force on i toward j: F_i = f * mj * (dx, dy). a_i = F_i / m_i,
      // so the f * mj / m_i term — but we'll divide by mass at integration
      // time? Cleaner: accumulate force-per-unit-mass for i:
      //   da_i = f * mj * (dx, dy)
      //   da_j = -f * mi * (dx, dy)
      ax[i] += f * mj * dx;
      ay[i] += f * mj * dy;
      ax[j] -= f * mi * dx;
      ay[j] -= f * mi * dy;
    }
  }

  // O(N²) collision pass. When two bodies overlap (center distance <
  // 2R), exchange the normal component of velocity (perfectly
  // elastic) and separate positions so they don't stick. Uses
  // mass-weighted exchange — equal masses just swap normal vel; one
  // light + one heavy = heavy barely moves.
  const minDist = 2 * R;
  const minDistSq = minDist * minDist;
  for (let i = 0; i < n; i++) {
    const mi = state[5*i + 4];
    for (let j = i + 1; j < n; j++) {
      const dx = state[5*j] - state[5*i];
      const dy = state[5*j + 1] - state[5*i + 1];
      const dSq = dx*dx + dy*dy;
      if (dSq >= minDistSq || dSq === 0) continue;
      const d = Math.sqrt(dSq);
      const nx = dx / d;
      const ny = dy / d;
      const vxi = state[5*i + 2];
      const vyi = state[5*i + 3];
      const vxj = state[5*j + 2];
      const vyj = state[5*j + 3];
      // Relative velocity along the collision normal. Positive means
      // moving apart already — skip (otherwise we'd flip into
      // sticking).
      const vRelN = (vxi - vxj) * nx + (vyi - vyj) * ny;
      if (vRelN < 0) continue;
      const mj = state[5*j + 4];
      const totalM = mi + mj;
      // Impulse magnitude for elastic collision: J = 2 * (vRelN) /
      // (1/mi + 1/mj). Distribute J along the normal, signed away
      // from each body.
      const J = (2 * vRelN) / (1/mi + 1/mj);
      state[5*i + 2] = vxi - (J / mi) * nx;
      state[5*i + 3] = vyi - (J / mi) * ny;
      state[5*j + 2] = vxj + (J / mj) * nx;
      state[5*j + 3] = vyj + (J / mj) * ny;
      // Separate to prevent sticking — push apart by half the
      // overlap each, weighted by mass so heavy bodies barely move.
      const overlap = minDist - d;
      const pushI = overlap * (mj / totalM);
      const pushJ = overlap * (mi / totalM);
      state[5*i]     = state[5*i] - nx * pushI;
      state[5*i + 1] = state[5*i + 1] - ny * pushI;
      state[5*j]     = state[5*j] + nx * pushJ;
      state[5*j + 1] = state[5*j + 1] + ny * pushJ;
    }
  }

  // Integrate (semi-implicit Euler) + reflect on walls.
  for (let i = 0; i < n; i++) {
    let x  = state[5*i];
    let y  = state[5*i + 1];
    let vx = state[5*i + 2];
    let vy = state[5*i + 3];
    vx += ax[i] * dt;
    vy += ay[i] * dt;
    x  += vx * dt;
    y  += vy * dt;
    if (x < R)        { x = R;       vx = -vx; }
    else if (x > W-R) { x = W - R;   vx = -vx; }
    if (y < R)        { y = R;       vy = -vy; }
    else if (y > H-R) { y = H - R;   vy = -vy; }
    state[5*i]     = x;
    state[5*i + 1] = y;
    state[5*i + 2] = vx;
    state[5*i + 3] = vy;
    // mass unchanged
  }
}

/** Generate the harness's authoritative N-body state at `frame`. */
export function generateNbodyReference(n, seed, frame) {
  const state = nbodyInitial(n, seed);
  for (let i = 0; i < frame; i++) {
    nbodyStep(state, FIXED_DT_SEC);
  }
  return state;
}

// ─────────────────────────────────────────────────────────────────────
// Percentile helpers
//
// Cheap nearest-rank percentile — for the sample sizes the suite
// produces (≤600 frames per N) the sort cost is negligible. Don't
// reach for a streaming algorithm here; readability matters more.

/**
 * Nearest-rank percentile. `p` in [0, 100]. Returns `0` for empty
 * input (suites treat that as "no data"). Mutates a copy; doesn't
 * touch the input.
 */
export function percentile(arr, p) {
  if (arr.length === 0) return 0;
  const sorted = Array.from(arr).sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1,
                       Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[idx];
}

/**
 * Count how many samples in `arr` exceed `threshold`. Used for
 * `dropped_frames` (count of intervals > 18ms).
 */
export function countAbove(arr, threshold) {
  let n = 0;
  for (let i = 0; i < arr.length; i++) {
    if (arr[i] > threshold) n++;
  }
  return n;
}

// ─────────────────────────────────────────────────────────────────────
// Cross-variant determinism check
//
// Each suite ships a `references` table mapping (n, seed, frame) →
// expected state Float32Array. The check calls the variant's
// stepTo() and getState(), diffs against the reference, and throws
// on disagreement past tolerance.
//
// Tolerance is per-float and absolute (not relative). 1e-3 leaves
// room for the legitimate FP ordering differences between V8 and
// wasm (the order of additions in a tight loop can vary across
// compilers; that diverges the low bits within a few frames but the
// drift is sub-pixel for our scales). A variant that's actually
// wrong — wrong dt, wrong reflection direction, off-by-one in
// position — will fail this by orders of magnitude.

const DETERMINISM_TOLERANCE = 1e-3;

/**
 * Check that the variant's stepTo + getState produces the reference
 * state at the given (n, seed, frame). Throws on mismatch with a
 * clear message naming the first divergent index. No-op if the
 * reference is missing (so suites can be written before reference
 * vectors are generated — that's a build-time TODO, not a runtime
 * failure).
 */
export function assertReference(variant, n, seed, frame, reference) {
  if (!reference) return;  // not yet generated; skip
  variant.stepTo(frame);
  const got = variant.getState();
  if (got.length !== reference.length) {
    throw new Error(
      `determinism: state length mismatch (n=${n}, seed=${seed}, frame=${frame}) ` +
      `— expected ${reference.length}, got ${got.length}`,
    );
  }
  for (let i = 0; i < reference.length; i++) {
    const d = Math.abs(got[i] - reference[i]);
    if (d > DETERMINISM_TOLERANCE) {
      const bodyIdx = Math.floor(i / 4);
      const field = ['x', 'y', 'vx', 'vy'][i % 4];
      throw new Error(
        `determinism: divergence at body ${bodyIdx}.${field} ` +
        `(n=${n}, seed=${seed}, frame=${frame}) — ` +
        `expected ${reference[i]}, got ${got[i]} (Δ=${d.toExponential(2)})`,
      );
    }
  }
}

/**
 * Generate a reference Float32Array by running the harness's own
 * physics from scratch. Called by tooling that updates the embedded
 * reference vectors in suite files — NOT called at suite runtime
 * (suites embed pre-computed vectors so a variant can't accidentally
 * pass determinism by matching its own bug).
 */
export function generateBounceReference(n, seed, frame) {
  const state = bounceInitial(n, seed);
  for (let i = 0; i < frame; i++) {
    bounceStep(state, FIXED_DT_SEC);
  }
  return state;
}
