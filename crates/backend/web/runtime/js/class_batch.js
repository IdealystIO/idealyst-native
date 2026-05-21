// JS-side batched `class` attribute updater.
//
// Companion to the Rust web-backend's style apply path. Every
// `apply_style` / `apply_styled_states` call would normally
// `element.setAttribute("class", className)` from wasm via
// wasm-bindgen — one wasm→JS boundary crossing per node. At
// reactive-style scale (one signal fanning out to N nodes), that
// became N FFI hops per signal write, ~60 ms of pure boundary
// crossing for N=100k.
//
// This shim moves the fan-out entirely to JS:
//
//   1. On first apply of a node, the Rust side registers the node
//      with `__idealystRegisterStyledNode(id, element)` — ONE FFI
//      per node, once per its lifetime.
//   2. Subsequent applies push `(id, class_name)` into a Rust-side
//      buffer; on microtask flush, ONE FFI ships the whole batch to
//      `__idealystApplyClassesBatch(ids, classesJoined, lengths)`.
//   3. JS walks the buffer and calls `node.setAttribute('class',
//      className)` for each entry from inside this function — no
//      wasm boundary crossing for the per-row work.
//
// Data model:
//   __idealystStyledNodes : Map<u32 → Element>
//
// Encoding for the apply batch:
//   ids       : Uint32Array of node ids (one per update)
//   classes   : single big string, all class names concatenated WITHOUT
//               separators. Walk per-entry length to slice.
//   lengths   : Uint32Array of per-entry utf-16 lengths (matches
//               `text_batch.js`'s length-prefixed shape).
//
// Why no NUL separator: class names never contain NUL, but slicing
// by length avoids a `split('\0')` walk on the receiver. `substring`
// is O(1) (SlicedString in V8).
(function () {
  if (window.__idealystStyledNodes) return;

  window.__idealystStyledNodes = new Map();

  // Diagnostic counters. Read via `window.__idealystClassBatchStats`.
  window.__idealystClassBatchStats = {
    registrations: 0,
    releases: 0,
    batchCalls: 0,
    totalUpdates: 0,
  };

  console.log('[idealyst] class_batch.js shim injected, __idealystApplyClassesBatch ready');

  // Called once per node when its style is first applied. Stashes
  // the Element handle so subsequent batched updates can find it by
  // id without crossing the wasm boundary.
  window.__idealystRegisterStyledNode = function (id, element) {
    window.__idealystClassBatchStats.registrations += 1;
    window.__idealystStyledNodes.set(id, element);
  };

  // Called when a styled node is unmounted. Drops the registry
  // entry so the Element can be GC'd. Safe to call defensively on
  // unknown ids.
  window.__idealystReleaseStyledNode = function (id) {
    if (window.__idealystStyledNodes.delete(id)) {
      window.__idealystClassBatchStats.releases += 1;
    }
  };

  // The batch dispatcher. Rust ships one buffer per microtask flush
  // covering every queued (node, class) pair. We walk the buffer in
  // pure JS and call setAttribute per entry — same DOM cost as
  // calling setAttribute from wasm, but with the FFI overhead
  // amortized across the whole batch.
  window.__idealystApplyClassesBatch = function (ids, classesJoined, lengths) {
    window.__idealystClassBatchStats.batchCalls += 1;
    var nodes = window.__idealystStyledNodes;
    var n = ids.length;
    window.__idealystClassBatchStats.totalUpdates += n;
    var offset = 0;
    for (var i = 0; i < n; i++) {
      var len = lengths[i];
      var cls = classesJoined.substring(offset, offset + len);
      offset += len;
      var el = nodes.get(ids[i]);
      // `el` may be undefined if the node was released between
      // queue and flush — just skip; the unregister path already
      // ensured the rule's refcount was dropped.
      if (el !== undefined) {
        el.setAttribute('class', cls);
      }
    }
  };

  // Standalone smoke test. Call from devtools to verify the shim
  // works end-to-end:
  //
  //   __idealystClassBatchSmokeTest()
  window.__idealystClassBatchSmokeTest = function () {
    var d1 = document.createElement('div');
    var d2 = document.createElement('div');
    window.__idealystRegisterStyledNode(9001, d1);
    window.__idealystRegisterStyledNode(9002, d2);
    var classes = 'aa' + 'bbb';   // 2 + 3 chars
    var lengths = new Uint32Array([2, 3]);
    var ids = new Uint32Array([9001, 9002]);
    window.__idealystApplyClassesBatch(ids, classes, lengths);
    var ok = d1.getAttribute('class') === 'aa' && d2.getAttribute('class') === 'bbb';
    window.__idealystReleaseStyledNode(9001);
    window.__idealystReleaseStyledNode(9002);
    console.log('[idealyst] class_batch smoke test:', ok ? 'PASS' : 'FAIL');
    return ok;
  };
})();
