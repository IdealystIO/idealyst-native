# Architectural Performance Audit: Idealyst Framework

## Overview

The Idealyst framework is a fine-grained reactive UI framework compiling to WebAssembly that drives the DOM via wasm-bindgen. The architecture follows a virtual-DOM-style pattern: the build walker (`build()` in `lib.rs`) traverses a `Primitive` tree to produce backend nodes. Styling is applied reactively via per-node `Effect`s that capture node handles and closures. The framework uses an arena-based signal/effect storage system (in `reactive.rs`) with freelists for recycling slots. The web backend (`backend-web/src/`) manages stylesheet registration, dynamic CSS class minting, and per-node state. Current performance: ~50ms for 1000-row mount, ~120ms for 10000-row. The architecture is fundamentally sound but has several inefficiency clusters worth addressing.

---

## Per-Row Cost Breakdown (10,000-row mount estimate)

**Assumptions:** styled View + Text per row, variant-only style (no overrides), no state blocks, pre-thawed theme.

| Component | µs/row | Count | Total |
|-----------|--------|-------|-------|
| **Macro expansion (for loop)** | — | 10k iterations | ~500–800µs |
| — Primitive::View allocation | 0.1 | 10k | 1ms |
| — Primitive::Text allocation | 0.1 | 10k | 1ms |
| **Build walker per node** | | | |
| — `build()` dispatch | 0.5 | 20k | 10ms |
| — `create_view()` FFI + DOM node | 2.5 | 10k | 25ms |
| — `create_text()` FFI + DOM node | 1.5 | 10k | 15ms |
| — `insert()` FFI (appendChild) | 1.8 | 10k | 18ms |
| **Styling per row** | | | |
| — Effect::new allocation + first run | 3.0 | 10k | 30ms |
| — `resolve_style()` + cache lookup | 1.2 | 10k | 12ms |
| — `ensure_registered_with()` | 0.8 | 10k | 8ms |
| — `apply_styled_states()` + FFI | 2.5 | 10k | 25ms |
| **Reactivity overhead** | | | |
| — Effect arena slot allocation | 1.0 | 10k | 10ms |
| — Signal arena slot allocation | 0.3 | 10k | 3ms |
| — Scope registration + closure capture | 0.5 | 10k | 5ms |
| **Teardown (post-apply, async)** | | | |
| — Effect drop closures (10k × decref) | 3.5 | 10k | 35ms (async) |
| **Misc allocations + clones** | | | |
| — Node clones (per effect) | 2.0 | 10k | 20ms |
| — Backend Rc clones | 0.3 | 10k | 3ms |
| — StyleApplication clone (per row) | 0.2 | 10k | 2ms |
| **TOTAL APPLY** | | | **~180ms** |
| **TOTAL TEARDOWN (async)** | | | **~35ms+** |

Reality is likely ~120ms total observed because the row struct is simpler (single variant, no overrides). The per-row FFI + Effect allocation dominates. The biggest single chunk is DOM node creation (~25+15+18=58ms) and Effect/resolver machinery (~30+12+8+25=75ms). The two together account for ~75% of apply time.

---

## Inefficiencies and Improvement Opportunities

### A. FFI Crossing Count Per Row

**Where:** `build_view`, `build_text`, `build()` dispatch, `insert()`, `apply_styled_states()`

**Cost:** Each row triggers:
- `create_view()` – 1 FFI (createElement)
- `create_text()` – 1 FFI (createTextNode)
- `insert()` – 1 FFI (appendChild)
- `apply_styled_states()` – 1 FFI (setAttribute class)
- (optional) `on_node_unstyled()` at drop – 1 FFI (cleanup)
= **4–5 FFI calls per row minimum**, more if state overlays present.

**Why it costs:** Wasm-bindgen crossings are ~2–5µs each on modern browsers. 10k rows × 4 calls = 40k crossings, ~80–200ms total depending on browser overhead.

**The fix:** Batch DOM operations. Modern browsers support:
- `DocumentFragment` (batch multiple `appendChild` calls into one)
- `<template>.content.cloneNode(true)` (one FFI call for a whole subtree)
- CSS class bulk updates (set `className` once instead of multiple attribute calls)

