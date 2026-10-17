//! Refs page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "refs",
    title = "Refs",
    category = Reference,
    description = "Programmatic handles to mounted primitives and components.",
    related = ["components", "primitives", "navigation", "portal"],
    concepts = [Ref, Handle, ComponentMethods],

    section(heading = "Overview") {
        p("A ", code("Ref"), " is how you get a programmatic handle to a mounted thing \
           — a view, a text input, a custom component — so you can call methods on it \
           imperatively. You'll reach for one whenever you need to trigger an action \
           on the thing rather than describe it through reactive state."),
        p("The most common case isn't a built-in primitive. It's giving a component \
           you wrote an imperative surface — \"reset this form,\" \"scroll this list \
           to the top,\" \"focus this input,\" \"play this animation\" — and calling \
           it from a parent. This page leads with that pattern because it's the one \
           app code actually uses every day."),
    },

    section(heading = "The two flavors") {
        p("Refs come in two shapes:"),
        list(
            ["Component refs — a handle to one of your own ", code("#[component]"),
             " functions, generated from the nested fns you mark ",
             code("#[method]"), ". Used to call imperative methods you declared on \
              the component."],
            ["Element refs — a handle to a built-in primitive (", code("ViewHandle"),
             ", ", code("TextInputHandle"), ", etc.). Used to trigger a press, read a \
              frame, focus an input, scroll a list to the top."],
        ),
        p("The mechanics are the same in both cases. A ", code("Ref<H>"),
          " is a slot. You hand it to the thing in ", code("ui!"), " (",
          code("bind_to = r"), " on a component, ", code("bind = r"),
          " on a primitive), the framework fills it at mount, and ",
          code("r.get()"), " gives you the handle. The two flavors differ only in \
           what the handle exposes."),
    },

    section(heading = "Component refs") {
        p("This is the pattern you'll reach for most. A parent component wants to \
           imperatively trigger something inside a child — reset a form, scroll a \
           list, focus an input, play an animation. Instead of routing the action \
           through a prop and a signal flip, the child exposes the action as a \
           method and the parent calls it directly."),
    },

    section(heading = "Declaring methods on a component") {
        p("Inside a ", code("#[component]"), " function body, mark nested fns with ",
          code("#[method]"), " to declare the imperative methods the component exposes:"),

        code(rust, r##"
            use runtime_core::{component, signal, ui, Element};

            #[component]
            pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
                let value = signal(initial);

                /// Reset the count to zero.
                #[method]
                fn reset() {
                    value.set(0);
                }

                #[method]
                fn bump_by(n: i32) {
                    value.update(|v| v + n);
                }

                #[method]
                fn set_to(n: i32) {
                    value.set(n);
                }

                ui! {
                    view {
                        text { "Count: {value}" }
                    }
                }
            }
        "##),

        p("What the macro does with them:"),
        list(
            ["Each ", code("#[method]"), " fn becomes a method on a generated ",
             code("CounterHandle"), " — the name is the component name plus ",
             code("Handle"), ". You don't write the struct."],
            ["Method bodies run against captures from the component body — here, \
              the ", code("value"), " signal. That's why there's no ",
             code("&self"), " and no ", code("pub"),
             ": the handle carries the public surface."],
            ["Methods return ", code("()"),
             ". They are commands, not queries — read state through signals, not \
              through the handle."],
            ["The component gets an extra prop, ", code("bind_to: Option<Ref<CounterHandle>>"),
             ", that the macro injects. Don't declare your own ",
             code("bind_to"), " parameter on a component with methods."],
        ),
        p("The return type stays ", code("Element"),
          ". Methods need the inline-props form (props as fn parameters; zero \
           parameters is fine) and a non-generic component. The doc comment on a \
           method is kept, and the methods are also invokable by name from the \
           robot and the inspector."),
        p("Writes inside a method stage and commit at the flush, exactly the way \
           they would if the same write happened inside an ", code("on_click"), "."),
    },

    section(heading = "Capturing the handle in a parent") {
        p("The parent allocates a ", code("Ref<CounterHandle>"),
          " and passes it through ", code("bind_to"), ":"),

        code(rust, r##"
            use runtime_core::Ref;

            #[component]
            pub fn ParentApp() -> Element {
                let counter_ref: Ref<CounterHandle> = Ref::new();

                ui! {
                    Counter(initial = 10, bind_to = counter_ref)

                    button(
                        label = "Reset",
                        on_click = move || { counter_ref.get().map(|h| h.reset()); },
                    )

                    button(
                        label = "Add 5",
                        on_click = move || { counter_ref.get().map(|h| h.bump_by(5)); },
                    )
                }
            }
        "##),

        p("What happens at runtime:"),
        list(
            [code("Ref::new()"), " allocates an empty slot."],
            ["The framework builds the counter. Once its body has built the handle, \
              it fills ", code("counter_ref"), " with a clone of it."],
            ["Before that, ", code("counter_ref.get()"), " returns ", code("None"),
             ", so the click does nothing. After it, ", code("get()"),
             " returns the handle."],
            ["Calling ", code("h.bump_by(5)"), " runs the method body, which \
              updates ", code("value"), ". The counter's text updates on the next \
              flush."],
        ),
        p("Prefer ", code("get()"), " over ", code("with(|h| ...)"),
          " for component handles. ", code("with"),
          " holds a borrow of the ref table for the whole closure, and method \
           bodies write signals — ", code("get()"),
          " clones the handle out first."),
    },

    section(heading = "Sibling-to-sibling through a parent") {
        p("Two children of the same parent can talk to each other through a ref \
           the parent holds:"),

        code(rust, r##"
            let target: Ref<CounterHandle> = Ref::new();

            ui! {
                Counter(bind_to = target)

                button(
                    label = "Set target to 100",
                    on_click = move || { target.get().map(|h| h.set_to(100)); },
                )
            }
        "##),

        p("The button doesn't know about the counter directly — it knows about the \
           ref. The ref is the wire that threads imperative reach across the tree."),
    },

    section(heading = "Method vs signal prop — when to pick which") {
        p("A natural question: \"Should ", code("reset"), " be a method, or should \
           I take a ", code("Signal<bool>"), " prop the parent flips to trigger \
           reset?\""),
        p("The method wins when:"),
        list(
            ["The action is genuinely fire-and-forget — \"do this thing now.\""],
            ["Multiple call sites need to invoke it from different points in the \
              tree."],
            ["The action involves more than just changing one piece of state — \
              focus shifts, animations, scroll position changes."],
        ),
        p("The signal wins when:"),
        list(
            ["The trigger is itself a piece of state worth observing."],
            ["Sibling components also care about the same state and should react \
              to it."],
        ),
        p("In practice, most apps mix both: signals for state that drives UI, \
           methods for actions the parent dispatches."),
    },

    section(heading = "Element refs") {
        p("The framework's built-in primitives expose handles the same way. \
           Allocate a ", code("Ref<H>"), ", pass it as ", code("bind = r"),
          ", call methods:"),

        code(rust, r##"
            use runtime_core::{Ref, ScrollViewHandle};

            let list: Ref<ScrollViewHandle> = Ref::new();

            ui! {
                scroll_view(bind = list) {
                    // rows
                }

                button(
                    label = "Back to top",
                    on_click = move || {
                        if let Some(h) = list.get() {
                            h.scroll_to_top();
                        }
                    },
                )
            }
        "##),

        p("The primitives that take ", code("bind"), " in ", code("ui!"), " are ",
          code("view"), ", ", code("text"), ", ", code("button"), ", ",
          code("icon"), ", ", code("image"), ", ", code("text_input"), ", ",
          code("text_area"), ", and ", code("scroll_view"),
          ". The handle types you'll meet most:"),
        list(
            [code("ViewHandle"), " — ", code(".frame()"), ", ",
             code(".absolute_frame()"), " for geometry"],
            [code("TextInputHandle"), " — ", code(".focus()"), ", ",
             code(".blur()"), ", ", code(".select_all()")],
            [code("ScrollViewHandle"), " — ", code(".scroll_to(x, y)"), ", ",
             code(".scroll_to_top()")],
            [code("PressableHandle"), " and the button handle — ", code(".click()"),
             " fires the press"],
            [code("StackHandle"), " / ", code("SwapHandle"),
             " — navigator commands (", code(".push"), ", ", code(".pop"),
             ", …), bound with ", code(".bind(r)"), " on the navigator builder; see ",
             link("Navigation", to = "navigation")],
        ),
        p("Other primitives have handles too (", code("TextHandle"), ", ",
          code("ImageHandle"), ", ", code("IconHandle"), ", ",
          code("TextAreaHandle"), ", ", code("PresenceHandle"), ", ",
          code("PortalHandle"), "). Third-party SDKs ship their own — e.g. ",
          code("webview::WebViewHandle"),
          " with ", code(".post_message()"), " / ", code(".reload()"), " / ",
          code(".execute_js()"), "."),
        p("The button's handle type is not re-exported from ", code("runtime_core"),
          ", so you can't name it. Let inference pick it: ",
          code("let b = Ref::new();"), " followed by ", code("button(bind = b, …)"),
          " types ", code("b"), " from the prop."),
    },

    section(heading = "Reading geometry") {
        p("A common primitive-ref use is reading a view's bounding rect:"),

        code(rust, r##"
            let row: Ref<ViewHandle> = Ref::new();

            ui! {
                view(bind = row) { /* content */ }

                button(label = "Where am I?", on_click = move || {
                    if let Some(rect) = row.get().and_then(|h| h.frame()) {
                        println!("row is at {:?}", rect);
                    }
                })
            }
        "##),

        p("Two methods:"),
        list(
            [code(".frame()"), " — rect in the parent's coordinate system"],
            [code(".absolute_frame()"), " — rect in viewport (window) coordinates"],
        ),
        p("Both return ", code("Option<ViewportRect>"), " because layout might not \
           have happened yet, or the backend might not expose geometry. Backends \
           that do: web (", code("getBoundingClientRect"), "), iOS (",
          code("UIView.frame"), " + ", code("UIView.convert"), "), Android (",
          code("View.getLocationOnScreen"), ")."),
    },

    section(heading = "Anchoring overlays") {
        p(code("ViewHandle"), ", ", code("PressableHandle"),
          ", and the button handle implement the ", code("AnchorableHandle"),
          " trait. ", code("anchored_overlay"),
          " reads it to position itself relative to the anchor:"),

        code(rust, r##"
            use runtime_core::{AnchorTarget, ElementAlign, ElementSide, Ref, ViewHandle};

            let trigger: Ref<ViewHandle> = Ref::new();
            let is_open = signal(false);

            ui! {
                view(bind = trigger) {
                    button(
                        label = "Open menu",
                        on_click = move || is_open.update(|v| !v),
                    )
                }

                if is_open.get() {
                    anchored_overlay(
                        target = AnchorTarget::from(trigger),
                        side = ElementSide::Below,
                        align = ElementAlign::Start,
                    ) {
                        // menu content
                    }
                }
            }
        "##),

        p("The overlay tracks the anchor through scrolls, layout shifts, and \
           orientation changes — same rect machinery as ", code(".absolute_frame()"),
          ", read continuously. See ", link("Portal", to = "portal"),
          " for how overlays mount."),
    },

    section(heading = "The Ref<H> API") {
        code(rust, r##"
            let r: Ref<H> = Ref::new();

            r.get();                           // Option<H> — clone of the handle once filled
            r.with(|h| h.some_method());      // Option<R> — borrow instead of clone
            r.is_mounted();                    // bool — has the slot been filled
        "##),

        p("A ", code("Ref<H>"), " is a ", code("Copy"), " token, like ",
          code("Signal<T>"), ". Pass it into closures, into child components, \
           anywhere, without ", code(".clone()"),
          ". The token indexes a thread-local ref table; it is separate from the \
           world that holds your signals."),
        p("Inside ", code("with(|h| ...)"),
          " the table is borrowed, so don't write signals or call component \
           methods from that closure. Read values out, or use ", code("get()"),
          " and act after."),
        p("Allocate refs early — typically right at the top of a component — and \
           bind them inline in the ", code("ui!"), " block. The slot stays empty \
           until the primitive (or component) mounts."),
    },

    section(heading = "node_ref! — terser allocation") {
        p(code("node_ref!"),
          " is the macro equivalent of ", code("Ref::new()"),
          ". It exists because the explicit form needs a type \
           somewhere — either on the let-binding or via turbofish:"),

        code(rust, r##"
            // Three equivalent forms — pick the most readable for the call site.
            let row: Ref<ViewHandle> = Ref::new();
            let row = Ref::<ViewHandle>::new();
            let row = node_ref!(ViewHandle);
        "##),

        p("The macro accepts either zero arguments (relies on the \
           let-binding's type annotation for inference, same as ",
          code("Ref::new()"),
          ") or one type argument (matches the turbofish form). The \
           one-argument form is the common case in handler-heavy \
           code where each ref's handle type is obvious from context."),

        p("Spelled ", code("node_ref!"),
          " — not ", code("ref!"), " — because ", code("ref"),
          " is a strict Rust keyword (used in patterns: ",
          code("let ref x = ..."),
          ") and can't be a macro identifier."),
    },

    section(heading = "Binding forms") {
        p("Where a ref goes depends on what you're binding:"),

        code(rust, r##"
            // Primitives in ui!: the `bind` prop.
            view(bind = view_ref) { /* ... */ }
            button(label = "...", on_click = on_click, bind = button_ref)
            scroll_view(bind = scroll_ref) { /* ... */ }

            // Components with #[method] fns: the injected `bind_to` prop.
            Counter(initial = 0, bind_to = counter_ref)

            // Builders outside ui! (navigators, `pressable`): `.bind(r)`.
            StackNavigator::new(&HOME).screen(DETAIL, detail_screen).bind(nav_ref)
        "##),

        p("The handle type has to match the primitive (or component) kind — \
           binding is type-checked. Passing a ", code("Ref<ScrollViewHandle>"),
          " to a ", code("view"), " is a compile error."),
    },

    section(heading = "Lifecycle") {
        p("A ref's slot is empty until the thing it's bound to mounts. Before \
           that, ", code(".get()"), " returns ", code("None"), " and ",
          code(".is_mounted()"), " is ", code("false"),
          ". A primitive fills the slot when it mounts; a component fills it \
           when its body builds the handle. If the bound thing mounts again — \
           an ", code("if"), " branch flipping back, a list row rebuilt — the slot \
           is refilled with the new handle."),
        p("When the bound thing unmounts, its slot empties: after an ", code("if"),
          " branch flips away, ", code(".get()"), " returns ", code("None"),
          ", so ", code("r.get().map(|h| h.reset())"), " is skipped instead of \
           reaching an element that is gone. If the new element mounts before \
           the old one tears down, the new handle stays."),
    },

    section(heading = "Refs vs Signals") {
        p("Two distinctions worth keeping straight:"),
        list(
            [code("Signal<T>"), " — reactive state. Reads inside tracked contexts \
              subscribe; writes notify subscribers. Use for anything that changes \
              over time and drives UI."],
            [code("Ref<H>"), " — an imperative handle slot, filled at mount. \
              Reading it doesn't subscribe to anything. Use to invoke methods on a \
              mounted thing."],
        ),
        p("Both are ", code("Copy"),
          " tokens; signals live in the world, refs in a thread-local ref table. \
           They solve different problems and coexist freely — you'll often fire \
           a ref-driven action from inside a signal-driven event handler."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Components", to = "components"), " — ", code("#[component]"),
             ", inline props, and ", code("#[method]"), "."],
            [link("Navigation", to = "navigation"), " — ", code("StackHandle"),
             ", ", code("SwapHandle"), " and their command surfaces."],
            [link("Portal", to = "portal"), " — how ", code("anchored_overlay"),
             " mounts and anchors."],
            [link("Primitives", to = "primitives"),
             " — the full list of primitives and their handles."],
        ),
    },
}
