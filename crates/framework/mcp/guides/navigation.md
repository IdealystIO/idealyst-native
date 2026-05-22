+++
title = "Navigation"
order = 50
tags = ["navigation"]
+++

# Navigation

Idealyst ships a navigator system that maps the platform-native navigation chrome (`UINavigationController`, Fragment back-stack, browser History) to a single author API.

## Primitives

- **Stack navigator** — push/pop screens, native swipe-back on iOS.
- **Tab navigator** — bottom tabs (mobile) or side rail (desktop/web).
- **Drawer navigator** — hamburger drawer; responsive between modal and pinned per theme breakpoint.
- **Card-tabs navigator** — secondary tab layer inside a screen.

Per [[native_first_layout_for_web]], chrome (titles, tab bars, drawer) is configured through navigator **screen options**, not the `style` system. The `.layout(...)` builder is a web-only escape hatch.

## A small example

```rust
ui! {
    StackNavigator {
        Screen(name = "home", options = ScreenOptions::default().title("Home")) {
            HomeScreen
        }
        Screen(name = "details", options = ScreenOptions::default().title("Details")) {
            DetailsScreen
        }
    }
}
```

Navigation between screens goes through the navigator's runtime handle — push a route by name, the framework drives the platform-native transition.

## Layout vs sidebar reactive scopes

If you wire layout or sidebar contents reactively, keep a keepalive `Effect` alive past the `build_*_navigator` return (see [[navigator_scope_keepalive]]). Without it the reactive scope is dropped and updates stop firing.

## Drawer responsiveness

The drawer navigator should switch between modal (mobile) and pinned (desktop) based on the active theme breakpoint, not a magic pixel threshold ([[drawer_pinned_above_smell]]). Read [[current_breakpoint]] and let the theme own the threshold.
