// Local-render batch executor for `Backend::execute_batch` on the
// web backend. Decodes a flat `Uint32Array` of op records in a tight
// JS loop. Sets `window.__idealystExecuteBatch` to a function the
// Rust side picks up via `js_sys::Reflect::get`.
//
// Encoding (4 u32s per op, packed back-to-back):
//
//   [kind, arg0, arg1, arg2]
//
//   kind=0 CreateView         [0, local_id, 0, 0]
//   kind=1 CreateText         [1, local_id, 0, string_idx]
//   kind=2 ApplyStyleStatic   [2, node_id,  0, string_idx]
//   kind=3 Insert             [3, parent,   child, 0]
//
// String payloads (CreateText content, ApplyStyleStatic class name)
// arrive in a single concatenated `stringsJoined` argument with a
// NUL (`\0`) separator. We split once at the top of the call. The
// `string_idx` field is an index into the resulting array.
//
// Return: a flat array of `nodeCount` DOM nodes, indexed by
// `local_id`. The Rust side reads it via `Array::get(i)` per slot.
(function () {
  if (window.__idealystExecuteBatch) return;
  // Diagnostic counters. Read via `window.__idealystBatchStats`.
  window.__idealystBatchStats = { calls: 0, totalOps: 0, lastOps: 0, lastNodes: 0 };
  console.log("[idealyst] batch.js shim injected, __idealystExecuteBatch ready");
  // Signature:
  //   __idealystExecuteBatch(u32Buf, stringsJoined, nodeCount)
  //   __idealystExecuteBatch(u32Buf, stringsJoined, nodeCount,
  //                          attachParent, attachLocalsBuf)
  //
  // The 5-arg form fires when Rust opts into the combined
  // execute-and-attach fast path: `attachParent` is the surrounding
  // parent Node, `attachLocalsBuf` is a `Uint32Array` of `local_id`s
  // (typically the row tops) to append in order. Doing the attach
  // here saves N `appendChild` FFI hops vs the equivalent
  // `insert_many` follow-up call from Rust — measured at ~60 ms
  // savings at 100 k rows in the rebuild bench.
  window.__idealystExecuteBatch = function (u32Buf, stringsJoined, nodeCount, attachParent, attachLocalsBuf) {
    var ops = (u32Buf.length / 4) | 0;
    window.__idealystBatchStats.calls += 1;
    window.__idealystBatchStats.totalOps += ops;
    window.__idealystBatchStats.lastOps = ops;
    window.__idealystBatchStats.lastNodes = nodeCount;
    console.log("[idealyst] execute_batch firing — ops:", ops, "nodes:", nodeCount);

    // Pre-split the strings once. `split` on a typical batch (hundreds
    // of strings, tens of KB total) is fast in V8 — way cheaper than
    // calling JS via wasm-bindgen N times to read individual strings.
    var strings = stringsJoined === "" ? [] : stringsJoined.split("\0");

    // Pre-allocate the node slots so we index by `local_id` without
    // push-bookkeeping. `nodeCount` is the high-water mark of slot ids
    // the batch ever references (CreateView/CreateText assign them).
    var nodes = new Array(nodeCount);
    var doc = document;

    // Tight loop. `u32Buf[off + k]` reads are direct typed-array
    // accesses — no boundary crossing, no allocation. We don't use a
    // switch on `kind` because the if/else chain JITs better in V8
    // for this op count.
    for (var i = 0; i < ops; i++) {
      var off = i * 4;
      var kind = u32Buf[off];
      var a0 = u32Buf[off + 1];
      var a1 = u32Buf[off + 2];
      var a2 = u32Buf[off + 3];
      if (kind === 0) {
        // CreateView
        nodes[a0] = doc.createElement("div");
      } else if (kind === 1) {
        // CreateText — a2 is the string-table index of `content`.
        nodes[a0] = doc.createTextNode(strings[a2]);
      } else if (kind === 2) {
        // ApplyStyleStatic — a2 is the string-table index of the
        // class name. `nodes[a0]` is the View whose className we set.
        nodes[a0].className = strings[a2];
      } else if (kind === 3) {
        // Insert — a0 = parent local_id, a1 = child local_id.
        nodes[a0].appendChild(nodes[a1]);
      }
    }

    // Combined attach path. When the caller passed a parent + locals
    // buffer, bulk-parent the row tops here. Using a
    // `DocumentFragment` for the multi-child case so the parent sees
    // exactly one mutation regardless of N (matches what the
    // Rust-side `insert_many` used to do, but with the per-child
    // `appendChild` calls living JS-side).
    if (attachParent != null && attachLocalsBuf != null) {
      var nLocals = attachLocalsBuf.length;
      if (nLocals === 1) {
        attachParent.appendChild(nodes[attachLocalsBuf[0]]);
      } else if (nLocals > 1) {
        var frag = doc.createDocumentFragment();
        for (var k = 0; k < nLocals; k++) {
          frag.appendChild(nodes[attachLocalsBuf[k]]);
        }
        attachParent.appendChild(frag);
      }
    }

    return nodes;
  };
})();
