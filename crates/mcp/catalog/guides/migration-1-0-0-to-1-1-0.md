+++
title = "Migrating 1.0 → 1.1"
order = 906
tags = ["migration", "1.1.0", "breaking", "sdk", "registry", "registration", "lazy", "code-splitting"]
+++

# Migrating 1.0 → 1.1

1.1.0 is a **small, entirely SDK-registration-shaped release**. Nothing in the
reactive system, the element model, the backend seam or the style system
changed. If your app renders no extension SDKs, there is nothing to do.

What changed is the seam every client SDK exposes for installing its scene
handler. In 1.0 those seams had four different names between them, one of them
was a no-op that silently did nothing, and there was no consumer-facing way to
defer an SDK's handler into a lazy chunk. All three are fixed here.

> **A note on versioning.** [[migrations]] says breaking changes wait for a
> major bump. This release breaks that rule deliberately: the surface involved
> is a handful of registration function names, the fixes are mechanical, and one
> of them repairs a call that silently did nothing — carrying that to 2.0 would
> have left a documented-but-broken call site in the tree for a whole major
> cycle. Weigh the exception against the size of the change, not the letter of
> the policy.

## 1. Every SDK registration seam is now named `register`

**What changed.** Four spellings collapsed to one.

| 1.0 | 1.1 |
| --- | --- |
| `table::register_handlers(registry)` | `table::register(registry)` |
| `screen_recorder::register_scene(registry)` | `screen_recorder::register(registry)` |
| `canvas_core::register_ssr_scene(registry)` | `canvas_core::register_ssr(registry)` |
| `stack_navigator::register` / `register_generic` | **deleted** — call nothing |
| `swap_navigator::register` / `register_generic` | **deleted** — call nothing |

**Why.** The name is the whole discovery surface. An app author reading a
`Cargo.lock` has no way to know that `table`'s seam was spelled differently from
`markdown`'s, and no tooling can check a convention that has four spellings.
`canvas_core::register_ssr_scene` additionally read as a typo of the *app-side*
`register_ssr_scene_handlers` seam, which is an unrelated thing.

**Migrate.** Rename the call. The signatures are unchanged.

```rust
// before (1.0)
table::register_handlers(registry);
screen_recorder::register_scene(registry);
canvas_core::register_ssr_scene(registry);

// after (1.1)
table::register(registry);
screen_recorder::register(registry);
canvas_core::register_ssr(registry);
```

For the navigators, **delete the call**. Stack and swap handlers are vocabulary
built-ins installed by `register_builtins`; there was never anything for an app
to register, and the shims only ever did nothing.

Status: landed

## 2. `table::register` is no longer a silent no-op

**What changed.** In 1.0, `table` shipped *two* functions. The real seam was
`register_handlers`; `register` was this:

```rust
pub fn register<B>(_backend: &mut B) {}
```

`B` is unconstrained, so `table::register(registry)` compiled, did nothing, and
left every table panicking at realize. The [[sdks]] guide documented exactly
that call. In 1.1 `register` *is* the real seam.

**Why.** This is the sharpest edge the whole release exists to remove. A caller
who followed the convention — or the official guide — got a call that type-checked,
ran, and silently failed, then sent them debugging a registration they believed
they had already fixed. The same shape existed on both navigators.

**Migrate.** If you called `table::register(&mut backend)` and your tables
panicked at realize anyway, that call now works — no edit needed. If you passed
something that is not a `Registry`, it will no longer compile, which is the
point.

```rust
// 1.0: compiled, did nothing, tables panicked at realize
table::register(&mut backend);

// 1.1: installs the three handlers
table::register(registry);
```

Status: landed

## 3. New: `defer` / `register_from_chunk` on SDKs that can be split

**What changed.** SDKs that own both their payload and their handler now expose
two more seams alongside `register`:

| Seam | Called from | Effect |
| --- | --- | --- |
| `register(registry)` | `register_scene_extensions`, at boot | installs the handler now |
| `defer(registry)` | `register_scene_extensions`, at boot | declares the kind late-bound |
| `register_from_chunk::<H>()` | inside a `#[component(lazy)]` body | installs it when the chunk lands |

Shipped on: `table`, `markdown`, `codeblock`, `svg`, `video`, `form`,
`webview`, `maps`.

**Why.** [[lazy-loading]] documents deferring an SDK handler as
`registry.defer::<HeavyProps>()`. That only works if the payload type is public —
and for most SDKs it isn't and shouldn't be. `svg::SvgPrim` and
`markdown::MarkdownPrim` are private, so **no app could defer them at all**.
`table` was worse: three payloads, each keyed on `PrimCell<…>`, a framework
internal an app would have to import to name. The seam moves that knowledge
back inside the SDK where it belongs.

**Migrate.** Nothing to change — these are additive. To adopt lazy registration:

```rust
// boot: declare instead of install
pub fn register_scene_extensions<H: runtime_scene::Host>(
    registry: &mut runtime_scene::Registry<H>,
) {
    markdown::defer(registry);
}

// the chunk installs the handler on its way in
#[component(lazy)]
fn DocsPane(source: String) -> Element {
    markdown::register_from_chunk::<backend_web::WebBackend>();
    ui! { Markdown(source = source) }
}
```

**Both halves are required.** Deferring a kind that nothing later registers
leaves the payload parked behind a layout-transparent placeholder forever — no
panic, no log, just missing content.

Off-web, `defer` installs the handler eagerly instead of declaring it, because
only web code-splits and native has no chunk to arrive. So `defer` is always
safe to call, on any target, and `register_from_chunk` is inert everywhere but
wasm.

Status: landed

### Canvas is deliberately excluded

`canvas-core` ships no `defer`. Its payload lives in one crate and its handlers
in `canvas-native` / `canvas-vello`, so it has no handler to fall back to
off-web, and both renderer chunk seams are web-only — a `defer` there would be
correct on web and a silent blank canvas on native. `CanvasPrim` is public, so
the deferred path is spelled directly, exactly as [[lazy-loading]] shows:

```rust
registry.defer::<canvas_core::CanvasPrim>();
```

Status: landed

## What did not change

- Handler signatures, payload types, and `ui!` call sites.
- The scene `Registry` API (`register`, `defer`, `register_deferred`,
  `defer_registration`) — unchanged since 1.0.
- Every SDK that already spelled its seam `register` (`markdown`, `codeblock`,
  `svg`, `video`, `webview`, `form`, `maps`, `toolbar`, `canvas-native`,
  `canvas-vello`).

## Migration checklist

- [ ] `table::register_handlers` → `table::register`.
- [ ] `screen_recorder::register_scene` → `screen_recorder::register`.
- [ ] `canvas_core::register_ssr_scene` → `canvas_core::register_ssr`.
- [ ] `stack_navigator::register` / `register_generic` calls deleted.
- [ ] `swap_navigator::register` / `register_generic` calls deleted.
- [ ] Any `table::register(&mut backend)` call reviewed — it used to do nothing
      and now installs handlers.
- [ ] Every extension SDK you render appears exactly once in
      `register_scene_extensions`, under `register` **or** `defer`.
- [ ] Any SDK you `defer` has a `#[component(lazy)]` body calling its
      `register_from_chunk` — a defer with no chunk renders nothing, silently.

## See also

- [[sdk-components]] — the authoring side: the seam convention these renames
  establish, and how to support lazy loading in your own SDK.
- [[lazy-loading]] — splitting an app, chunks, and `--data-prune`.
- [[sdks]] — the consumer index of available SDKs.
