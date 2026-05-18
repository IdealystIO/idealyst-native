//! Primitives page — built via the `docs!` macro.
//!
//! Migrated from `docs-content-plan/02-primitives.md`. The macro emits
//! `pub fn page() -> Primitive` and `pub static PAGE_META: PageMeta`.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{codeblock, pageheader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "primitives",
    title = "Primitives",
    category = Foundation,
    description = "The fixed set of things the framework knows how to put on screen.",
    related = ["reactivity", "styles", "lists", "navigation", "robot"],
    concepts = [Primitive, Container, Content, Input, ReactiveControlFlow],

    section(heading = "Overview") {
        p("Primitives are the fixed set of things the framework knows how to put \
           on screen. Every Idealyst app — and every component library built on \
           top of the framework, including idea-ui — reduces to a tree of these."),
        p("There is no way to add a new primitive without changing the framework \
           itself, because each primitive corresponds to a method on the ",
          code("Backend"), " trait that every backend has to implement. The cost \
           of adding one is high; that's deliberate. What you build out of the \
           primitives is unbounded."),
    },

    section(heading = "What every primitive shares") {
        p("A few things are true of every primitive on this page, so they're \
           worth saying once instead of repeating per entry:"),
        list(
            ["Styles are orthogonal. Every primitive takes an optional ",
             code("style"), " slot. A primitive can have any style applied to \
             it, and styling is its own subsystem — see Styles for the full story."],
            ["Refs are optional. Every primitive takes an optional ref so parent \
             code can hold a handle to the underlying node and call imperative \
             methods on it. See Refs."],
            ["Test ids are optional. With ", code("--features robot"), ", every \
             primitive accepts a ", code("test_id"), " that the Robot \
             introspection layer uses to find it. See Robot."],
            ["Some primitives are reactive on their content. When an input is a \
             closure that reads a signal (or a ", code("format!"), " that reads ",
             code(".get()"), "), the framework wraps that read in an Effect and \
             updates the live node when the signal changes. The relevant \
             primitives note this below."],
        ),
    },

    section(heading = "Containers") {
        p("The structural primitives — boxes, scrolling boxes, and tappable boxes. \
           Everything else nests inside one of these at some level."),
    },

    section(heading = "View") {
        p("A generic box. The structural workhorse — everything composes inside \
           a View at some level."),
        code(rust, r##"
            ui! {
                View(style = my_view_style()) {
                    Text { "hello" }
                    Text { "world" }
                }
            }
        "##),
        p("Children are a flat list of primitives, laid out by the platform's \
           flex engine (via framework-native-layout on native backends, via the \
           browser on web). A View has no behavior of its own — no press target, \
           no scrolling, no clipping unless its style says so."),
        p("Optional ", code("safe_area_sides"), " opts the view into per-side \
           safe-area padding. The backend reactively adds the platform's inset \
           (status bar, home indicator, dynamic island) to the matching sides; \
           rotation and dynamic-island changes propagate without a rebuild."),
    },

    section(heading = "ScrollView") {
        p("A View that scrolls. Vertical by default; ", code("horizontal = true"),
          " flips the axis."),
        code(rust, r##"
            ui! {
                ScrollView {
                    // ...children scroll vertically
                }
            }
        "##),
        p("Maps to a ", code("div"), " with ", code("overflow: scroll"), " on \
           web, ", code("UIScrollView"), " on iOS, ", code("ScrollView"), " or ",
          code("HorizontalScrollView"), " on Android. Like ", code("View"),
          ", it can opt into safe-area padding per side — useful at the screen \
           root so content can pass under the status bar while headers respect \
           the inset."),
    },

    section(heading = "Pressable") {
        p("A View that's also tappable. No native chrome — the visual is whatever \
           its children and style say it is."),
        code(rust, r##"
            ui! {
                Pressable(on_click = move || open_sheet()) {
                    Card { /* ... */ }
                }
            }
        "##),
        p("Use Pressable when you want button behavior without button visuals: \
           tappable card surfaces, menu rows, custom-styled buttons whose look \
           is owned by the stylesheet. For a button with a label and native \
           semantics (form submission, default focus ring), use ", code("Button"),
          " instead."),
        p("State styling works the same as any other primitive — ",
          code("state hovered { ... }"), ", ", code("state pressed { ... }"), ", ",
          code("state focused { ... }"), ", ", code("state disabled { ... }"),
          " blocks in a stylesheet apply automatically."),
    },

    section(heading = "Content") {
        p("Primitives that render media — text runs, bitmaps, vector icons, \
           video, and embedded web content."),
    },

    section(heading = "Text") {
        p("A run of text."),
        code(rust, r##"
            ui! {
                Text { "Hello, Idealyst" }
                Text { format!("Count: {}", count.get()) }
            }
        "##),
        p("The child can be any expression that produces a string. If the \
           expression reads a signal (via ", code(".get()"), "), the framework \
           wraps the read in an Effect and updates the live node when the signal \
           changes. A static literal is computed once and never updated."),
    },

    section(heading = "Image") {
        p("A bitmap from a URL."),
        code(rust, r##"
            ui! {
                Image(src = "https://example.com/avatar.png", alt = "Avatar")
            }
        "##),
        p(code("src"), " accepts a string or a closure for reactive URLs. ",
          code("alt"), " maps to the platform accessibility label (", code("alt"),
          " on web, ", code("accessibilityLabel"), " on iOS, ",
          code("contentDescription"), " on Android). Codec support is whatever \
           the platform handles natively."),
    },

    section(heading = "Icon") {
        p("A vector icon, rendered as inline SVG on web, ", code("CAShapeLayer"),
          " on iOS, ", code("VectorDrawable"), " on Android."),
        code(rust, r##"
            ui! {
                Icon(name = "chevron-right")
            }
        "##),
        p("The icon registry is tree-shakeable — only icons referenced by your \
           code end up in the binary. Icons support a reactive ", code("color"),
          " override and a stroke-draw animation (the path can progressively \
           reveal itself). See the Icons page for the registry and authoring \
           custom icons."),
    },

    section(heading = "Video") {
        p("Video playback. URL-only — backends use their native players, so \
           codec support is whatever the platform handles."),
        code(rust, r##"
            ui! {
                Video(src = "https://...", autoplay = true, controls = true, loop_playback = false)
            }
        "##),
    },

    section(heading = "WebView") {
        p("Embedded web content. A sandboxed iframe on web, ", code("WKWebView"),
          " on iOS, ", code("android.webkit.WebView"), " on Android."),
        code(rust, r##"
            ui! {
                WebView(url = "https://example.com")
            }
        "##),
    },

    section(heading = "Inputs") {
        p("All four input primitives are controlled: the parent owns the value \
           as a signal, and the input round-trips through ", code("on_change"), "."),
    },

    section(heading = "Button") {
        p("A labeled tappable with native button semantics."),
        code(rust, r##"
            ui! {
                Button(
                    label = "Increment",
                    on_click = move || count.update(|n| *n += 1),
                )
            }
        "##),
        p("Optional ", code("leading_icon"), " / ", code("trailing_icon"),
          " render before/after the label using the platform's native \
           button-icon API (", code("UIButton.setImage"), " on iOS, compound \
           drawable on Android, inline SVG on web)."),
        p("The optional ", code("disabled"), " is a reactive flag — a closure \
           that returns ", code("bool"), ". When it flips, the framework marks \
           the native widget inert and toggles the ", code("state disabled"),
          " styling block."),
    },

    section(heading = "TextInput") {
        p("A text field whose value is owned by the parent."),
        code(rust, r##"
            let value = signal!(String::new());

            ui! {
                TextInput(
                    value = value,
                    on_change = move |s| value.set(s),
                    placeholder = "Search...",
                )
            }
        "##),
        p("The framework writes the value back into the native widget when the \
           signal changes (cyclic but stable — widgets no-op when set to their \
           current value)."),
    },

    section(heading = "Toggle") {
        p("A switch / checkbox bound to a ", code("Signal<bool>"),
          ". The builder function is ", code("switch(...)"),
          "; the primitive variant is ", code("Toggle"),
          ". They're the same thing — the constructor is named for what you'd \
           call the control on screen."),
        code(rust, r##"
            let enabled = signal!(true);

            ui! {
                Switch(
                    value = enabled,
                    on_change = move |v| enabled.set(v),
                )
            }
        "##),
    },

    section(heading = "Slider") {
        p("A numeric slider with min/max bounds and an optional step."),
        code(rust, r##"
            let volume = signal!(50.0);

            ui! {
                Slider(
                    value = volume,
                    on_change = move |v| volume.set(v),
                    min = 0.0,
                    max = 100.0,
                    step = Some(5.0),
                )
            }
        "##),
        p("When ", code("step"), " is set, the framework snaps incoming ",
          code("on_change"), " values to the nearest step before dispatching — \
           behavior matches across backends, regardless of whether the platform \
           widget supports stepping natively."),
    },

    section(heading = "Feedback") {
        p("Primitives whose only job is to show that something is happening."),
    },

    section(heading = "ActivityIndicator") {
        p("An indeterminate loading spinner. No methods, no value — it just spins."),
        code(rust, r##"
            ui! {
                ActivityIndicator(size = ActivityIndicatorSize::Medium, color = Some(my_color))
            }
        "##),
    },

    section(heading = "Reactive control flow") {
        p("These three primitives express dynamic structure: they decide what \
           to build based on a signal, and rebuild atomically when the signal \
           changes."),
    },

    section(heading = "When") {
        p("Reactive ", code("if"), "/", code("else"),
          ". You usually don't construct this directly — write a plain ",
          code("if"), " inside ", code("ui!"),
          " whose condition reads a signal, and the macro lowers it to ",
          code("When"), "."),
        code(rust, r##"
            ui! {
                if logged_in.get() {
                    Text { "Welcome back!" }
                } else {
                    Button(label = "Log in", on_click = move || logged_in.set(true))
                }
            }
        "##),
        p("When the condition changes, the old branch's scope drops (freeing \
           every signal, effect, and node inside it) and the new branch builds \
           in a fresh scope. See Reactivity for how this works."),
    },

    section(heading = "Switch") {
        p("Reactive multi-way match — the n-way version of ", code("When"),
          ". Each arm has a JSON-serializable pattern; the framework picks the \
           first arm whose pattern equals the discriminant."),
        code(rust, r##"
            ui! {
                Switch(discriminant = mode) {
                    Arm(pattern = "loading") { ActivityIndicator() }
                    Arm(pattern = "ready") { Body { /* ... */ } }
                    Default { Text { "error" } }
                }
            }
        "##),
        p("(The exact ", code("ui!"), " syntax for this is settling — see \
           Reactive control flow for current shape.)"),
        p("The JSON constraint exists because the match must round-trip through \
           both the runtime path and the generator-backend wire format with the \
           same equality semantics."),
    },

    section(heading = "Repeat") {
        p("Bulk children. The macro lowers ", code("for i in 0..n { ... }"),
          " inside ", code("ui!"), " into ", code("Repeat"),
          ", which hands the backend a single ", code("insert_many"),
          " call with all rows preassembled — one FFI call instead of N."),
        p("You don't write ", code("Repeat"), " by hand; you write a ",
          code("for"), "."),
        code(rust, r##"
            ui! {
                View {
                    for item in items {
                        Card { Text { item.title } }
                    }
                }
            }
        "##),
        p("For large lists you want a ", code("Virtualizer"), ", not a ",
          code("Repeat"), " — see below."),
    },

    section(heading = "Lists") {
        p("The virtualized-list entry point. Use this when row count is large \
           enough that materializing every row would be wasteful."),
    },

    section(heading = "Virtualizer (via flat_list)") {
        p("A virtualized list that only realizes the visible rows. The typed \
           entry point is ", code("flat_list<T>(items, render_item, …)"), "."),
        code(rust, r##"
            ui! {
                flat_list(
                    items = signal_of_items,
                    item_size = ItemSize::Fixed(72.0),
                    render_item = |i, item| ui! { Card { Text { &item.title } } },
                )
            }
        "##),
        p("Backends drive their native virtualization widget (",
          code("UICollectionView"), " on iOS, ", code("RecyclerView"),
          " on Android, intersection-observer-based on web). For Roku and other \
           generator backends, the row template is pre-built once and the device \
           runtime materializes per-row instances."),
        p("See Lists for keying, overscan, item-size strategies, and horizontal \
           lists."),
    },

    section(heading = "Navigation") {
        p("Navigation has its own page — these are the entry points."),
    },

    section(heading = "Navigator") {
        p("A stack-based navigator. Push, pop, replace, reset, with a \
           declarative route table built up via ",
          code("Screen(route = ..., title = ...)"),
          " children. Backends own the platform-native stack (",
          code("UINavigationController"), " on iOS, ", code("FragmentManager"),
          " on Android, an inline subtree swap on web)."),
    },

    section(heading = "TabNavigator") {
        p("A tab bar plus a switched content region. An ordered list of ",
          code("Screen"), " entries plus a route table."),
    },

    section(heading = "DrawerNavigator") {
        p("A slide-in side panel plus a switched body region. Can be pinned \
           beside the body above a viewport-width breakpoint (becomes a \
           sidebar). The docs site uses this at the top level."),
    },

    section(heading = "Link") {
        p("Declarative navigation. Wraps a child that, when pressed, dispatches \
           a ", code("NavCommand"), " against the closest ambient navigator."),
        code(rust, r##"
            ui! {
                Link(route = "/profile/:id", params = ProfileParams { id: 42 }) {
                    Text { "Open profile" }
                }
            }
        "##),
        p("On web, the wrapper emits an ", code("<a href=…>"),
          " so right-click \"open-in-new-tab\" works. On native, the wrapper is \
           invisible and the press dispatches in-process."),
        p("See Navigation for the full route / params / dispatch model."),
    },

    section(heading = "Overlays and animation") {
        p("Floating subtrees that escape the parent's layout and clipping — \
           modals, popovers, presence-aware mount/unmount."),
    },

    section(heading = "Overlay") {
        p("A viewport-positioned floating subtree — modals, drawers, full-screen \
           sheets. Renders above the rest of the UI and escapes the parent's \
           layout / clipping."),
        p("The host owns open/close state. Mounting the primitive opens the \
           overlay; unmounting closes it. Wire ", code("on_dismiss"),
          " to flip your open-state signal when the platform requests dismissal \
           (Escape, back gesture, click-outside on a dismissible backdrop)."),
    },

    section(heading = "AnchoredOverlay") {
        p("A floating subtree positioned relative to another primitive's \
           rendered bounds — popovers, tooltips, dropdowns, context menus, \
           edit-menus. Follows its anchor through scrolls, layout shifts, and \
           orientation flips."),
        p("Backends can route this to a native anchored presentation (",
          code("UIContextMenuInteraction"), ", ",
          code("UIPopoverPresentationController"), ", Android ",
          code("PopupWindow"), ", web ", code("popover"),
          " + CSS anchor positioning) or fall back to manual positioning with \
           a scroll-tracking observer."),
    },

    section(heading = "Presence") {
        p("Mount/unmount with enter and exit animations. Backed by a ",
          code("Signal<bool>"),
          " for the present/absent state; the framework defers the actual \
           unmount until the exit animation's duration elapses, so the leaving \
           subtree stays alive long enough to play its exit."),
        p("See Overlays and animation for placement, backdrop modes, focus \
           trapping, and the animation primitives."),
    },

    section(heading = "Graphics") {
        p("A GPU canvas primitive. You own the rendering; the framework hands \
           you a wgpu device and stays out of the way."),
    },

    section(heading = "Graphics primitive") {
        p("A GPU canvas. You own the rendering — ", code("on_ready"),
          " runs once after the backend has a ", code("wgpu"),
          " device available and produces your render state; ",
          code("on_resize"),
          " and (implicitly) per-frame callbacks let you update."),
        p("The framework does not interpret any of it. The GPU context is \
           type-erased, so framework-core stays wgpu-free even though backends \
           that support graphics carry the dependency."),
        p("Not supported in AAS dev mode — the wire protocol can't ship GPU \
           work, so an AAS host renders a placeholder. Local-render mode is \
           required."),
        p("See Graphics for the lifecycle, surface configuration, and the \
           constraints."),
    },

    section(heading = "What about styles?") {
        p("Every primitive accepts an optional ", code("style"),
          " and an optional set of state-driven overrides. The mechanics of how \
           a stylesheet is declared, themed, and resolved is its own subsystem \
           — see Styles for the model."),
    },

    section(heading = "Where to read more") {
        list(
            ["Reactivity — ", code("When"), ", ", code("Switch"), ", and ",
             code("Repeat"), " rest on the reactive substrate."],
            ["Styles — the styling system every primitive's ", code("style"),
             " slot feeds into."],
            ["Refs — programmatic handles on a primitive."],
            ["Navigation — ", code("Navigator"), ", ", code("TabNavigator"),
             ", ", code("DrawerNavigator"), ", ", code("Link"), "."],
            ["Lists — ", code("Virtualizer"), " / ", code("flat_list"), " in depth."],
            ["Overlays and animation — ", code("Overlay"), ", ",
             code("AnchoredOverlay"), ", ", code("Presence"), "."],
            ["Graphics — the wgpu canvas primitive."],
            ["Robot — ", code("test_id"), " and the introspection layer."],
        ),
    },
}
