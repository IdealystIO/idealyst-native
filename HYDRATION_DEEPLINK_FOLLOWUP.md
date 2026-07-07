# Follow-up: deep-link two-screen SSG hydration (reactive-text drop on sub-routes)

Status: **open**. This is the last known bug in the SSG-hydration work that
fixed the nicho-me portfolio site. Home (single-screen) hydrates cleanly;
sub-routes (deep links) render ~90% but drop the first few reactive-text nodes
at the top of the content. Root-caused, not yet fixed (it's a real refactor).

---

## Context: what already landed (do NOT re-do these)

Five framework fixes shipped while chasing this, all with tests, all verified:

1. **`When`-splice hydration gate** — `walker/view.rs`: `Element::When` anchorless
   splice is now gated on `&& !is_hydrating()` (mirrors `Switch`). SSR emits a
   `display:contents` reactive anchor; the client must take the anchored path
   under hydration or it adopts off-by-one. Tests in `tests/walker/hydration.rs`.

2. **`create_external` stale-host guard** — `backend/web/src/lib.rs`: during
   hydration, if an external handler builds fresh (doesn't adopt the SSR host),
   arm a clean subtree remount so the stale host is detached. Tests in
   `backend/web/src/tests.rs` (`create_external_*`).

3. **Canvas SSR host** — `backend/ssr/src/lib.rs` interns `"canvas"` in
   `create_element`; `canvas-core::register_ssr` registers a `CanvasProps` SSR
   handler that emits a bare `<canvas>` the client `graphics` primitive adopts
   via `hydrate_next("canvas")`. Without it the canvas external fell to a `<div>`
   fallback → first divergence + cascade. Tests in `backend/ssr/src/lib.rs`.

4. **Navigator double-mount under hydration** (THE big one) — `helpers/web/src/lib.rs`
   + `backend/web/src/lib.rs` (`is_hydrating()` free fn) + `backend/web/src/scheduler.rs`
   (`is_hydration_active()`). In local mode the initial screen was built twice
   under hydration: the walker's `attach_initial` build (which ADOPTS the SSR
   screen in place) AND the deferred create-time microtask (fresh). Fix: during
   hydration `attach_initial_with_node` is authoritative and the create-time
   microtask's home/initial auto-mount skips. This fixed the whole-screen
   duplication (double nav, double hero, mis-bound overlay, invisible content)
   on single-screen routes. Verified live: home 4 diverging nodes → 1 (harmless),
   content visible. Test: `sdk/client/navigators/stack/tests/hydration_web.rs`
   (integration smoke test; see note below).

5. Regression tests also prove **reactive text and animation hydrate correctly
   in isolation** (`stack/tests/hydration_animated.rs`) — which is how we know
   the remaining bug is NOT reactive-text or animation, but the deep-link path.

Also: gated the pre-existing broken `robot_nav_screen_tree` test behind
`required-features = ["robot"]` (it was blocking wasm test builds).

Test-harness note: the navigator hydration test renders SSR via `backend-ssr`,
injects it into `#app`, then drives `WebBackend::hydrate` + `runtime_core::mount`
in headless Chrome. `hydration_web.rs` asserts the initial screen BUILDS once;
it's an integration smoke test (it caught a cyclic-insert crash mid-dev) but does
NOT reproduce the full-app duplication in isolation — the strict behavioral guard
is the e2e Playwright harness against the built site.

---

## The remaining bug

**Symptom:** on a cold load of a sub-route (e.g. `/contact`, `/about`), the first
few reactive-i18n text nodes are **absent** (not empty) — on `/contact`: the
`Contact` title, the intro (`The best ways to reach me.`), `Email`, and the email
address; on `/about`: the intro paragraph. Everything from there on renders. The
`[hydrate] SSR/client diverge` warnings that remain are exactly these nodes, and
one ("Email") even shows up nested under the *home* hero overlay (`ui-6c2f9131`),
i.e. the two screens' DOM is cross-mixed.

**Why sub-routes only:** the stack navigator, on a cold deep link, reconstructs a
two-screen back-stack `[home-underneath, target]` (so browser-Back returns to the
index). Single-screen routes (home) don't hit this and hydrate perfectly.

### Root cause / mechanism

On web in local mode (`defer_initial_mount == false`), during hydration:

1. The walker (`runtime/core/src/walker/navigator.rs`, the `if !defer_initial_mount`
   branch ~line 414) resolves the initial route. On web `peek_initial_path()` is
   `None` (web reads the URL in the SDK handler layer, not here), so it resolves
   to **`initial` = home** and calls `mount_screen(home)` (~line 454) →
   `navigator_attach_initial`. Under hydration this ADOPTS the SSR home screen in
   place and advances the hydration cursor **through home to the target SSR
   screen** (home is the 1st outlet child, target is the 2nd).

2. The walker then returns and continues to the navigator's **siblings** (the
   scene controls). The cursor is now parked on the **unadopted target SSR
   screen**, so the siblings' adoption consumes/misaligns it → the target
   screen's first reactive-text nodes get dropped/cross-mixed.

