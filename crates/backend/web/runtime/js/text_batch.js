// Batched text-update path for the web backend.
//
// At create time, each reactive text node is registered in a JS-side
// `Array<Node>` indexed by a monotonic `u32` id assigned by the Rust
// side. At update time, the Rust side collects pending updates into
// (Uint32Array of ids, NUL-joined contents) and ships them in one
// FFI call to `__idealystUpdateTextBatch`. The shim splits the
// strings once and walks the ids in a tight JS loop, setting
// `textContent` per node.
//
// This collapses the per-fire FFI cost from O(N) wasm→JS hops to
// O(1) — at a 2000-leaf hierarchy fan-out, that's ~6-10 ms of
// `set_text_content` marshalling reduced to ~0.5 ms of shim work.
//
// Stability:
//   - Ids are NEVER reused. The registry array's slots become `null`
//     when a node is released, never re-assigned. Slot reuse would
//     race against a stale `update_text_by_id` (queued before the
//     release but flushed after).
//   - `__idealystReleaseText` clears a single slot on Effect/Scope
//     teardown. The Rust side calls it via the same batched flush
//     so the unregister cost is also O(1).
//   - `__idealystRegisterText` is a single-pair register; the
//     create-time path doesn't batch (creates are spread across the
//     walker's recursion, no natural batch point). Each create
//     costs one extra FFI hop over the unbatched path.
(function () {
  if (window.__idealystTextRegistry) return;
  window.__idealystTextRegistry = [];
  // Diagnostic counters (read via `window.__idealystTextBatchStats`).
  window.__idealystTextBatchStats = {
    registered: 0,
    released: 0,
    flushes: 0,
    totalUpdates: 0,
    lastUpdates: 0,
  };
  console.log("[idealyst] text_batch.js shim injected, __idealystUpdateTextBatch ready");

  window.__idealystRegisterText = function (id, node) {
    window.__idealystTextRegistry[id] = node;
    window.__idealystTextBatchStats.registered += 1;
  };

  window.__idealystReleaseText = function (id) {
    window.__idealystTextRegistry[id] = null;
    window.__idealystTextBatchStats.released += 1;
  };

  // Batched release: called at scope teardown when many text
  // effects unmount together (e.g. 2k+ hierarchy leaves on a
  // switch-arm flip). Collapses what would be N per-id FFI calls
  // into one. Same Uint32Array shape as the update path.
  window.__idealystReleaseTextBatch = function (idsU32) {
    var reg = window.__idealystTextRegistry;
    var n = idsU32.length;
    for (var i = 0; i < n; i++) {
      reg[idsU32[i]] = null;
    }
    window.__idealystTextBatchStats.released += n;
  };

  window.__idealystUpdateTextBatch = function (idsU32, lengthsU32, bigString) {
    var stats = window.__idealystTextBatchStats;
    stats.flushes += 1;
    var n = idsU32.length;
    stats.lastUpdates = n;
    stats.totalUpdates += n;
    if (n === 0) return;

    // Length-prefixed walk. Replaces the prior NUL-joined design's
    // `bigString.split("\0")` (which would allocate `n` JS String
    // objects up front and burn ~2-5 ms per flush at 20 k segments).
    // `substring` is O(1) in V8 (creates a SlicedString view), so
    // we do the same total work distributed across the per-segment
    // loop with no upfront allocation.
    //
    // Registry holds Text nodes (created via `create_text_node` on
    // the Rust side, wrapped in a `<span>` for styling). Setting
    // `.nodeValue` on a Text node is an O(1) string-slot
    // assignment.
    var reg = window.__idealystTextRegistry;
    var offset = 0;
    for (var i = 0; i < n; i++) {
      var len = lengthsU32[i];
      var node = reg[idsU32[i]];
      if (node !== null && node !== undefined) {
        node.nodeValue = bigString.substring(offset, offset + len);
      }
      offset += len;
    }
  };
})();
