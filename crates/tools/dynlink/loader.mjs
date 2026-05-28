// Minimal wasm dynamic linker (Emscripten dylink.0 subset).
//
// Loads a PIC `--shared` side module against an already-instantiated main
// module, resolving the side's imports against main's exports:
//   - env.memory / __indirect_function_table / __stack_pointer  -> main's
//   - env.__memory_base / __table_base                          -> assigned region
//   - env.<fn>                                                  -> main.exports[fn]
//   - GOT.mem.<sym>   (a static's address)   -> main.exports[sym] (address global)
//   - GOT.func.<sym>  (a fn's table index)   -> append main.exports[sym] to the table
//
// This is what makes main + side share ONE heap, ONE reactive arena, ONE of
// every `thread_local!`: every shared static resolves to main's instance.

export async function loadSide(main, sideMod, { regionBytes = 8 * 1024 * 1024, sideTableReserve = 2048 } = {}) {
  const ex = main.exports;
  const mem = ex.memory;
  const table = ex.__indirect_function_table;
  const g = (v, mut) => new WebAssembly.Global({ value: "i32", mutable: mut }, v);

  // Reserve the side's region FROM main's allocator so DLMALLOC won't reuse
  // it. Data loads at memoryBase (grows up); stack at the top (grows down).
  const memoryBase = ex.host_reserve(regionBytes);
  const stackTop = memoryBase + regionBytes - 16;

  const imports = {};
  const unresolved = [];
  for (const imp of WebAssembly.Module.imports(sideMod)) {
    const ns = (imports[imp.module] ??= {});
    const { name } = imp;
    if (imp.module === "env" && name === "memory") { ns.memory = mem; continue; }
    if (name === "__indirect_function_table") { ns[name] = table; continue; }
    if (name === "__memory_base") { ns[name] = g(memoryBase, false); continue; }
    if (name === "__table_base")  { continue; }    // deferred: set after GOT.func slots
    if (name === "__stack_pointer") {
      ns[name] = ex.__stack_pointer instanceof WebAssembly.Global ? ex.__stack_pointer : g(stackTop, true);
      continue;
    }
    if (imp.module === "GOT.mem") {
      const a = ex[name];                          // main's address-global for the static
      if (a instanceof WebAssembly.Global) ns[name] = g(a.value, true);
      else { ns[name] = g(0, true); unresolved.push("GOT.mem." + name); }
      continue;
    }
    if (imp.module === "GOT.func") {
      const fn = ex[name];                         // main's function -> give the side a table slot for it
      if (typeof fn === "function") {
        const idx = table.length; table.grow(1); table.set(idx, fn);
        ns[name] = g(idx, true);
      } else { ns[name] = g(0, true); unresolved.push("GOT.func." + name); }
      continue;
    }
    if (imp.kind === "function") {                 // env.<fn> imported from main
      const fn = ex[name];
      ns[name] = typeof fn === "function" ? fn : () => { throw new Error("unresolved fn " + name); };
      if (typeof fn !== "function") unresolved.push("env." + name);
      continue;
    }
    ns[name] = g(0, true);                          // fallback (other globals)
  }

  // The side places its OWN functions at __table_base; reserve that region
  // AFTER the GOT.func slots so the two don't overlap.
  const tableBase = table.length;
  table.grow(sideTableReserve);
  (imports.env ??= {}).__table_base = g(tableBase, false);

  const inst = await WebAssembly.instantiate(sideMod, imports);
  inst.exports.__wasm_apply_data_relocs?.();        // patch the side's data pointers via the GOT
  inst.exports.__wasm_call_ctors?.();
  return { side: inst, memoryBase, tableBase, unresolved };
}