3. The deep-link **target** screen mount lives in the deferred create-time
   microtask (`helpers/web/src/lib.rs`, the `else` no-layout branch ~line 976-978:
   `mount_internal(home)` + `mount_internal(target)`), which runs in the drain
   **after** the walker pass. My fix (#4) skips that microtask under hydration, so
   the target is never mounted client-side at all — but even if it weren't
   skipped, it runs too late: the cursor has already moved past the target.

**Core problem:** the deep-link target must be mounted+adopted **during the
synchronous walker pass, right after home** (before the navigator's siblings
consume the target's cursor). Today the deep-link back-stack reconstruction lives
in the deferred microtask, which is too late.

### Fix approach (framework)

Move the deep-link back-stack reconstruction into the synchronous hydration path:

1. **Seed the initial path on the client under hydration.** Before `mount`, on
   web, call `runtime_core::set_initial_path(window.location.pathname)` (only
   during hydration). Then the walker's `navigator.rs` `peek_initial_path()`
   resolves the **target** route synchronously (like SSR's render-at-path does).

2. **Reconstruct `[home, target]` in the walker pass, not the microtask.** With
   the initial path seeded, `navigator.rs` resolves to the target; the STACK
   handler must seat the configured `initial` (home) BELOW the resolved target
   (the code comment at `navigator.rs` ~line 447 says this is "the stack SDK
   handler's job… seats the configured initial BELOW the resolved screen" — but
   the WEB handler doesn't currently do it; the web path does home-underneath in
   the microtask instead). So: implement back-stack reconstruction in the web
   stack handler's `attach_initial` (mount home 1st, target 2nd), each adopting
   the corresponding SSR outlet child IN ORDER during the walker pass.

3. **Cursor: sequential outlet-child adoption.** Each screen mount under
   hydration must adopt the Nth outlet child (home → child 0, target → child 1),
   not reset to the first child. `mount_internal` currently calls
   `hydrate_enter(mount_point())` unconditionally (~line 520), which resets the
   cursor to `first_element_child` — wrong for the 2nd screen. Either continue
   the cursor (skip `hydrate_enter` when `stack` is non-empty) or add a
   `hydrate_enter_child(region, idx)` that points at the Nth child.

4. Keep #4's microtask skip for the now-redundant home/initial auto-mount under
   hydration; the deep-link target is handled synchronously in step 2.

Watch out for: `has_layout()` (drawer/tab) mode does home-underneath differently
(no home mounted; `url_history` push) — handle or explicitly scope to no-layout
(the site is no-layout). Also the `attach_initial`/`create_navigator` container-
cursor mechanics: `hydrate_adopt_container("ui-nav-root")` returns the container
leaving the cursor ON it (doesn't descend); the isolated wasm test hit a
container-as-screen cyclic insert that the full app avoids — understand this
before trusting a minimal repro.

### App-side alternative (sidestep)

The nicho-me nav uses `NavKind::Replace` (no real back-stack in use); the
home-underneath only matters for browser-Back after a cold deep link. If that's
not worth the complexity for a portfolio, drop the two-screen reconstruction so
sub-routes mount a single screen and hydrate cleanly like home does — needs a way
to disable the stack's cold-deep-link back-stack reconstruction (currently
automatic; may need a small framework opt-out flag on the navigator config).

---

## How to verify

1. Build the site with the framework rev: `cd ~/Desktop/nicho-me && idealyst build --web --release --ssg nicho-me` (site pins the idealyst-native rev in `Cargo.toml`; bump it to match the CLI's baked rev — mismatched revs cause "multiple runtime_core versions"). Serve: `idealyst serve /Users/nicho/Desktop/nicho-me/nicho-me/dist/web --port 8080 --host 127.0.0.1`.
2. Playwright harness at `/tmp/pw-harness/` (created during the work; recreate if
   gone — it's a `chromium.launch()` script). Load `/contact` and `/about`,
   capture `[hydrate] SSR/client diverge` console warnings and check the rendered
   `document.body.innerText` includes `Contact` / `The best ways` / `Email`.
   Expected after fix: zero divergences on sub-routes, all content present.
3. Framework unit tests (fast, native + headless wasm):
   - `cargo test -p runtime-core`
   - `wasm-pack test --headless --chrome crates/backend/web --features hydrate`
   - `wasm-pack test --headless --chrome crates/sdk/client/navigators/stack --test hydration_web --test hydration_animated`
   Add a new wasm test reproducing the DEEP-LINK case (two SSR screens in the
   outlet, hydrate at a sub-path via `history.replaceState(null,"","/contact")`,
   assert the target's reactive text survives) — the existing tests only cover
   the single-screen case.

## Key files

- `crates/runtime/core/src/walker/navigator.rs` (~414–455) — initial-mount resolution + `attach_initial` call.
- `crates/sdk/client/navigators/helpers/web/src/lib.rs` — `mount_internal` (~516), the create-time microtask (~918–985), `attach_initial_with_node` (~470).
- `crates/sdk/client/navigators/stack/src/web.rs` — stack web handler `attach_initial` (~106) — where back-stack reconstruction should live.
- `crates/runtime/core/src/primitives/navigator/shared.rs` — `set_initial_path` / `peek_initial_path` (~1442), `defer_initial_mount` default `false` (~1574).
- `crates/backend/web/src/lib.rs` — `is_hydrating()` free fn, `hydrate_enter` (~163), `hydrate_adopt_container` (~784).
