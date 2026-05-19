# Native runtime: Navigator push/pop/select — DONE

All command-shape dispatch is now wired:

- **Stack `Navigator`**: `Push` / `Pop` / `Replace` / `Reset` all
  mount/unmount real screen subtrees. The renderer's last-child walk
  picks up the new top of stack automatically.
- **`TabNavigator`**: `Select` flips `active_tab` for already-mounted
  tabs; mounts on demand and appends to the tab list when the route is
  first activated (lazy-persistent semantics — every mounted tab stays
  mounted).
- **`DrawerNavigator`**: `OpenDrawer` / `CloseDrawer` / `ToggleDrawer`
  flip `is_open` (unchanged). `Select` swaps the active body screen via
  the same name→index lookup as tabs and closes the panel after
  selection (matches the React Navigation default).

The remaining unfinished work below is preserved as historical
context for the redesign — it explains the pattern that landed.

---

## The blocking issue (resolved)

## The blocking issue

Backend trait methods only have `&mut self`. The navigator dispatcher
needs to:

1. Call `callbacks.mount_screen(name, params)` → gets a `(WgpuNode, u64)`
2. Insert the new screen as a Taffy child of the navigator
3. For pop: call `callbacks.release_screen(scope_id)` and remove the old
   child from Taffy

Steps 2–3 require mutating `self.layout` (the `LayoutTree`) and the
navigator node's children Vec. The dispatcher closure runs from user
code (handle.push, etc.) **after** `create_navigator` returns, when
the framework has released its borrow. It needs a `&mut WgpuBackend`
to do its work — but it's a long-lived closure stored on
`NavigatorControl`, not a method call.

The framework's iOS / Android / web backends solve this by capturing
an `Rc<RefCell<Self>>` of the backend into the dispatcher closure. They
have that handle because their `create_navigator` impl runs inside the
framework's `render(Rc<RefCell<B>>, app)` entry, where the outer Rc is
threaded down to the per-primitive create paths via the Backend's own
constructor.

Our `WgpuBackend` doesn't currently hold a self-pointer.

## The fix (~3 small commits)

### 1. Backend stores `Weak<RefCell<Self>>`

Add to `WgpuBackend`:

```rust
pub(crate) self_weak: std::cell::OnceCell<Weak<RefCell<WgpuBackend>>>,
```

`OnceCell` because it gets set exactly once. Host plumbs the value in:

```rust
// in Host::new, after constructing the backend Rc:
let backend = Rc::new(RefCell::new(WgpuBackend::new(...)));
let _ = backend.borrow().self_weak.set(Rc::downgrade(&backend));
```

This is the same pattern already used for `request_redraw`'s thread-local
hook.

### 2. Dispatcher captures the weak handle

Rewrite `install_navigator_dispatcher` to:

```rust
fn install_navigator_dispatcher(
    nav_node: &WgpuNode,
    callbacks: NavigatorCallbacks<WgpuNode>,
    control: Rc<NavigatorControl>,
    backend_weak: Weak<RefCell<WgpuBackend>>,
) {
    let nav_weak = Rc::downgrade(nav_node);
    let cbs = Rc::new(callbacks); // already Rc-friendly internally
    control.install(Box::new(move |cmd| {
        let (Some(backend), Some(nav)) =
            (backend_weak.upgrade(), nav_weak.upgrade()) else { return };
        let mut b = backend.borrow_mut();
        match cmd {
            NavCommand::Push { name, params, .. } => {
                let (screen, _scope) = (cbs.mount_screen)(name, params);
                // Insert as Taffy child + push onto nav.children.
                let nl = nav.borrow().layout;
                let cl = screen.borrow().layout;
                b.layout.add_child(nl, cl);
                nav.borrow_mut().children.push(screen.clone());
                b.roots.retain(|n| !Rc::ptr_eq(n, &screen));
                (cbs.depth_changed)(nav.borrow().children.len());
            }
            NavCommand::Pop => {
                if let Some(top) = nav.borrow_mut().children.pop() {
                    let cl = top.borrow().layout;
                    let nl = nav.borrow().layout;
                    b.layout.remove_child(nl, cl);
                    drop_subtree(&mut b.layout, &b.text, &mut b.animator,
                                 &mut b.active_spinner_count, &top);
                    // Find the scope_id via tracking we'd add per-screen.
                    // (cbs.release_screen)(scope_id);
                    (cbs.depth_changed)(nav.borrow().children.len());
                }
            }
            NavCommand::Replace { name, params, .. } => {
                // pop + push without firing two depth_changed
            }
            NavCommand::Reset { name, params, .. } => {
                // clear all + push
            }
            _ => {}
        }
        crate::scheduler::request_redraw();
    }));
}
```

The tricky bit: `cbs.release_screen` takes a `scope_id` (u64). The
backend needs to remember the scope_id per-screen so pop can release
the right one. Add it to `NodeKind::Navigator`:

```rust
Navigator {
    /// `(child_index, scope_id)` for each mounted screen, in push
    /// order. `release_screen` is keyed by scope_id, not node ref.
    screen_scopes: RefCell<Vec<u64>>,
}
```

`*_attach_initial` pushes scope_id 0 (the framework uses 0 for the
initial route) and subsequent `Push` commands push the framework-
returned ids.

### 3. Tab + drawer dispatchers do the same

`TabNavigator::Select` needs a name→index map. The simplest approach:
store the registered route names on the node at create time. The
`TabNavigatorCallbacks` struct (in `primitives/navigator/tabs.rs`)
already exposes the registered routes; capture them into the dispatcher
closure.

`DrawerNavigator::Select` is identical to TabNavigator::Select once
the screen-swap dispatcher exists.

## Mount policy: tabs

Tab navigators have a `mount_policy`:

- `EagerPersistent` — mount every tab at create time.
- `LazyPersistent` — mount on first activation, keep mounted.
- `LazyDisposing` — mount on activation, release the previous tab.

The wgpu backend should honor at least `EagerPersistent` + `LazyPersistent`.
`LazyDisposing` is a nice-to-have. For Eager:

```rust
// inside create_tab_navigator's body, *after* returning:
// (can't mount synchronously here — re-entrant borrow). Use a deferred
// queue drained from host.tick or from the framework's microtask hook.
```

The framework's `tab_navigator_attach_initial` covers the first tab; the
rest need to mount when the borrow lifts. Easiest: have `create_tab_navigator`
queue mount commands on the navigator node, drain them from the
dispatcher's first call (or from host.tick if no commands come).

## What to do about `default_link_kind`

A `Link` nested inside a tab/drawer navigator should default to `Select`
instead of `Push`. The framework's `NavigatorControl::set_default_link_kind`
sets this — call it in `create_tab_navigator` and `create_drawer_navigator`:

```rust
control.set_default_link_kind(DefaultLinkKind::Select);
```

Already done for stack navigators (they keep the `Push` default).

## Testing

Add a tab-nav screen + a drawer-nav screen to `examples/native-test`
once the dispatcher lands. The smoke test should:

1. Open the app with a tab navigator that has 3 tabs.
2. Click each tab — content should switch.
3. The third tab contains a stack navigator with a button that pushes
   a sub-screen; back gesture (header-bar tap or hardware back) pops.

## Scope / non-goals

- The simulator does **not** need to implement animated transitions
  between screens. iOS-style horizontal slide + drawer slide are stretch
  goals. A snap-replace works for v1.
- `WebView` stays unsupported indefinitely (placeholder panel).
- Bundling Blitz for real HTML rendering is a separate effort tracked
  outside this file.
