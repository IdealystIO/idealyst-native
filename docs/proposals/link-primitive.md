# Proposal: the `Link` primitive

Navigation is currently imperative: authors hold a `Ref<NavigatorHandle>`
and call `handle.push(&route, params)` from a click handler. That
works, but it loses a lot of what hyperlinks naturally give you on the
web — semantic markup (`<a href>`), browser hover affordances,
right-click "open in new tab," screen-reader "link" role, search-engine
crawlability, the URL appearing in the status bar on hover.

We can have those *and* the imperative API. The `Link` primitive
is the declarative counterpart to `NavigatorHandle::push` — a
content-wrapping primitive that emits the platform's native
"navigate" affordance and dispatches a nav command when activated.

> Status: proposal. Not implemented.

---

## Surface

### Author API

```rust
use framework_core::ui;
use framework_core::primitives::link::{link, NavKind};

ui! {
    Link(route = HOME_ROUTE, params = ()) {
        Text { "Home" }
    }

    Link(route = DETAIL_ROUTE, params = DetailParams { id: 42 }) {
        Icon(name = IconName::ChevronRight)
        Text { "View user 42" }
    }

    // Replace semantics — same shape, different builder.
    Link(route = LOGIN_ROUTE, params = ()) {
        Text { "Log in" }
    }.kind(NavKind::Replace)
}
```

Same `Route<P>` + typed-params machinery the imperative
`NavigatorHandle::push` uses. The link's type-check rules are
identical to push's: if `params` doesn't match `route`'s `P`, the
build fails.

### Constructor

```rust
pub fn link<P: RouteParams>(
    route: &Route<P>,
    params: P,
    children: Vec<Primitive>,
) -> Bound<LinkHandle>
```

Children are the visible content — the same `Vec<Primitive>` slot
`scroll_view`, `view`, and `navigator` accept. The link itself
contributes interaction + accessibility; visual styling is the
content's job (or the link's optional `style` slot).

### Builder methods

```rust
impl Bound<LinkHandle> {
    /// Push (default) / replace / reset.
    pub fn kind(self, k: NavKind) -> Self;

    /// Explicitly target a different navigator (the link otherwise
    /// uses the ambient one — see "Ambient navigator" below).
    pub fn via(self, nav: Ref<NavigatorHandle>) -> Self;

    /// Bind to a Ref<LinkHandle> for imperative `activate()`.
    pub fn bind(self, r: Ref<LinkHandle>) -> Self;
}
```

`NavKind` is a small enum:

```rust
pub enum NavKind {
    /// Push the route onto the stack — the default.
    Push,
    /// Replace the top of the stack with the new route. Same as
    /// `NavigatorHandle::replace`.
    Replace,
    /// Clear the stack and mount the new route as the root. Same as
    /// `NavigatorHandle::reset`. Useful for post-login redirects.
    Reset,
}
```

`Pop` is intentionally not a link kind — a hyperlink that navigates
backward isn't really a hyperlink, it's a back button. Use a
regular `Button` + `nav.pop()` for that.

### Handle

```rust
pub struct LinkHandle { … }

impl LinkHandle {
    /// Fire the link's nav command programmatically. Useful for
    /// "press enter on a focused row triggers its link" patterns
    /// where you can't synthesize a click.
    pub fn activate(&self);
}

pub trait LinkOps {
    fn activate(&self, node: &dyn Any);
}
```

---

## The `Primitive` variant

```rust
pub enum Primitive {
    // …existing…
    Link {
        children: Vec<Primitive>,

        /// Route name (stable; matches `Route::name()`). The backend
        /// passes this to the dispatcher as-is — no string parsing
        /// per click.
        route: &'static str,

        /// Concrete URL path produced by `params.to_path(route.path)`
        /// at construction time. Web uses this for the `<a href>`
        /// and right-click behavior; native backends ignore it.
        url: String,

        /// Type-erased params; downcast to the registered route's
        /// `P` inside the navigator's screen builder when the link
        /// is activated.
        params: Rc<dyn Any>,

        /// Push / Replace / Reset.
        kind: NavKind,

        /// Explicit target navigator. `None` ⇒ use the ambient
        /// navigator (see below).
        target: Option<Ref<NavigatorHandle>>,

        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
    },
}
```

Notes:

