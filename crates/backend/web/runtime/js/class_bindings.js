// JS-side reactive class-binding layer.
//
// Companion to the Rust framework's `StyleSource::SignalClass`. Rust
// keeps signals as the source of truth, but for *structured*
// class bindings (one signal → discrete u32 value → pre-resolved
// class name) the per-fire fan-out moves entirely to JS — no
// per-node wasm boundary crossing, no per-node Rust Effect.
//
// Mirrors `text_bindings.js` but for `setAttribute('class', …)`
// instead of `nodeValue = …`. Like text bindings, each binding
// subscribes a node to one signal at mount; the existing
// signal-change dispatcher (`__idealystOnSignalChanged`) routes
// fires to both text AND class subscribers in one pass.
//
// ## Data model
//
//   __idealystClassBindings           : Map<bindingId → spec>
//   __idealystClassSignalSubscribers  : Map<signalId → Set<bindingId>>
//
// A spec is: `{ nodeId, signalId, valueToClass: Map<value → className> }`.
// On signal change the dispatcher looks up subscribers, fetches each
// node from `__idealystStyledNodes` (same registry the class-apply
// batch uses), and `setAttribute('class', class)`s the resolved class.
//
// ## Wire encoding (RegisterClassBinding)
//
// Rust ships at mount:
//   - bindingId : u32        — stable id; used to release later
//   - nodeId    : u32        — addresses `__idealystStyledNodes`
//   - signalId  : u64 (as two u32 lo/hi)
//   - values    : Uint32Array — discrete signal values, in declared order
//   - classes   : string      — class names concatenated, length-prefixed
//   - lengths   : Uint32Array — utf-16 length per class
//
// Each `values[i]` maps to `classes[lengths[0..i].sum()..lengths[0..i+1].sum()]`.
//
// ## Initial paint
//
// At register time, the spec looks up the signal's current value in
// `__idealystSignalValues` (populated by the text-bindings layer or
// by a fresh write) and applies the matching class once. Without
// this, the node would paint with no class until the first signal
// change.

(function () {
  if (window.__idealystClassBindings) return;

  // Diagnostic counters. Read via `window.__idealystClassBindingStats`.
  window.__idealystClassBindingStats = {
    registrations: 0,
    releases: 0,
    signalNotifications: 0,
    bindingsUpdated: 0,
  };

  window.__idealystClassBindings = new Map();
  window.__idealystClassSignalSubscribers = new Map();

  console.log('[idealyst] class_bindings.js shim injected, __idealystRegisterClassBinding ready');

  // Apply the binding's current class to its node. Looks up the
  // signal's cached value, finds the matching class, and writes the
  // class attribute. Used at registration time AND from the
  // signal-changed tap below.
  function applyBindingClass(binding) {
    var values = window.__idealystSignalValues;
    var rawValue = values.get(binding.signalIdLo);
    // Signal values were originally stored as strings (text bindings
    // use string interpolation). We coerce to a number here so the
    // class binding's Map lookup works.
    var v = (typeof rawValue === 'string') ? Number(rawValue) : rawValue;
    var className = binding.valueToClass.get(v);
    if (className === undefined) return;   // unknown value; leave class alone
    var node = window.__idealystStyledNodes.get(binding.nodeId);
    if (node !== undefined) {
      node.setAttribute('class', className);
    }
  }

  // Args layout — Rust packs the four small u32 args into a single
  // header `Uint32Array` so the FFI hop carries 4 JsValues, not 7.
  //   header[0] = bindingId
  //   header[1] = nodeId
  //   header[2] = signalIdLo (only meaningful half today)
  //   header[3] = signalIdHi (reserved for >32-bit ids)
  window.__idealystRegisterClassBinding = function (
    headerU32,
    valuesU32,
    classesJoined,
    lengthsU32,
  ) {
    window.__idealystClassBindingStats.registrations += 1;

    var bindingId = headerU32[0];
    var nodeId = headerU32[1];
    var signalIdLo = headerU32[2];
    // headerU32[3] (signalIdHi) reserved.

    // Slice classes by length into a value→class map.
    var valueToClass = new Map();
    var offset = 0;
    for (var i = 0; i < valuesU32.length; i++) {
      var len = lengthsU32[i];
      valueToClass.set(valuesU32[i], classesJoined.substring(offset, offset + len));
      offset += len;
    }

    var binding = {
      nodeId: nodeId,
      signalIdLo: signalIdLo,
      valueToClass: valueToClass,
    };
    window.__idealystClassBindings.set(bindingId, binding);

    // Subscribe.
    var subs = window.__idealystClassSignalSubscribers;
    var set = subs.get(signalIdLo);
    if (!set) {
      set = new Set();
      subs.set(signalIdLo, set);
    }
    set.add(bindingId);

    // Initial paint.
    applyBindingClass(binding);
  };

  // Per-id release. Used by the batched release path.
  function releaseOne(bindingId) {
    var binding = window.__idealystClassBindings.get(bindingId);
    if (!binding) return;
    var subs = window.__idealystClassSignalSubscribers;
    var set = subs.get(binding.signalIdLo);
    if (set) {
      set.delete(bindingId);
      if (set.size === 0) subs.delete(binding.signalIdLo);
    }
    window.__idealystClassBindings.delete(bindingId);
    window.__idealystClassBindingStats.releases += 1;
  }

  // Batched release: one FFI call carries N ids. Matches the
  // `__idealystReleaseStyledNodesBatch` shape — same `IdBatch`
  // helper on the Rust side ships to either.
  window.__idealystReleaseClassBindingsBatch = function (idsU32) {
    for (var i = 0; i < idsU32.length; i++) releaseOne(idsU32[i]);
  };

  // Tap into the existing signal-changed dispatcher. text_bindings.js
  // owns `__idealystOnSignalChanged` and updates text nodes; we
  // add a hook that ALSO updates class bindings subscribed to the
  // same signal. The original handler stays in charge of text;
  // we wrap it so both fire on every signal change.
  //
  // The tap is idempotent — if class_bindings.js is loaded before
  // text_bindings.js (unusual but possible), the wrap is deferred
  // until the original exists. In practice both shims are
  // pre-injected together by `install_text_batcher`, so this just
  // chains.
  var originalOnSignalChanged = window.__idealystOnSignalChanged;
  window.__idealystOnSignalChanged = function (signalId, newValue) {
    if (originalOnSignalChanged) originalOnSignalChanged(signalId, newValue);
    window.__idealystClassBindingStats.signalNotifications += 1;
    var set = window.__idealystClassSignalSubscribers.get(signalId);
    if (!set) return;
    // Coerce the value once per signal change; binding's Map lookup
    // wants a number. Cached signal values use whatever type the
    // dispatcher writes — text bindings write strings (interpolation-
    // ready), so coerce defensively.
    var coerced = (typeof newValue === 'string') ? Number(newValue) : newValue;
    var bindings = window.__idealystClassBindings;
    var nodes = window.__idealystStyledNodes;
    var updated = 0;
    set.forEach(function (bid) {
      var binding = bindings.get(bid);
      if (!binding) return;
      var className = binding.valueToClass.get(coerced);
      if (className === undefined) return;
      var node = nodes.get(binding.nodeId);
      if (node !== undefined) {
        node.setAttribute('class', className);
        updated += 1;
      }
    });
    window.__idealystClassBindingStats.bindingsUpdated += updated;
  };
})();
