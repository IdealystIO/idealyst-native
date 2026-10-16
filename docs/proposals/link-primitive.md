# Proposal: the `link` primitive

Navigation used to be only imperative: authors hold a `Ref<NavigatorHandle>`
and call `handle.push(&route, params)` from a click handler. That works,
but it loses what hyperlinks give you on the web — semantic markup
(`<a href>`), browser hover affordances, right-click "open in new tab,"
the screen-reader "link" role, search-engine crawlability, the URL in the
status bar on hover.

The `link` primitive is the declarative counterpart to
`NavigatorHandle::push` — a content-wrapping primitive that emits the
platform's native "navigate" affordance and dispatches a nav command when
activated.

> **Status: implemented.** The primitive's types live in
> `crates/runtime/shared/src/primitives/link.rs` (`NavKind`, `LinkHandle`,
> `LinkConfig`), the builder in `crates/runtime/vocabulary/src/builders/media.rs`
> (`link()` / `LinkBuilder`), the mount handler in
> `crates/runtime/vocabulary/src/handlers/media.rs` (`mount_link`), and the
> backend capability is `caps::LinkOps` in
> `crates/runtime/vocabulary/src/caps/media.rs`. The author-facing reference
> is [`docs/primitives.md`](../primitives.md). Where the shipped design
> differs from the original sketch, this document describes what shipped.

---

## Surface

```rust
use runtime_core::ui;

ui! {
    link(route = &HOME_ROUTE, params = ()) {
        text { "Home" }
    }

    link(route = &DETAIL_ROUTE, params = DetailParams { id: 42 }) {
        icon(name = IconName::ChevronRight)
        text { "View user 42" }
    }
}
```

Same `Route<P>` + typed-params machinery the imperative
`NavigatorHandle::push` uses: if `params` doesn't match `route`'s `P`, the
build fails. `P: Clone` — each activation reproduces a fresh boxed params
payload.

The builder (`runtime_vocabulary::builders::link()`) also takes:

- `.url(…)` — the destination URL, static or reactive (the rendered href
  swaps in place via `LinkOps::update_link_url`). `.route(…)` pre-computes it
  as `params.to_path(route.path())` so web's `<a href>` and right-click
  "copy link" work without deferring URL rendering to click time.
- `.external(true)` — the link leaves the app; with no explicit callback,
  activation opens the URL with the platform URL opener.
- `.on_activate(f)` — a raw activation callback (in-app dispatch without a
  typed route).
- `.style(…)`, `.a11y(…)`, `.test_id(…)`, `.on_handle(…)` (a `LinkHandle` for
  imperative `activate()`).

A link with none of route / callback / external panics at mount: a link
that silently does nothing is a footgun.

### Which command a link sends

The enclosing navigator decides: its activator turns an activation into
the command that fits its kind — the swap navigator Selects, the stack
navigator Pushes. There is no per-link override (the original sketch had a
`.kind(NavKind::Replace | Reset)` builder; it was not built). `Pop` is
intentionally not something a link does — a hyperlink that navigates
backward is a back button. Use a regular `button` + `nav.pop()`.

---

## Ambient navigator

A route link resolves its destination against the **enclosing navigator**
without the author threading a `Ref<NavigatorHandle>` through every
component that crosses a screen boundary. Each navigator `provide`s a
`LinkActivator` (`crates/runtime/vocabulary/src/prims`) while it mounts its
screens; `mount_link` `inject`s it at mount and dispatches through it on
activation. Nested navigators provide their own activator, so a link inside
a child navigator's screen drives the child.

A link mounted outside any navigator has no activator and silently no-ops on
activation — the same posture as commands sent to a navigator handle before
the navigator is built.

---

## Per-backend behavior

`caps::LinkOps::create_link` receives a `LinkConfig` and wires the platform's
native interaction: a real `<a href>` on web (and in SSR output), an
accessibility-`Link`-roled tappable container on native backends. The
trait default lowers to `create_view`, so a backend without a link
implementation still mounts the children as a non-tappable container.

---

## Why this shape

### Why not just expose `nav.push` and document the pattern?

- **Web semantics for free.** A real `<a href>` participates in the
  browser's link contract: hover URL preview, right-click menu,
  middle-click, cmd-click, screen-reader role, search-engine crawlability.
  Re-implementing those by hand on every call site is busywork, and the
  typical mistakes (missing middle-click handling, missing role) are what
  users with assistive tech notice first.
- **Static analysis.** A primitive lets tooling extract the declared link
  graph and cross-check it against the declared routes.
- **SSR / pre-rendering.** The SSR backend renders the link graph as real
  anchors without executing handlers.

### Why not a builder-only API (`button(...).links_to(route, params)`)?

Buttons and links are semantically different: a button activates a verb
on the current screen ("save," "send"); a link navigates to another
screen. On web the difference shows up in middle-click behavior,
screen-reader announcements, and keyboard activation (Enter on a focused
link navigates; Space on a button activates).

### Why type-erase params?

Different links in the same tree have different `P`; keeping the generic
would parameterize the scene on the link's params type. The downcast at the
screen-builder boundary is the same one `nav.push` performs.