- `params` is `Rc<dyn Any>` so the same link can fire multiple times
  without re-cloning the params payload per click. Construction
  consumes `P` once; dispatch borrows.
- `url` is pre-computed at construction. Web's `<a href>` and right-
  click/copy-link work without needing to defer URL rendering to
  click time.
- `target: Option<Ref<NavigatorHandle>>` is the explicit-override
  escape hatch. The common path is `None` and the ambient
  navigator resolves it.

---

## Ambient navigator

The navigator system already runs each screen inside its own
`Scope` (`mount_screen` builds the subtree inside
`with_scope(&mut screen_scope, || …)`). We extend that to also
**stash a reference to the navigator's `Rc<NavigatorControl>` in
a thread-local** while the screen builds:

```rust
// framework-core internal:
thread_local! {
    static AMBIENT_NAV: RefCell<Vec<Rc<NavigatorControl>>> =
        const { RefCell::new(Vec::new()) };
}

// inside Navigator's mount_screen:
fn mount_screen(name: &'static str, params: Box<dyn Any>) -> (N, u64) {
    let mut scope = Box::new(reactive::Scope::new());
    let node = reactive::with_scope(&mut scope, || {
        AMBIENT_NAV.with(|stack| stack.borrow_mut().push(control.clone()));
        let result = build(&backend, screen_builder(params));
        AMBIENT_NAV.with(|stack| stack.borrow_mut().pop());
        result
    });
    // … register scope, return ((node, id))
}
```

Nested navigators push/pop in order, so a `Link` inside a child
navigator's screen targets the child by default. Authors who want
to break out (deep-link from a nested nav back to the root nav)
use `.via(root_nav_ref)`.

`link(...)` reads `AMBIENT_NAV` at construction time, captures the
relevant `Rc<NavigatorControl>` in the primitive, and dispatches
through it on activation. The control plane is already shared
between the handle and the framework (it's what
`NavigatorHandle::push` flows through), so a link going through
the ambient control plane is dispatched-identically to a
programmatic `handle.push`.

### Failure mode

A `Link` constructed outside any screen has no ambient navigator.
The constructor warns once and the link silently no-ops on
activation. This mirrors the navigator's existing
"handle-before-build" posture (commands dispatched before
`control.install(...)` runs are dropped). The `.via(...)` escape
hatch is the explicit fix.

---

## Render walker

Add a `Primitive::Link` arm to `build`:

```rust
Primitive::Link { children, route, url, params, kind, target, style, ref_fill } => {
    let on_activate: Rc<dyn Fn()> = {
        let route_name = route;
        let url = url.clone();
        let params = params.clone();
        let target = target
            .and_then(|r| r.with(|h| h.control()))   // explicit
            .or_else(|| AMBIENT_NAV.with(|s| s.borrow().last().cloned()));  // ambient
        Rc::new(move || {
            let Some(control) = target.as_ref() else { return };
            let cmd = match kind {
                NavKind::Push    => NavCommand::Push    { name: route_name, url: url.clone(), params: clone_any(&params) },
                NavKind::Replace => NavCommand::Replace { name: route_name, url: url.clone(), params: clone_any(&params) },
                NavKind::Reset   => NavCommand::Reset   { name: route_name, url: url.clone(), params: clone_any(&params) },
            };
            control.dispatch(cmd);
        })
    };

    let n = backend.borrow_mut().create_link(LinkConfig {
        url: url.clone(),
        route: route_name,
        on_activate,
    });

    // children are built and inserted exactly like View's:
    for child in children {
        let child_node = build(backend, child);
        backend.borrow_mut().insert(&mut n.clone(), child_node);
    }

    if let Some(s) = style { attach_style(backend, &n, s); }
    if let Some(RefFill::Link(fill)) = ref_fill {
        fill(backend.borrow().make_link_handle(&n));
    }
    n
}
```

(`clone_any` is a small helper that deep-clones the params Rc; we
need a fresh `Box<dyn Any>` per dispatch because the
`NavCommand::*` variants own their params.)

---

## Backend trait additions

