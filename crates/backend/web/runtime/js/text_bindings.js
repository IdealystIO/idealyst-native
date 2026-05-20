// JS-side reactive text binding layer.
//
// Companion to the Rust framework's text-binding system. The Rust
// side keeps signals as the source of truth, but for *structured*
// text bindings (the ones created via the `text_fmt!` macro) the
// per-fire fan-out moves entirely to JS — no per-leaf wasm boundary
// crossing, no per-leaf Rust Effect bookkeeping.
//
// Compare to the older `text_batch.js` flow, which still ran a Rust
// Effect per leaf and just batched the resulting `nodeValue` updates.
// At 20 k leaves, that path bottlenecked on Rust Effect bookkeeping +
// `format!()` String allocation. This shim eliminates both by
// putting binding execution in pure JS that V8 inline-caches.
//
// ## Data model
//
//   signalValues       : Map<signal_id (u32) → cached value>
//   bindings           : Map<binding_id (u32) → {node, signalIds, parts}>
//   signalSubscribers  : Map<signal_id → Set<binding_id>>
//
// A "binding" is one reactive text node + its signal dependencies +
// its format template's static parts. When any subscribed signal
// changes, the dispatcher updates every dependent binding by
// interpolating cached signal values into the template's parts and
// writing the result to `node.nodeValue`.
//
// ## Template encoding
//
// Templates ship as N+1 NUL-separated string parts for N signal
// slots. For `"leaf {}: g={} b={}"` with three slots:
//
//   parts = ["leaf ", ": g=", " b=", ""]
//   signalIds = [s0, s1, s2]   // the three signals interpolated, in order
//
// Captured values (non-signal scalars baked into the closure at
// create time) are folded into the parts on the Rust side BEFORE
// shipping — so e.g. `text_fmt!("leaf {}: g={}", leaf_id, global)`
// with leaf_id=42 produces `parts = ["leaf 42: g=", ""]` and only
// `[global.id()]` as signal deps. JS doesn't need to know that
// `leaf_id` ever existed.
//
// ## Why a Map (not an object) for the registries
//
// Numeric keys + frequent insert/delete (mount/unmount cycles) hit
// V8's "dictionary mode" on plain objects, which is slow. `Map`
// avoids that and gives predictable iteration order, which matters
// for repeatable measurement.
(function () {
  if (window.__idealystBindings) return;

  // Diagnostic counters. Read via `window.__idealystBindingStats`.
  window.__idealystBindingStats = {
    registrations: 0,
    releases: 0,
    signalNotifications: 0,
    bindingsUpdated: 0,
  };

  window.__idealystSignalValues = new Map();
  window.__idealystBindings = new Map();
  window.__idealystSignalSubscribers = new Map();

  console.log('[idealyst] text_bindings.js shim injected, __idealystRegisterBinding ready');

  // Compose a binding's current text by interpolating cached signal
  // values into its template parts. Single-pass string concat —
  // V8 specializes this aggressively at hot scale.
  function composeText(binding) {
    var parts = binding.parts;
    var signalIds = binding.signalIds;
    var n = signalIds.length;
    if (n === 0) {
      return parts[0];
    }
    var s = parts[0];
    var values = window.__idealystSignalValues;
    for (var i = 0; i < n; i++) {
      var v = values.get(signalIds[i]);
      // Coerce to string. `v` may be number, string, bool, etc. —
      // string concat (`+`) does the cast inline in V8's hot path
      // without a separate `String()` call.
      s += v;
      s += parts[i + 1];
    }
    return s;
  }

  // Register a new binding. Called by the Rust side once per
  // reactive text node at mount time.
  //
  // - `bindingId`        : u32 (matches the text-node registry id
  //                        the existing `text_batch.js` uses; releasing
  //                        a text node also releases its binding)
  // - `textNode`         : the Text DOM node (NOT the wrapping span —
  //                        same convention as the batched text path)
  // - `signalIdsU32`     : Uint32Array of signal ids this binding reads
  // - `templatePartsJoined` : the N+1 parts of the format template,
  //                        NUL-separated (same encoding as
  //                        `text_batch.js`)
  // - `initialValuesJoined` : the N initial values for this binding's
  //                        signals, NUL-separated. Used to seed
  //                        `signalValues` if not already cached AND
  //                        to compute the binding's initial nodeValue
  //                        synchronously inside this call (so no
  //                        empty-text flash on first paint).
  window.__idealystRegisterBinding = function (
    bindingId,
    textNode,
    signalIdsU32,
    templatePartsJoined,
    initialValuesJoined,
  ) {
    window.__idealystBindingStats.registrations += 1;

    var parts = templatePartsJoined === ''
      ? ['']
      : templatePartsJoined.split('\0');
    var initials = initialValuesJoined === ''
      ? []
      : initialValuesJoined.split('\0');
    // Materialize the typed array into a plain JS Array so we can
    // re-iterate it cheaply (Uint32Array indexing is fine but a
    // plain Array of numbers gives the V8 JIT a more stable shape).
    var signalIds = new Array(signalIdsU32.length);
    for (var i = 0; i < signalIdsU32.length; i++) {
      signalIds[i] = signalIdsU32[i];
    }

    var binding = { node: textNode, signalIds: signalIds, parts: parts };
    window.__idealystBindings.set(bindingId, binding);

    // Subscribe + seed signal values that we haven't seen yet.
    // (A signal value already known from a previous binding wins —
    // it's the more recent truth.)
    var values = window.__idealystSignalValues;
    var subs = window.__idealystSignalSubscribers;
    for (var j = 0; j < signalIds.length; j++) {
      var sid = signalIds[j];
      var set = subs.get(sid);
      if (!set) {
        set = new Set();
        subs.set(sid, set);
      }
      set.add(bindingId);
      if (!values.has(sid)) {
        // Initial values arrive as strings (Rust formatted them).
        // We don't try to coerce — anything we'd store gets
        // concatenated back into a string on the next compose.
        values.set(sid, initials[j] !== undefined ? initials[j] : '');
      }
    }

    // Initial paint: compute the binding's text from current cached
    // values and set the node. Without this, the node would render
    // empty until the next signal change.
    textNode.nodeValue = composeText(binding);
  };

  // Release a binding's subscriptions and remove it from the
  // registry. Called when a reactive text node is unmounted (the
  // existing `release_text_id` Rust path piggy-backs onto this).
  // No-op for unknown ids — safe to call defensively.
  window.__idealystReleaseBinding = function (bindingId) {
    var binding = window.__idealystBindings.get(bindingId);
    if (!binding) return;
    window.__idealystBindingStats.releases += 1;
    var subs = window.__idealystSignalSubscribers;
    for (var i = 0; i < binding.signalIds.length; i++) {
      var sid = binding.signalIds[i];
      var set = subs.get(sid);
      if (set) {
        set.delete(bindingId);
        if (set.size === 0) {
          subs.delete(sid);
          // Don't clear the signalValues entry — another binding
          // may register on the same signal later and the cached
          // value is still valid.
        }
      }
    }
    window.__idealystBindings.delete(bindingId);
  };

  // The dispatcher: Rust calls this when a tracked signal's value
  // changes. We update the cache, walk the subscribers, and rewrite
  // each one's nodeValue. Single FFI hop per signal change — the
  // per-binding fan-out happens entirely in JS.
  window.__idealystOnSignalChanged = function (signalId, newValue) {
    window.__idealystBindingStats.signalNotifications += 1;
    window.__idealystSignalValues.set(signalId, newValue);
    var set = window.__idealystSignalSubscribers.get(signalId);
    if (!set) return;
    var bindings = window.__idealystBindings;
    var updated = 0;
    set.forEach(function (bid) {
      var binding = bindings.get(bid);
      if (binding) {
        binding.node.nodeValue = composeText(binding);
        updated += 1;
      }
    });
    window.__idealystBindingStats.bindingsUpdated += updated;
  };

  // Standalone smoke test. Call from devtools console after
  // injection to verify the shim works end-to-end without any
  // framework integration:
  //
  //   __idealystBindingsSmokeTest()
  //
  // Creates a text node, registers two bindings sharing one signal,
  // fires a signal change, asserts both nodeValues updated.
  // Logs PASS/FAIL.
  window.__idealystBindingsSmokeTest = function () {
    var t1 = document.createTextNode('');
    var t2 = document.createTextNode('');
    var sid = 999999;
    window.__idealystRegisterBinding(
      1,
      t1,
      new Uint32Array([sid]),
      'count: \0',
      '0',
    );
    window.__idealystRegisterBinding(
      2,
      t2,
      new Uint32Array([sid]),
      '(again: \0)',
      '',
    );
    var initialOK = t1.nodeValue === 'count: 0' && t2.nodeValue === '(again: 0)';
    window.__idealystOnSignalChanged(sid, 42);
    var updatedOK = t1.nodeValue === 'count: 42' && t2.nodeValue === '(again: 42)';
    window.__idealystReleaseBinding(1);
    window.__idealystReleaseBinding(2);
    window.__idealystSignalValues.delete(sid);
    var releasedOK = !window.__idealystSignalSubscribers.has(sid)
      && !window.__idealystBindings.has(1)
      && !window.__idealystBindings.has(2);
    var ok = initialOK && updatedOK && releasedOK;
    console.log('[idealyst] bindings smoke test:', ok ? 'PASS' : 'FAIL',
      { initialOK: initialOK, updatedOK: updatedOK, releasedOK: releasedOK });
    return ok;
  };
})();
