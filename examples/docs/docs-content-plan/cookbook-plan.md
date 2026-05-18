# Cookbook plan

The cookbook is a set of **worked recipes** — concrete, end-to-end
examples of common app patterns. Each recipe is one page, one
problem, one complete solution you can read top-to-bottom.

The conceptual pages (Reactivity, Styles, Navigation, …) teach the
framework's vocabulary. The cookbook teaches what to build *with*
it. The two reference each other but stay in their lanes:
conceptual pages explain `signal!` and `flat_list`, the recipes
*use* them to build a search-filtered list with debounced input.

## What goes in the cookbook

Each recipe answers a question of the form **"how do I build
X?"** with a complete answer:

- The user-facing description of the pattern (one paragraph).
- The full source code for the component (one or two files).
- A narrative walkthrough explaining *why* each piece is shaped
  the way it is.
- A live demo embedded in the page (when the pattern produces
  something interactive).
- "See also" pointers to relevant conceptual pages.

Recipes are **not** API reference — they don't enumerate every
prop or describe edge cases. The conceptual pages do that. A
recipe is the happy path.

## MCP behavior

Recipes use `PageCategory::Cookbook`. The MCP server's default
`list_doc_pages` tool excludes them; a separate
`list_cookbook_recipes` tool exposes them. An LLM asking "how
does reactivity work?" lands on Reactivity, not on a list of
seven recipes that happen to use signals.

A model can still navigate to a recipe (`get_page("forms")`
works for any slug), and `search_docs` returns matches across
all categories — but recipes don't drown out the concept pages
in the default surface.

## Proposed recipes

Initial recipe list, in roughly the order I'd write them. Each
gets its own page using the `docs!` macro and the
`PageCategory::Cookbook` tag.

| Recipe | Covers |
| --- | --- |
| Theme switcher | `set_theme`, signal-driven theme swap, why no nodes re-mount |
| Form with validation | controlled inputs, derived state, error display, submit flow |
| List with filter | `flat_list`, reactive filter via derived data |
| Tabs with nested stacks | `TabNavigator` + `Navigator` per tab, state preservation |
| Drawer + body | `DrawerNavigator`, content panel, breakpoint behavior |
| Modal overlay | `Overlay`, signal-driven open/close, dismiss handling |
| Tooltip / popover | `AnchoredOverlay`, `Ref<ButtonHandle>` as anchor |
| Animated mount / unmount | `Presence`, enter/exit transitions |
| Counter component with methods | `methods!`, parent-side `Ref<H>` |
| Reactive style | passing `Signal<Variant>` to a stylesheet builder |
| Imperative scroll | `Ref<ScrollViewHandle>` + scroll-to action |
| Image gallery with detail screen | navigation with typed params |
| Settings screen | inputs (`Toggle`, `Slider`) bound to a settings struct |
| Robot-driven smoke test | how to write a deterministic UI test against `--features robot` |

Fifteen feels about right for a v1 cookbook — enough to cover
the major shapes, not so many that nothing stands out. Add to
the list as patterns prove themselves.

## Recipe page shape

Every recipe follows the same structure so authoring is rigid
and the result reads consistently:

1. **Intro paragraph.** What this recipe does, in one sentence.
2. **What you'll need.** Imports, dependencies, related
   primitives.
3. **The code.** One or two code blocks containing the full
   working source. The reader can paste this into a project and
   run it.
4. **Walkthrough.** Three to five subsections explaining the
   non-obvious parts. Why is this signal `Copy`? Why does the
   derived state read both inputs? Why isn't `value` a `Ref`?
5. **Try it.** A live demo of the pattern (when applicable).
6. **Variations.** Two or three short paragraphs on common
   tweaks — "what if I want this to be async?", "what if the
   list comes from a server?".
7. **See also.** Pointers to the conceptual pages this recipe
   draws on.

A recipe is comfortable around 150-300 lines, similar to a
midsize conceptual page. Recipes that grow beyond ~400 lines
are probably two recipes.

---

## Sample recipe — "Theme switcher"

To anchor the format, here's the first recipe written out as a
prose draft (same shape as the other 13 docs-content-plan files;
will be translated to the `docs!` macro when that lands).

### Theme switcher

A light/dark toggle that swaps the entire app's theme without
re-mounting a single node. Demonstrates how `set_theme` interacts
with the styling system to make theme changes effectively free.

### What you'll need