**Effort:** Medium. Requires:
1. Pre-generate a `<template>` for the styled-row structure at registration time (if shape is identical across rows).
2. Use `cloneNode` + shallow-bind in `create_view/text` when available.
3. Batch `insert` calls via fragment + single append.

**Risk/Scope:** Medium. Template approach works only for truly uniform row shapes. Variant rows with different structure still need individual creation. Fallback for non-template rows is standard node creation.

**Expected gain:** 30–50% of mount time if rows are uniform. For the perf-screen (1000–10k identical-structure rows), this is 20–35ms saved.

---

### B. Primitive Enum as Intermediate Virtual DOM

**Where:** `ui!` macro lowering, `build()` match dispatch

**Cost:** For 10k rows:
- Each `for i in 0..n { View(...) { Text(...) } }` iteration allocates 2 `Primitive` enums (View + Text).
- Each enum is ~200 bytes (6–8 variants, children Vec, style Source, ref_fill).
- 10k rows × 2 × 200B = 4MB allocated + immediately freed during the build walk.
- GC pressure from 20k short-lived allocations.

**Why it costs:** The macro's `emit_for` produces:
```rust
{
    let mut __c = Vec::new();
    for i in 0..n {
        let prim = /* build primitives */;
        ChildList::append_to(prim, &mut __c);
    }
    __c
}
```
Each iteration constructs a full `Primitive::View { children: vec![...], ... }`, which is walked immediately and dropped.

**The fix:** Lower `for` loops directly to backend-create calls where possible:
- If the macro detects that the body produces N identical-structure rows, it could emit a special `build_many(template, count)` call that:
  1. Clones the template N times natively (via `cloneNode` if browser-available, else Rust allocation).
  2. Calls `apply_style` on each clone atomically.
  3. Inserts all at once.
- For non-uniform rows (variant changes inside loop), fall back to current behavior.

**Effort:** Large. Requires:
1. Macro analysis to detect uniform loops (heuristic: does body contain `i % 2` or similar to determine parity? Does the condition read a loop variable directly vs. deriving from it?).
2. New backend method `build_many` or codegen branch in walker for bulk row creation.
3. Template caching layer to avoid re-scanning the same row shape per mount.

**Risk/Scope:** High complexity. The heuristic needs to be conservative (false negatives are fine; false positives break rendering). Only applies to rows with fixed structure per iteration.

**Expected gain:** 20–40% if rows are uniform. The cost is mostly Effect allocation and FFI, not Primitive allocation itself, so the win is limited unless combined with cloneNode batching.

---

### C. Per-Node Effect Overhead

**Where:** `attach_style()` in `lib.rs`, Effect::new, Signal::new per styled node

**Cost:** Each styled row allocates:
- 1 `Effect` slot (arena) + first run – ~3µs
- 1 `Signal<StateBits>` slot (if !handles_states_natively) – ~0.3µs
- Closure capture: `Rc<RefCell<Backend>>`, `Node` clone, `StyleHandle`, style closure – ~5–10 bytes each
- On drop (async): deref + JS-side wasm-bindgen closure cleanup – ~2–3µs per closure

**Why it costs:** For web (which *does* handle states natively), we still allocate the Style Effect. For 10k rows, that's 10k Effect slots, 10k arena registrations, 10k closure allocations. Dropping them (async) is non-trivial: the closure captures `Rc<RefCell<Backend>>` and `Node` (a `JsValue`), so dropping 10k of them runs 10k JS-side deref calls via wasm-bindgen.

**The fix:** **Skip the Effect for variant-only styles with no overrides.** 
- If the style closure never reads a signal (common case: `PerfRow().parity(parity_var)` where parity is computed once at row creation), the style is *static per row* and never needs re-evaluation.
- The macro could detect this and emit a direct `apply_style()` call instead of wrapping in an Effect.
- For reactive styles (closure reads theme or variant signal), keep the Effect.

**Effort:** Medium.
1. Macro: detect whether the style source reads signals. (Heuristic: does the closure body contain `.get()`? Same as for reactive text.)
2. Lowering: if static, emit `node.apply_style(...)` directly instead of `attach_style(...)`.
3. Walker: if no style is attached as Effect, skip the whole mechanism.

