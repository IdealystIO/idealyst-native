// JS-side `WeakMap<Node, u32>` for stable per-DOM-node ids.
//
// The web backend keys per-node state (dynamic CSS class slot,
// state listeners, animated state) by `u32`. The id has to be
// derived from the DOM element itself, NOT the Rust `web_sys::Node`
// wrapper pointer — multiple Rust wrappers can refer to the same
// JS object, and earlier code that keyed by `*const Node` saw
// different ids for the "same" element when the framework cloned
// a Node into a `Ref<ViewHandle>` and a separate apply path saw a
// fresh wrapper.
//
// This shim resolves any number of Rust wrappers around the same
// JS object to the same `u32`:
//
//   __idealystNodeId(node) → u32   // allocate on first sight, recall thereafter
//
// Storage is a `WeakMap` so entries auto-clear when the DOM element
// is garbage-collected. No explicit unregister is required; the
// Rust-side cache (`HashMap<*const Node, u32>`) is what
// `impl_on_node_unstyled` drops to release per-id state.
//
// The id space is JS-side; Rust treats them as opaque values.
(function () {
  if (window.__idealystNodeId) return;
  console.log("[idealyst] node_ids.js shim injected");
  var map = new WeakMap();
  var next = 1;
  window.__idealystNodeId = function (node) {
    var id = map.get(node);
    if (id === undefined) {
      id = next;
      next = (next + 1) >>> 0;
      map.set(node, id);
    }
    return id;
  };
  // Diagnostic: how many distinct DOM nodes have we minted ids for?
  // Backed by `next - 1` rather than walking the map (WeakMap has no
  // size accessor) — this is the high-water mark, not the live count,
  // but matches what the old Rust-side `next_node_id` counter
  // reported.
  window.__idealystNodeIdHighWater = function () {
    return next - 1;
  };
})();
