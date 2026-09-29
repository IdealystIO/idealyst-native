// when running the harness we need to make sure to uncommon this out...

export function makeLoad(url, deps, fusedImports, initIt) {
  let alreadyLoaded = false;
  return async (callbackIndex, callbackData) => {
    await Promise.all(deps.map((dep) => dep()));
    if (alreadyLoaded) return;

    // Resolve the main module's callback table up front. The Rust
    // `SplitLoaderFuture` stays `Poll::Pending` until this callback fires and
    // it also reclaims the leaked `Rc<SplitLoader>` handed to `into_raw`.
    // If we ever return without calling it, `LazyLoader::load()` hangs forever
    // AND the loader is leaked. So we MUST signal on every exit path, including
    // failure — resolve `load()` to `false` rather than never.
    let signal;
    let mainExports;
    try {
      const initSync = initIt || globalThis.__wasm_split_main_initSync;
      mainExports = initSync(undefined, undefined);
      signal = (ok) => {
        if (callbackIndex !== undefined) {
          mainExports.__indirect_function_table.get(callbackIndex)(
            callbackData,
            ok
          );
        }
      };
    } catch (e) {
      // If even the main module's init/table lookup fails there is no way to
      // wake the future; surface the error loudly rather than hang silently.
      console.error(
        "Failed to resolve wasm-split callback table",
        e,
        url,
        deps
      );
      throw e;
    }

    try {
      const response = await fetch(url);

      let imports = {
        env: {
          memory: mainExports.memory,
        },
        __wasm_split: {
          __indirect_function_table: mainExports.__indirect_function_table,
          __stack_pointer: mainExports.__stack_pointer,
          __tls_base: mainExports.__tls_base,
          memory: mainExports.memory,
        },
      };

      for (let mainExport in mainExports) {
        imports["__wasm_split"][mainExport] = mainExports[mainExport];
      }

      for (let name in fusedImports) {
        imports["__wasm_split"][name] = fusedImports[name];
      }

      let new_exports = await WebAssembly.instantiateStreaming(
        response,
        imports
      );

      alreadyLoaded = true;

      for (let name in new_exports.instance.exports) {
        fusedImports[name] = new_exports.instance.exports[name];
      }

      signal(true);
    } catch (e) {
      console.error(
        "Failed to load wasm-split module",
        e,
        url,
        deps,
        fusedImports
      );
      // Wake the Rust future with `false` so `load()` resolves and the app can
      // fall back, instead of spinning on its loading state forever.
      signal(false);
    }
  };
}

let fusedImports = {};
