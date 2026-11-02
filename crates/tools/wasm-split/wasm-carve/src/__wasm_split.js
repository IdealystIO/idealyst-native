// Retry schedule for a TRANSIENT fetch failure (no response at all, or a
// 408 / 429 / 5xx): one attempt plus one per entry. A brief network blip
// should not fail the lazy boundary.
const RETRY_DELAYS_MS = [500, 2000];

// sessionStorage key prefix for the stale-build reload guard (see
// `reloadOnceForStaleBuild`).
const RELOAD_GUARD_PREFIX = "__wasm_split_reloaded:";

class ChunkLoadError extends Error {
  // `status` is the HTTP status, or `null` when fetch produced no response.
  constructor(url, status, cause) {
    super(
      status === null
        ? `network error fetching ${url}`
        : `HTTP ${status} fetching ${url}`,
      { cause }
    );
    this.name = "ChunkLoadError";
    this.url = url;
    this.status = status;
  }
  get transient() {
    const s = this.status;
    return s === null || s === 408 || s === 429 || s >= 500;
  }
  // The server no longer has this build's file: a deploy replaced the
  // build this page is running and removed the old files.
  get stale() {
    return this.status === 404 || this.status === 410;
  }
}

async function fetchChunk(url) {
  for (let attempt = 0; ; attempt++) {
    let error;
    try {
      const response = await fetch(url);
      if (response.ok) return response;
      error = new ChunkLoadError(url, response.status);
    } catch (e) {
      error = new ChunkLoadError(url, null, e);
    }
    if (!error.transient || attempt >= RETRY_DELAYS_MS.length) throw error;
    await new Promise((resolve) => setTimeout(resolve, RETRY_DELAYS_MS[attempt]));
  }
}

// A chunk of the RUNNING build is gone from the server, so no retry can
// succeed: the page has to load the current build. Reload once per chunk
// URL — the URL carries the build hash, so after the reload the new build
// asks for different URLs, and a server that 404s the NEW build's chunk
// too is reloaded at most once more instead of looping. Without
// sessionStorage (blocked storage) the guard can't be kept, so no reload;
// the lazy boundary's error UI is what the user sees.
function reloadOnceForStaleBuild(url) {
  if (typeof location === "undefined") return;
  const key = RELOAD_GUARD_PREFIX + url;
  try {
    if (sessionStorage.getItem(key) !== null) return;
    sessionStorage.setItem(key, "1");
  } catch (_) {
    return;
  }
  console.warn("wasm-split module missing from the server; reloading for the current build", url);
  location.reload();
}

export function makeLoad(url, deps, fusedImports, initIt) {
  // The in-flight or finished instantiation, shared by every caller (the
  // module's own loader and every module that lists it as a dependency).
  // Cleared on failure so the next call fetches again.
  let instantiated = null;

  const instantiate = async (mainExports) => {
    await Promise.all(deps.map((dep) => dep()));
    const response = await fetchChunk(url);

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

    let new_exports = await WebAssembly.instantiateStreaming(response, imports);

    for (let name in new_exports.instance.exports) {
      fusedImports[name] = new_exports.instance.exports[name];
    }
  };

  const ensure = (mainExports) => {
    if (instantiated === null) {
      instantiated = instantiate(mainExports);
      instantiated.catch(() => {
        instantiated = null;
      });
    }
    return instantiated;
  };

  // Called by Rust with a callback (an index into main's function table)
  // and called WITHOUT one by a dependent module's loader, which awaits the
  // returned promise — so a failed dependency rejects into the dependent.
  return async (callbackIndex, callbackData) => {
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
        mainExports.__indirect_function_table.get(callbackIndex)(callbackData, ok);
      };
    } catch (e) {
      // If even the main module's init/table lookup fails there is no way to
      // wake the future; surface the error loudly rather than hang silently.
      console.error("Failed to resolve wasm-split callback table", e, url, deps);
      throw e;
    }

    if (callbackIndex === undefined) return ensure(mainExports);

    try {
      await ensure(mainExports);
      signal(true);
    } catch (e) {
      console.error("Failed to load wasm-split module", e, url, deps, fusedImports);
      if (e instanceof ChunkLoadError && e.stale) reloadOnceForStaleBuild(e.url);
      // Wake the Rust future with `false` so `load()` resolves and the lazy
      // boundary shows its error UI (a later attempt fetches again).
      signal(false);
    }
  };
}

let fusedImports = {};