```rust
pub trait Backend {
    // …existing…

    /// Create a navigable container.
    ///
    /// The backend is responsible for:
    /// - Producing the platform-native widget that wraps the
    ///   eventual children (a `<a>` on web, a tappable container
    ///   with accessibility role "link" on native).
    /// - Calling `config.on_activate()` when the user activates it
    ///   (click on web, tap / VoiceOver-activate on native).
    /// - Honoring `config.url` for any platform affordances that
    ///   need it (web hover status bar, right-click "copy link
    ///   address," etc.).
    ///
    /// On web specifically, the backend SHOULD emit a real `<a>`
    /// with `href=config.url` so the browser's native link behaviors
    /// (middle-click open in new tab, right-click menu, keyboard
    /// activation, search-engine crawlability) work without
    /// re-implementation. The click handler that fires
    /// `on_activate` should `preventDefault` on a plain click so
    /// the SPA stays single-page, but leave modified clicks
    /// (cmd/ctrl/middle, shift) to the browser's default handler.
    #[allow(unused_variables)]
    fn create_link(&mut self, config: LinkConfig) -> Self::Node {
        unimplemented!("create_link not implemented for this backend")
    }

    #[allow(unused_variables)]
    fn make_link_handle(&self, node: &Self::Node) -> LinkHandle {
        LinkHandle::new(Rc::new(()), &NoopLinkOps)
    }
}

/// What `Backend::create_link` receives.
pub struct LinkConfig {
    /// Concrete URL for the link's target. Useful on web; ignored on
    /// native.
    pub url: String,
    /// Route name. Useful to backends that want to expose the route
    /// in accessibility metadata (e.g. "Link to home").
    pub route: &'static str,
    /// Fire on activation. The framework wraps push/replace/reset
    /// dispatch in here, so the backend doesn't need to know which.
    pub on_activate: Rc<dyn Fn()>,
}
```

The trait defaults are unimplemented and a no-op handle so
backends ship the rest of the framework before adding link
support.

---

## Per-backend implementation

### Web

```rust
fn create_link(&mut self, config: LinkConfig) -> Node {
    let a = document.create_element("a").unwrap();
    a.set_attribute("href", &config.url).ok();
    // Accessibility role "link" is implicit on <a>.

    let on_activate = config.on_activate.clone();
    let listener = Closure::wrap(Box::new(move |evt: web_sys::MouseEvent| {
        // Leave modified clicks to the browser (middle-click,
        // cmd/ctrl-click, shift-click → open in new tab, new
        // window, etc.).
        if evt.button() != 0
            || evt.meta_key() || evt.ctrl_key()
            || evt.shift_key() || evt.alt_key()
        {
            return;
        }
        evt.prevent_default();
        on_activate();
    }) as Box<dyn FnMut(_)>);
    a.add_event_listener_with_callback("click", listener.as_ref().unchecked_ref()).unwrap();
    // … stash listener so it lives as long as the node …

    a.into()
}
```

Plus the existing `popstate` handler (already in the web
navigator) for forward/back. The navigator's URL state stays in
sync because the dispatcher's `Push` branch calls
`history.pushState(state, "", url)` exactly as `handle.push` does
today.

The CSS reset for `<a>` defaults (`color`, `text-decoration`) is
either applied by the framework's web backend or left to user
styles, same way `Button`'s native chrome is handled.

### iOS

```rust
fn create_link(&mut self, config: LinkConfig) -> Self::Node {
    let view = UIView::new();
    view.set_user_interaction_enabled(true);
    view.set_accessibility_traits(UIAccessibilityTraits::Link);
    view.set_accessibility_label(format!("Link to {}", config.route));

    let tap = UITapGestureRecognizer::new();
    let on_activate = config.on_activate.clone();
    tap.set_action(move || on_activate());
    view.add_gesture_recognizer(&tap);

    // …return GlobalRef-equivalent…
}
```

The `Link` accessibility trait is what VoiceOver reads — distinct
from `Button` ("activates an action") so the user knows they're
navigating, not triggering an action.

### Android

```rust
fn create_link(&mut self, config: LinkConfig) -> GlobalRef {
    let layout = FrameLayout(context);
    layout.set_clickable(true);
    layout.set_focusable(true);

    // Accessibility — Android exposes link role via
    // AccessibilityNodeInfo's setRoleDescription/className.
    layout.set_content_description(format!("Link to {}", config.route));
    // The compat way: set className to "android.widget.TextView" +
    // mark as link via an AccessibilityDelegate. Real impl uses
    // ViewCompat.setAccessibilityDelegate to emit a Link role.

    let on_activate = config.on_activate.clone();
    layout.set_on_click_listener(move || on_activate());

    // …return GlobalRef…
}
```