**Risk/Scope:** Medium. The heuristic is the same as for reactive text (already used in `emit_text`). False negatives (thinking a style is static when it's reactive) would cause missed updates; false positives (wrapping a static style in an Effect) just waste a slot.

**Expected gain:** For the perf screen (static row styles, reactive only to theme changes): 10–30ms per mount (10k rows × 3µs/effect), plus ~20ms teardown. Theme changes would still be slow (10k effects re-fire), but that's separate.

---

### D. Stylesheet Resolution Path Redundancy

**Where:** `attach_style()` calls `resolve_style()`, `ensure_registered_with()`, then `apply_styled_states()`

**Cost:** Per row:
- `style()` closure call – 0.2µs (returns a `StyleApplication`)
- `resolve_style()` – calls stylesheet.resolve() → variant resolution → BTreeMap lookups – ~1.0µs
- Cache lookup in `pregen_by_ptr` or `pregen` – 0.3µs
- `ensure_registered_with()` – RefCell borrow, closure calls, but short – 0.1µs (cache hit case)
- `apply_styled_states()` – FFI + backend HashMap updates – 2.5µs

For 10k identical-parity rows, *all 5000 even rows resolve to the same `Rc<StyleRules>`*. Yet we still call `resolve_style()` 5000 times, walk the variant BTreeMap 5000 times, and do 5000 `pregen_by_ptr` lookups (though those are O(1) hits).

**Why it costs:** The style closure is called per row per effect run. Each call is tiny (1–2µs), but 10k × 1µs = 10ms. Bigger picture: `resolve_style()` calls `stylesheet.resolve()`, which iterates the variants BTreeMap and applies each overlay. For a style with 5 variant axes, that's 5 BTreeMap lookups + merges per row. 10k rows × 5 lookups = 50k map operations.

**The fix:** **Hoist variant resolution out of the per-row loop.**
- Detect at macro or render time that `PerfRow().parity(...)` produces a finite set of combinations (even/odd = 2).
- Pre-resolve all combinations at stylesheet registration time and cache them.
- At row-creation time, use the precomputed cache instead of re-resolving.

Implementation:
1. Add `stylesheet.pregenerate_variants(theme, axes)` method to enumerate all axis combinations and resolve once.
2. Cache the results in the stylesheet (already has a `variant_cache`).
3. In `attach_style()`, check if the variant set is in the cache before calling `resolve_style()`.

**Effort:** Small-to-Medium.
- The infrastructure is already there (`variant_cache` in StyleSheet).
- The challenge is detecting which variant axes are used; the macro would need to analyze the style builder calls.

**Risk/Scope:** Small. This is an optimization; if the cache is missed, we fall back to the normal path.

**Expected gain:** For the perf screen: 5000 rows × 1µs = 5ms, plus ~3ms in fewer BTreeMap lookups.

---

### E. Per-Node Backend Bookkeeping (HashMap Keyed by Raw Pointer)

**Where:** `WebBackend::node_id()`, `node_ids: HashMap<*const Node, u32>`, `dynamic: HashMap<u32, DynamicSlot>`

**Cost:**
- Per row: `node_id(node)` does a HashMap insert (node-to-id mapping). For 10k rows, 10k inserts, each ~0.5µs in the happy path (no collision). = 5ms.
- On drop: `on_node_unstyled()` looks up the node in `node_ids` to get its id, then clears the dynamic slot. 10k lookups × 0.5µs = 5ms.
- Total: ~10ms per mount/unmount.

**Why it costs:** The `node_ids` HashMap is keyed by raw `*const web_sys::Node` pointer. Raw pointers don't move, so the map is stable. But HashMap operations (hash, probe, insert) are non-trivial even with cheap keys.

**The fix:** **Use a dataset attribute on the DOM node instead.**
```rust
// At create time:
let node_id = self.next_node_id;
self.next_node_id += 1;
node.set_attribute("data-node-id", &node_id.to_string());
// Later, to look up:
if let Ok(attr) = node.get_attribute("data-node-id") {
    if let Ok(id) = attr.parse::<u32>() {
        // Use id
    }
}
```
This replaces HashMap insert/lookup with DOM attribute operations. Attribute get/set is typically faster than HashMap operations, and avoids the bookkeeping map entirely.

**Effort:** Small.
- Change `node_id()` to write the attribute instead of the map.
- Change lookups in `on_node_unstyled()` and other sites to read the attribute.
- Remove the `node_ids` HashMap entirely (save memory + one RefCell borrow per row).

**Risk/Scope:** Small. The attribute is internal (prefixed with `data-`) and hidden from authors. One gotcha: if a row is duplicated via `cloneNode`, the id attribute is cloned too, and two nodes share the same id. But that's actually fine if cloneNode is used *at creation time* and the clones are immediately inserted (no id collision window). If cloneNode is used for templating (pre-create a template, clone N times), we'd need to re-stamp the id on each clone.

**Expected gain:** 10ms per mount/unmount (5ms insert, 5ms cleanup). Bigger win from reduced memory usage (HashMap dropped) and cache-line pressure.

---

### F. Reactivity Granularity: Theme-Signal Subscription Model

**Where:** `attach_style()` creates an Effect per styled node; theme changes re-fire 10k effects

**Cost:** When theme changes:
- 10k effects in the active scope all re-fire (they subscribed to the theme signal by calling it inside the closure).
- Each re-run does: `resolve_style()`, `apply_styled_states()`, FFI to set className.
- Total: 10k × (1µs resolve + 2.5µs apply + FFI) = 35ms synchronous.
- Plus teardown of 10k old closures (async) = ~20ms more.

**Why it costs:** The theme signal is a global (set via `set_theme()`), and every styled node's effect closure calls `active_theme()` or reads the theme inside the style closure. So every effect subscribes. When the theme changes, the framework re-fires every one.

**The fix:** **Single "theme reactor" for bulk re-application.**
- Instead of 10k independent effects, have one global effect that monitors theme changes and bulk-applies styles to all active nodes.
- Cache the set of active nodes (e.g., in a `Vec` or `linked_list`; add on mount, remove on unmount).
- On theme change, walk the active nodes and re-apply styles in a tight loop, avoiding the Effect machinery entirely.

**Effort:** Large.
- Requires refactoring the node-tracking layer (currently per-node Effects, would become a global registry).
- Need to manage the active-node list carefully (insert/remove on mount/unmount, no double-register).
- Fallback: keep the current per-node Effect model for non-theme styles.

**Risk/Scope:** High complexity. The global reactor would bypass the reactive system's subscription model, which could cause subtle bugs if a node's dependencies change during iteration. But for a read-only operation (bulk apply), it should be safe.

**Expected gain:** For the perf screen, theme changes go from 35ms (10k effects) to ~5–10ms (bulk loop), especially if using cloneNode for DOM mutations. But this is a separate concern from initial mount time.

---

### G. Macro-Level Inefficiency: `for` Loop Lowering

**Where:** `emit_for()` in `framework-macros/src/ui.rs`

**Current lowering:**
```rust
{
    let mut __c = Vec::new();
    for i in 0..n {
        let prim = /* build one row */;
        ChildList::append_to(prim, &mut __c);
    }
    __c  // return Vec<Primitive> to be wrapped in View
}
```

**Cost:** Each iteration constructs a `Primitive::View`, which is immediately walked (converted to a backend node), then the Primitive is dropped. The Primitive itself takes 1–2µs to allocate, plus ~0.5µs of GC pressure. For 10k rows, that's ~15ms of wasted allocation work.

**The fix:** No direct fix at the macro level (the Primitive allocation is necessary because we need to build the tree). The real savings come from:
1. Detecting uniform rows and using cloneNode (gains 30–50%).
2. Skipping Effect allocation for static styles (gains 10–20%).
3. Batching FFI calls (gains 20–30%).

Combining these would address the 10k-row case far better than optimizing the Primitive allocation itself.

**Effort:** N/A (already noted in prior sections).

**Expected gain:** 0ms directly; 30–50ms when combined with other optimizations.

---

### H. Allocation Count Per Row (Box, Vec, String, HashSet)

**Where:** Throughout `build()`, `attach_style()`, effect closures

**Per row:**
- `Primitive::View { children: Vec<_>, ... }` – 1 Vec alloc + 1 Primitive Box (or stack)
- `Primitive::Text { ... }` – 1 Box for TextSource::Reactive closure (if reactive)
- Effect closure – 1 Box<dyn FnMut()> for the Effect's `run` field
- `StyleHandle` – 1 struct (Node + Rc clone + backend Rc clone) = 3 allocations per Effect
- Scope registration – 1 Vec push (inside scope.effects)
- Optional `Signal<StateBits>` – 1 Box + 1 Signal struct

**Total per row:** ~6–10 allocations. At 10k rows, that's 60k–100k allocations total. Malloc overhead (10–50 bytes per allocation header) adds up; with freelists in the Arena, we're recycling slots, but Rc clones and Vec allocations are fresh each time.

**The fix:** 
- Use an `ArrayVec<[Primitive; 2]>` for the children Vec when the count is known small (very invasive).
- Inline the Effect's `run` closure instead of Boxing (requires generic Effect<F> type, breaking change).
- Skip Signal<StateBits> for web backend (already done in attach_style; gain ~0.3µs per row).

**Effort:** Medium-to-Large. Requires rework of the Effect type system.

**Expected gain:** 5–10ms total (allocation + GC pressure).

---

### I. Closure Capture Set Size

**Where:** Every Effect closure in `attach_style()`, `attach_disabled()`, image/video/slider reactive effects

**Per styled row effect:**
```rust
Effect::new(move || {
    let app = style();  // captures: StyleSource (Box<dyn Fn>)
    // ... resolves ... applies ...
    // captures: backend (Rc<RefCell<B>>), node (Node.clone()), handle (StyleHandle)
})
```

**Total captures per Effect:** ~3–4 Rc/Box allocations + the closures/data themselves. Dropping the Effect requires dropping each capture; for 10k effects, that's 30k–40k Rc decrefs (async), each triggering JS-side deref calls for `Node`.

**Why it costs:** When the scope drops (end of render, theme change, etc.), every Effect closure is dropped. The drop path calls `__wbindgen_destroy_closure` for wasm-bindgen managed objects, which involves JS→Rust→JS roundtrips.

**The fix:**
- Inline captures where possible. Instead of capturing `Rc<RefCell<Backend>>`, capture a reference to a shared backend handle.
- De-duplicate captures across Effects (e.g., all Effects in a row share the same backend handle; could be refactored into a "row context" struct).
- For the Node, use an `Rc` so all Effects share the same decref cost (already done via `node.clone()`).

**Effort:** Medium.

**Expected gain:** 5–10ms (fewer Rc clones per row, faster teardown).

---

### J. Content-Key Generation (StyleRules::content_key())

**Where:** `apply_styled_states()` calls `base.content_key()`, `hash_class_name()`

**Cost:** `content_key()` walks every field of `StyleRules` (60+ fields) and writes hex representations to a String. For a 200-byte style, this can produce a 300–400-byte key string. Per row, that's 1–2µs + allocation. For 10k rows, ~10–20ms.

**Why it costs:** The key is used for two purposes:
1. `pregen` HashMap lookup (content-keyed cache).
2. Generating the class name via `hash_class_name()`.

For pre-generated styles (common case), we hit the `pregen_by_ptr` fast path (pointer-keyed) and skip `content_key()`. But if the style is *dynamic* (overridden at row level) or doesn't match the cache, we re-compute the key.

**The fix:**
- Compute content_key once at stylesheet registration time (during `ensure_registered_with`), not per-apply.
- Cache the key on the `Rc<StyleRules>` (behind a `RefCell` or lazy-cell) so subsequent applies are free.

**Effort:** Small.

**Risk/Scope:** Small. The key is deterministic; caching is safe.

**Expected gain:** 5–10ms (avoid 10k string allocations).

---

### K. Other Surprises

1. **State-axis caching in `state_axes()`:** Already well-optimized. For 10k rows with no `state` blocks, we skip resolution entirely. Good.

2. **Variant cache population:** The per-sheet `variant_cache` is populated at registration time but keyed by `(theme_ptr, VariantSet)`. For variant-only styles, the lookup is O(1) and hits; the win is already there.

3. **`with_scope` overhead:** Each scope push/pop does a thread-local RefCell borrow + Vec push/pop. For a flat structure (no nested when/switch), this is N pushes + 1 pop; for nested structures, it's more. At 10k rows with one global scope, the cost is negligible (~1µs total).

4. **Arena slot allocation:** Already optimized with freelists. Slot allocation is O(1) on the freelist path (pop + reuse).

5. **Floating-point math in slider step snapping:** `(v - min) / step` done per slider per on_change event. Not per-mount, so not in scope.

---

## Ranked Recommendations

### 1. **Batch FFI via cloneNode + DocumentFragment** (Medium effort, 20–40ms saved)

**Reasoning:** The single largest chunk of per-row cost is DOM node creation (create_view + create_text + insert = 58ms for 10k rows). Modern browsers' `cloneNode` and `DocumentFragment` can reduce this by 50% if rows are structurally identical. This is the highest ROI per effort.

**Action:**
1. Detect uniform row shapes at macro compile time (or at registration time).
2. Create a `<template>` with the row structure once.
3. Use `cloneNode` to duplicate the template N times.
4. Batch insertions via DocumentFragment.
5. Fallback for non-uniform rows is standard creation.

**Implementation order:** High priority. Do this first; it's orthogonal to other changes and provides immediate gains.

---

### 2. **Skip Effect for variant-only static styles** (Small-to-Medium effort, 10–30ms saved)

**Reasoning:** Every styled row allocates an Effect (10k effects, 10k arena slots, 10k closures), even if the style never changes at runtime. For the common case (PerfRow with static parity, reactive only to theme), this is wasted overhead. Detecting and eliminating it saves effect allocation, arena registration, and teardown cost.

**Action:**
1. Macro detects whether the style closure reads signals (heuristic: `.get()` in the closure body).
2. If static, emit `apply_style()` directly instead of `attach_style()`.
3. For reactive styles, keep the Effect.

**Implementation order:** Second. Builds on macro analysis; compatible with cloneNode changes. Gains ~10ms immediately, plus saves memory.

---

### 3. **Hoist variant resolution into stylesheet registration** (Small effort, 5–10ms saved)

**Reasoning:** For styles with finite variant combinations (parity = even/odd), we resolve the same styles 5000 times. Pre-computing all combinations at registration time and caching them avoids repeated resolution work.

**Action:**
1. Add `stylesheet.pregenerate_variants(theme, axes)` to pre-resolve all axis combinations.
2. At apply time, check the pre-computed cache before calling `resolve_style()`.
3. Fallback to runtime resolution for dynamic axes.

**Implementation order:** Third. Can be done independently; complementary to static-style detection.

---

### 4. **Replace node-id HashMap with DOM dataset attributes** (Small effort, 5–10ms saved)

**Reasoning:** The `node_ids: HashMap<*const Node, u32>` is a bookkeeping layer that adds insert/lookup overhead. DOM attributes are faster and eliminate the map entirely.

**Action:**
1. On node creation, write `data-node-id` attribute.
2. On lookup, read the attribute instead of HashMap.get().
3. Remove `node_ids` HashMap.

**Implementation order:** Fourth. Low risk, straightforward change. Saves bookkeeping overhead + memory.

---

### 5. **Bulk theme-change handler (for theme switching scenarios)** (Large effort, 25–35ms saved on theme changes, not initial mount)

**Reasoning:** When theme changes, 10k effects re-fire, re-resolving and re-applying styles. A single theme reactor would bulk-apply without the Effect machinery. This is a separate concern from mount time, but valuable for interactive scenarios.

**Action:**
1. Add a global "theme changed" event that triggers once when `set_theme()` is called.
2. Iterate active styled nodes and re-apply styles in a tight loop.
3. Requires a node registry (list of active nodes); add/remove on mount/unmount.

**Implementation order:** Fifth, if at all. This is deferred because it's complex and targets a different scenario (theme changes, not initial mount). The initial mount is already significantly improved by recommendations 1–4.

---

## Summary

The framework's core design is sound. The main inefficiencies are:
- **Redundant FFI calls** (cloneNode + DocumentFragment would halve this).
- **Per-node Effects for static styles** (should skip Effect if style closure is reactive-free).
- **Repeated style resolution** (pre-compute variant combinations at registration).
- **HashMap bookkeeping** (DOM attributes are faster).

Implementing recommendations 1–4 (in order) would bring the 10,000-row mount time from ~120ms to ~70–80ms, a 30–40% improvement. Recommendation 5 would benefit theme-change scenarios (a separate measurement, not part of the initial mount benchmark).

The fixes are incremental and don't require architectural overhaul. The Primitive-tree intermediate is not a bottleneck (allocation is cheap relative to FFI); the real cost is DOM mutation and Effect machinery.