- A theme struct implementing `ThemeTokens` (see [Styles](#)).
- A signal to drive the active mode.
- A button to flip the signal.

That's it. No theme-context provider, no consumer wrapping, no
re-render trigger.

### The code

```rust
use framework_core::{component, install_theme, set_theme, signal, ui, Primitive};
use crate::theme::{MyTheme};  // your app's theme; see Styles

#[component]
pub fn app() -> Primitive {
    install_theme(MyTheme::light());

    let is_dark = signal!(false);

    ui! {
        ScrollView {
            View(style = page_style()) {
                Heading { "Settings" }

                View(style = row_style()) {
                    Body { "Dark mode" }
                    Switch(
                        value = is_dark,
                        on_change = move |on| {
                            is_dark.set(on);
                            set_theme(if on { MyTheme::dark() } else { MyTheme::light() });
                        },
                    )
                }

                // ...rest of the app
            }
        }
    }
}
```

### Walkthrough

**The signal is local.** `is_dark` lives in `app()`'s scope. It
drives the toggle's UI state but doesn't propagate through the
tree as a context. The framework finds the theme through a
different mechanism — the active-theme arena slot updated by
`set_theme` — so the signal's only job is to keep the toggle's
visual position in sync with whichever theme is active.

**`set_theme` writes to the arena, not to the components.** The
active theme is stored as a `Signal<Rc<dyn Any>>` inside the
framework's arena. Every styled node's `apply_style` call lives
inside an Effect that reads this signal. When `set_theme` runs:

1. The arena slot updates.
2. Every Effect subscribed to the theme signal fires.
3. Each fires runs its stylesheet against the new theme,
   producing fresh `StyleRules`, and calls the backend's
   `apply_style` on its node.

No component re-runs. No subtree re-mounts. No node is replaced.
The Effect re-fire is the *only* update path.

**On web, even the Effects often don't run.** The web backend
emits stylesheet rules as `var(--name, fallback)` references and
installs theme tokens as CSS custom properties on `:root`. When
`set_theme` runs on web, the framework calls
`install_theme_variables` once with the new token values; the
browser re-resolves every `var(--...)` reference in the rendered
DOM in a single paint. No Effect re-fire per node, no
`apply_style` per node. Theme swap is O(tokens), not O(styled
nodes). See [Styles](#) for the full mechanism.

**Native backends do the per-Effect path.** iOS and Android don't
have a runtime variable system, so they take the standard route:
re-fire the style Effect for each styled node, re-resolve, push
fresh values to the native widget. Cost proportional to
*affected* nodes, not the size of the tree.

**You can drive the signal from anywhere.** `is_dark` is
`Copy`; pass it into nested components, capture it in event
handlers, save it to local storage in an Effect. The `Switch`'s
visual state stays in sync because both the toggle and any
`set_theme` writer share the one source of truth.

### Try it

*(Live demo will be inserted via `demo(theme_switcher_demo)`
once the `docs!` macro lands.)*

### Variations

**Persist the choice.** Wrap the signal write in an Effect that
syncs to local storage on web (via `web_sys::Storage`) or to
`UserDefaults`/`SharedPreferences` on native. Read the persisted
value at app start and pass it to `signal!`.

**Follow the OS.** Read `Backend::color_scheme()` at startup to
seed the signal, then listen for OS-level appearance changes.
The framework's render setup calls `color_scheme` before the
first install; you can use the returned value to pick the
initial theme.

**More than two themes.** `MyTheme::light()` and
`MyTheme::dark()` are just constructor functions. Add
`MyTheme::high_contrast()`, `MyTheme::sepia()`, whatever — the
framework only sees one type. Drive the active variant from a
`Signal<ThemeChoice>` and call `set_theme(...)` in an Effect.

### See also

- [Styles](#) — the full theming model, including tokens,
  runtime variables, and the per-backend cost model.
- [Reactivity](#) — why `is_dark` is `Copy` and what
  "the active theme is a signal" means in practice.
- [Backends](#) — what each backend does on theme swap (CSS
  variables on web, Effect re-fire on native).

---

That's one recipe. The plan above lists fourteen more; each
follows the same shape (intro / what-you-need / code /
walkthrough / try-it / variations / see-also). I'll write them
once the `docs!` macro is in place and we can author them
directly in their final form — translating these planning drafts
would just be busywork before the macro lands.

## Where this slots in

When the docs ship:

- **Sidebar:** Cookbook is its own section, listed after the
  reference pages.
- **Cross-references:** conceptual pages can link to a recipe
  ("for a complete example, see [Theme switcher](#)").
- **MCP:** `list_cookbook_recipes` exposes the recipe set as a
  separate surface; `get_page` works for any slug; cookbook
  recipes don't appear in `list_doc_pages` by default.
- **Search:** recipes participate in `search_docs`. A model
  asking "form validation" finds the Forms recipe even though
  it's in the Cookbook category.