---

## What changes elsewhere

- `Primitive` enum grows one variant.
- `RefFill` enum grows one variant: `Link(Box<dyn FnOnce(LinkHandle)>)`.
- Walker grows the `Primitive::Link` arm above.
- `framework_macros::ui` / `jsx` recognize `Link` as a primitive
  name — same special-casing as `Button`, `View`, `Text`. (Lower
  to `framework_core::primitives::link::link(...)`.)
- `Navigator`'s `mount_screen` pushes/pops the ambient navigator
  stack around its `with_scope` call.
- New module: `framework_core::primitives::link` (handle, ops,
  constructor, `Bound<LinkHandle>` impl).

Backends without an implementation get the trait default
(`unimplemented!()`) — same posture as every other primitive. An
app that doesn't construct `Link` builds and runs without
requiring backend support.

---

## Why this shape, not the alternatives

### Why not just expose `nav.push` and document the pattern?

Imperative `nav.push` is what we have today and what `Link` will
desugar to internally. The reason to add a primitive on top:

- **Web semantics for free.** A real `<a href>` participates in
  the browser's link contract: hover URL preview, right-click menu,
  middle-click, cmd-click, screen reader role, search-engine
  crawlability. Re-implementing those by hand on every link
  callsite is busywork, and the typical mistakes (missing
  middle-click handling, missing role) are exactly the things
  users with assistive tech notice first.
- **Static analysis.** A primitive lets future tooling extract the
  set of declared routes + the set of declared links and
  cross-check them. Imperative dispatch can't be statically
  inspected.
- **SSR / pre-rendering.** A future SSR backend can render the
  full link graph at build time without executing handlers.
  Imperative dispatch breaks that.

### Why not a builder-only API (`button(...).links_to(route, params)`)?

Buttons and links are semantically different:

- A button activates a verb on the current screen ("save," "send,"
  "delete").
- A link navigates to another screen.

Conflating them obscures that distinction in markup. Mobile users
don't see it, but on web the difference shows up in middle-click
behavior, in screen-reader announcements, and in keyboard
activation (Enter on a focused link navigates; Space on a button
activates).

### Why not require the explicit `nav` ref?

Discussed in the "Ambient navigator" section: the implicit case
is the overwhelming common path, and forcing every link to plumb
a ref through every component crossing the boundary creates the
exact "prop drilling" friction the framework otherwise avoids.
The explicit `.via(...)` escape hatch covers nested navigators.

### Why type-erase params via `Rc<dyn Any>` instead of keeping `P`?

Same reason `NavCommand::Push` already type-erases: keeping the
generic `P` would parameterize `Primitive` (and the whole walker)
on the link's params type, which doesn't work — different links
in the same tree have different `P`. The downcast at the
screen-builder boundary is the same downcast `nav.push` already
performs, and it's already well-tested.

---

## Outstanding questions

1. **Should `Link` participate in interaction states?** Web links
   have `:hover`, `:focus`, `:active`. The framework's `StateBits`
   model supports those (`HOVERED`, `PRESSED`, `FOCUSED`). The
   answer is almost certainly yes — link styling needs hover
   underline, focus rings, etc. Implementation: same path
   `Button` uses, no new state bits.

2. **Should we expose a `download` attr equivalent on web?** Out
   of scope for v1. If wanted later, a separate primitive
   (`Download`) or a builder method that emits `<a download>` on
   web and a "share/save" gesture on native.

3. **Should `Link` allow no-ambient + no-`.via` (no-op silently),
   or hard-fail?** Current proposal: warn-once + silently no-op
   (matches handle posture). Hard-fail would catch misuse but
   breaks the "build link outside a navigator just to render the
   static URL" pre-render case some SSR setups would want.

4. **Should we add an `external` shape — `Link(url = "https://…")`
   — for non-app destinations?** Web would emit `<a target=_blank
   rel=noopener>`; native would launch the system browser.
   Different enough from in-app navigation that it should probably
   be its own primitive (`ExternalLink` or `OpenURL`). Out of
   scope for this proposal.
