# runtime-core

The framework's **author surface**. Every app, SDK, component library and
example spells the framework as `runtime_core::…`, and this crate is that
root — a paper-thin re-export of [`runtime_vocabulary::glue`] plus the
macro set (`ui!`, `#[component]`, `#[props]`, `stylesheet!`, `rx!`,
`effect!`, `typeface!`, …), which a module re-export cannot carry.

It is **not** the runtime. Nothing is implemented here.

## Where the runtime actually lives

| Crate | Owns |
| --- | --- |
| `runtime-world` | The reactive kernel: worlds, signals, staged commits, derivation-class flush. |
| `runtime-scene` | The scene model: `Element`, `realize`, the `Host` seam, the `Registry` (third-party handlers), the Dyn/Keyed structural drivers. |
| `runtime-vocabulary` | The 30 capability (`*Ops`) traits over the `Host` seam, the builtin primitive handlers, the style-attach engine, navigation, and `glue` — the author-surface module this crate re-exports. |
| `runtime-shared` | The substrate: style engine + tokens, assets/typefaces, animation, touch/wheel/hover/file-drop input, scheduling, identity, introspection, the robot registry + bridge, and every primitive's prop/handle types. |
| `runtime-macros` | The proc macros. Their expansions target `::runtime_vocabulary::glue`. |
| `runtime-layout` | Taffy wrapper: `StyleRules` → flex/grid layout for the native backends. |

**Backends do not depend on this crate.** They consume `runtime-shared`,
`runtime-scene` and `runtime-vocabulary` directly. This root is the
author surface, and its `glue` re-export deliberately shadows several
substrate names with authoring wrappers (`view`, `text`, `primitives`, …)
that a backend must not pick up.

## Extending the author surface

Add the item to `runtime_vocabulary::glue`, with a vocabulary-suite test
if it carries logic. Do not add it here — items in `glue` sit next to
the machinery they wrap, and the glob re-export below picks them up
automatically. The only exceptions are macros, whose `$crate::…`
expansions need a crate root to resolve against.

## History

Until the runtime-v2 deletion this package held the pre-v2 renderer: the
`Element` enum, the 159-method `Backend` mega-trait, the render walker,
`Bound`/builders, the `External` table, `batch`, and the legacy reactive
arena — about 34 k lines. All of it is gone. See
[`docs/migrating-to-runtime-v2.md`](../../../docs/migrating-to-runtime-v2.md)
for the what-moved / what-replaced-it table and
[`docs/runtime-v2-deletion-baseline.md`](../../../docs/runtime-v2-deletion-baseline.md)
for the frozen pre-deletion record.
