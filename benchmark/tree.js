// Deterministic tree generator shared by the hierarchy suite's JS
// variants. The Rust (idealyst-native) variant ports the same
// algorithm to its own source. Both implementations MUST produce
// the same tree shape for a given seed — that's what makes
// cross-variant numbers comparable.
//
// PRNG: Mulberry32. Compact, well-distributed, identical-in-Rust.
// Algorithm copied verbatim from public-domain sources.

export function mulberry32(seed) {
  let state = seed | 0;
  return () => {
    state = (state + 0x6D2B79F5) | 0;
    let t = Math.imul(state ^ (state >>> 15), 1 | state);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0);
  };
}

/// Build the canonical tree shape for a given seed + target leaf
/// count. Returns:
///   {
///     root: NodeSpec,             // recursive tree
///     leaves: NodeSpec[],         // flat array of every leaf, in
///                                 // traversal order
///     targetLeaf: NodeSpec,       // pre-picked target for BRANCH
///                                 // updates (middle of leaves[])
///     totalNodes: number,         // branch + leaf count
///   }
///
/// `NodeSpec` shape:
///   { kind: 'leaf' | 'branch', id: number, depth: number, children?: NodeSpec[] }
///
/// Algorithm — every variant must implement identically:
///   - At each call, pull one rng() value and mod-100 it. <15 → leaf,
///     else → branch with (2 + rng() % 3) children (range 2..=4).
///   - Stop recursion if depth >= maxDepth or total leaves >= target
///     (any new node past that point is forced to a leaf, capping
///     the tree near `target`).
///   - `id` is monotonic, assigned in visitation order.
///
/// `maxDepth` defaults to a value sized to the requested target so
/// the tree actually has room to grow to `target` leaves. With
/// leaf_prob=15% and avg branching factor ~2.55, the natural
/// fanout caps at `2.55^maxDepth` leaves; we add a small headroom
/// so termination is driven by the target cap, not the depth cap.
///
/// Defaults: leafProb = 15%, branch factor avg ≈ 2.55.
///   target=2000     → maxDepth=10  (≈ 12k capacity)
///   target=10000    → maxDepth=12  (≈ 75k capacity)
///   target=100000   → maxDepth=14  (≈ 500k capacity)
export function genTreeShape(seed, target, maxDepth) {
  if (maxDepth == null) {
    // log_2.55(target) + 2 headroom levels.
    maxDepth = Math.max(8, Math.ceil(Math.log(Math.max(target, 1)) / Math.log(2.55)) + 2);
  }
  const rng = mulberry32(seed);
  let nextId = 0;
  let leafCount = 0;
  const leaves = [];

  function walk(depth) {
    const id = nextId++;
    // Force leaf when at depth cap OR when we've collected enough leaves.
    if (depth >= maxDepth || leafCount >= target) {
      const leaf = { kind: 'leaf', id, depth };
      leaves.push(leaf);
      leafCount++;
      return leaf;
    }
    const r = rng() % 100;
    if (r < 15) {
      const leaf = { kind: 'leaf', id, depth };
      leaves.push(leaf);
      leafCount++;
      return leaf;
    }
    const nChildren = 2 + (rng() % 3);
    const children = [];
    for (let i = 0; i < nChildren; i++) {
      children.push(walk(depth + 1));
    }
    return { kind: 'branch', id, depth, children };
  }

  const root = walk(0);
  // Middle leaf by traversal order. Stable, deterministic,
  // reproducible from seed alone. Used as the BRANCH-update target.
  const targetLeaf = leaves[Math.floor(leaves.length / 2)];
  return { root, leaves, targetLeaf, totalNodes: nextId };
}
