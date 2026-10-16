//! Components page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "components",
    title = "Components",
    category = Foundation,
    description = "Rust functions, wrapped in compile-time machinery, that compose into trees.",
    related = ["reactivity", "ui-dsl", "refs", "styles"],
    concepts = [Component, ComponentMethods, Props, Defaults, UiMacro, JsxMacro],

    section(heading = "Overview") {
        p("A component is a Rust function that returns an ", code("Element"),
          ". The ", code("#[component]"), " attribute turns that function into a \
           tag you can call from ", code("ui!"), " or ", code("jsx!"),
          ", gives it a stable identity for hot reload, can expose imperative \
           methods, and rewrites reactive reads in its body. This page covers all \
           of that."),
    },

    section(heading = "The shape") {
        code(rust, r##"
            use runtime_core::{component, signal, ui, Element};

            #[component]
            pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
                let count = signal(initial);

                ui! {
                    view {
                        text { format!("Count: {}", count.get()) }
                        button(
                            label = "Increment",
                            on_click = move || count.update(|n| n + 1),
                        )
                    }
                }
            }
        "##),

        p("Three rules cover the basic shape:"),
        list(
            ["Annotate the function with ", code("#[component]"),
             ". Name it in PascalCase — the tag you write in ", code("ui!"),
             " is the function name."],
            ["Declare props as ordinary parameters. The macro generates a ",
             code("CounterProps"), " struct from them, a ", code("Default"),
             " impl from the ", code("#[prop(default = …)]"), " values, a ",
             code("pub type Counter = CounterProps"), " alias, and a ",
             code("BuildElement"), " impl. That alias and impl are what ",
             code("ui!"), " calls — there is no per-component macro. Data props are \
              wrapped in ", code("Reactive<T>"), " by default; ",
             code("#[prop(static)]"), " keeps a prop as a plain value."],
            ["Return ", code("Element"), ". ", code("ui!"), " already produces one, \
              so the macro block is the whole tail."],
        ),
    },

    section(heading = "Calling a component") {
        p("A component is called by its tag:"),

        code(rust, r##"
            ui! {
                Counter(initial = 0)
                Counter()            // every prop is optional; omitted ones use the default
            }
        "##),

        p("The tag lowers to a struct literal plus a ", code("BuildElement::build"),
          " call: ", code("CounterProps { initial: (0).into(), ..defaults() }"),
          ". Because the call site is a real struct literal, rust-analyzer gives \
           completion, hover, and go-to-definition on every prop, and the tag \
           resolves across crates by ordinary ", code("use"), " paths."),

        p("Inside ", code("ui!"), ", the known snake_case names (", code("view"),
          ", ", code("text"), ", ", code("button"), ", …) are primitives. Every \
           other tag is a component, so write component tags in PascalCase."),
    },

    section(heading = "The explicit props form") {
        p("When the props struct needs extra derives or a hand-written ",
          code("Default"), ", declare it yourself and take it as the one parameter:"),

        code(rust, r##"
            #[derive(Default)]
            pub struct BadgeProps {
                pub label: String,
                pub count: i32,
            }

            #[component(default(count = 3))]
            pub fn Badge(props: &BadgeProps) -> Element {
                ui! { text { format!("{} ({})", props.label, props.count) } }
            }
        "##),

        p(code("#[component(default(field = expr, …))]"),
          " overrides the struct's ", code("Default"), " for the listed fields. \
           Defaults are evaluated per call, not once. Take ", code("props: BadgeProps"),
          " by value when the component needs to move something out of it, such as \
           a ", code("children: Vec<Element>"), " field. ", code("#[method]"),
          " fns (below) need the parameter form."),
    },

    section(heading = "Macros at a glance") {
        p("The authoring macros, each doing one thing:"),
        list(
            [code("#[component]"),
             " — turns a function into a tag. Generates the props glue, rewrites \
              reactive reads, and handles hot reload. Covered on this page."],
            [code("#[method]"),
             " — marks a nested fn inside a ", code("#[component]"),
             " body as an imperative method. Covered below."],
            [code("effect!({ … })"),
             " — a reactive side effect owned by the surrounding component \
              scope (", code("watch(…)"), " is the out-of-tree peer). \
              Covered in Reactivity."],
            [code("ui! { … }"),
             " — the primary UI DSL. Lowers to plain runtime-core calls. Covered \
              below and on the UI DSL page."],
            [code("jsx! { … }"),
             " — a JSX-flavored variant of ", code("ui!"),
             " with identical output. Covered below."],
            [code("stylesheet! { … }"), " — declares a themed stylesheet. Covered in Styles."],
        ),
        p("State is a plain function: ", code("signal(value)"),
          " returns a ", code("Signal<T>"), ". See Reactivity."),
    },

    section(heading = "#[method] — imperative handles") {
        p("Sometimes a parent needs to trigger an action on a child — reset a \
           counter, clear a form, scroll a list to the top. The reactive substrate \
           isn't the right tool for \"do something now\"; an imperative handle is."),

        p("Declare methods as nested fns marked ", code("#[method]"),
          " inside the component body:"),

        code(rust, r##"
            use runtime_core::{component, signal, ui, Element};

            #[component]
            pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
                let value = signal(initial);

                #[method]
                /// Reset the count to zero.
                fn reset() {
                    value.set(0);
                }

                #[method]
                fn bump_by(n: i32) {
                    value.update(|v| v + n);
                }

                ui! {
                    view {
                        text { format!("Count: {}", value.get()) }
                    }
                }
            }
        "##),

        p("The macro generates a ", code("CounterHandle"), " struct with ",
          code("reset"), " and ", code("bump_by"),
          " methods, and adds a ", code("bind_to: Option<Ref<CounterHandle>>"),
          " prop to ", code("CounterProps"), ". The component still returns ",
          code("Element"), "."),

        p("The parent passes a ", code("Ref"), " through that prop:"),

        code(rust, r##"
            use runtime_core::Ref;

            #[component]
            pub fn ParentApp() -> Element {
                let counter: Ref<CounterHandle> = Ref::new();

                ui! {
                    Counter(initial = 0, bind_to = counter)

                    button(
                        label = "Reset",
                        on_click = move || { counter.get().map(|c| c.reset()); },
                    )
                }
            }
        "##),

        p("A few rules:"),
        list(
            ["No ", code("pub"), " and no ", code("&self"),
             ": the body captures the component's locals directly, and the handle \
              carries the public surface."],
            ["Methods return ", code("()"),
             ". They are commands; reads stay signals."],
            ["Call through ", code("Ref::get()"), ", not ", code("Ref::with()"),
             ": methods write signals, and ", code("with"),
             " holds a borrow while the closure runs."],
            ["The handle name is the function name plus ", code("Handle"), ": ",
             code("Counter"), " → ", code("CounterHandle"), "."],
            ["Methods are also registered for the robot and inspector, which can \
              call them by name with JSON arguments."],
            ["Methods need the parameter form of props (zero parameters is fine). \
              The explicit ", code("props: &FooProps"),
             " form and generic components are rejected with a compile error."],
        ),

        p("See Refs for the full handle / ref surface."),
    },

    section(heading = "ui! and jsx! — the same DSL, two surfaces") {
        p("The framework ships two UI DSLs. They produce the same output. The choice \
           is purely stylistic."),
    },

    section(heading = "ui!") {
        code(rust, r##"
            ui! {
                view(style = card_style()) {
                    text { "Hello" }
                    button(label = "Click me", on_click = move || println!("click"))

                    if logged_in.get() {
                        text { "Welcome back!" }
                    } else {
                        text { "Please log in." }
                    }

                    for item in items.iter() {
                        text { item.name.clone() }
                    }
                }
            }
        "##),
    },

    section(heading = "jsx!") {
        code(rust, r##"
            jsx! {
                <view style={card_style()}>
                    <text>"Hello"</text>
                    <button label="Click me" on_click={move || println!("click")} />

                    if logged_in.get() {
                        <text>"Welcome back!"</text>
                    } else {
                        <text>"Please log in."</text>
                    }

                    for item in items.iter() {
                        <text>{item.name.clone()}</text>
                    }
                </view>
            }
        "##),

        p("Both produce identical output. You can mix them in the same file — the \
           choice is per-call-site."),

        p("The mechanical differences are minor:"),
        list(
            [code("ui!"), ": parens for props, braces for children. ",
             code("style = expr"), ", ", code("text { \"hi\" }"), ", ",
             code("Card(variant = card::variant::Outline) { Counter() }"), "."],
            [code("jsx!"), ": angle brackets. String attrs are bare (",
             code("label=\"x\""), "), expression attrs are braced (",
             code("value={signal}"), "). Closing tags must match (",
             code("<Foo>...</Foo>"), "); ", code("</>"),
             " is not supported. Text content goes through a ", code("<text>"),
             " wrapper."],
            ["Both: primitives are snake_case (", code("view"), ", ",
             code("<view>"), "), components are PascalCase."],
        ),

        p("Pick whichever reads better to you."),
    },

    section(heading = "What ui! actually emits") {
        p(code("ui!"),
          " is syntax sugar — the macro parses tokens and emits ordinary Rust calls \
           into runtime-core's primitive constructors. You can write the same \
           component without the macro, and the framework can't tell the difference."),

        p("Here's the counter from above in three forms."),
    },

    section(heading = "With ui!") {
        code(rust, r##"
            #[component]
            pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
                let count = signal(initial);

                ui! {
                    view {
                        text { format!("Count: {}", count.get()) }
                        button(
                            label = "Increment",
                            on_click = move || count.update(|n| n + 1),
                        )
                    }
                }
            }
        "##),
    },

    section(heading = "With jsx!") {
        code(rust, r##"
            #[component]
            pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
                let count = signal(initial);

                jsx! {
                    <view>
                        <text>{format!("Count: {}", count.get())}</text>
                        <button
                            label="Increment"
                            on_click={move || count.update(|n| n + 1)}
                        />
                    </view>
                }
            }
        "##),
    },

    section(heading = "With no macro at all") {
        code(rust, r##"
            use runtime_core::{button, component, signal, text, view, Element, IntoElement};

            #[component]
            pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
                let count = signal(initial);

                // Builders outside a macro do need `.into_element()`;
                // `ui!` / `jsx!` output is already an `Element`.
                view(vec![
                    text(move || format!("Count: {}", count.get())).into_element(),
                    button("Increment", move || count.update(|n| n + 1)).into_element(),
                ])
                .into_element()
            }
        "##),

        p("All three build the same tree. The third form shows that primitives \
           are just functions — you can drop the DSL, write your tree out of plain \
           calls, and the result is the same. You might do this where the DSL gets \
           in the way (highly procedural generation), or just to see what the macro \
           produces when debugging."),
    },

    section(heading = "The pieces being emitted") {
        p("Looking at the no-macro form, you can see what ", code("ui!"), " does:"),
        list(
            [code("view { ... }"), " → ", code("view(vec![...])"), ". The ",
             code("view"), " constructor takes a ", code("Vec<Element>"),
             ". The primitive constructors (", code("text"), ", ", code("button"),
             ", etc.) return their own builder types, so each child is coerced to an ",
             code("Element"), " before joining the vec."],
            [code("text { format!(\"...\", count.get()) }"),
             " → a reactive text, ", code("text(move || format!(...))"),
             ". A literal ", code("text { \"hi\" }"), " emits the static form, ",
             code("text(\"hi\")"), "."],
            [code("button(label = \"...\", on_click = ...)"), " → ",
             code("button(label, on_click)"),
             ". Both arguments go through the framework's coercion traits (",
             code("TextContent"), ", ", code("IntoAction"), ")."],
            ["A component tag ", code("Counter(initial = 0)"), " → ",
             code("BuildElement::build(Counter { initial: (0).into(), ..defaults() })"),
             "."],
            ["The macro coerces its whole output to ", code("Element"),
             ", so a ", code("ui!"), " block is the function's return value as is."],
        ),
    },

    section(heading = "Reactive if") {
        code(rust, r##"
            ui! {
                if count.get() > 0 {
                    text { "positive" }
                } else {
                    text { "zero or negative" }
                }
            }
        "##),

        p("…lowers to:"),

        code(rust, r##"
            when(
                move || count.get() > 0,
                move || text("positive"),
                move || text("zero or negative"),
            )
        "##),

        p(code("when"), " is the reactive conditional. The macro uses it for any \
           condition that might read a signal — a method call, a function call, or a ",
          code(".get()"), " anywhere in it. Only a condition that provably reads no \
           signal lowers to a plain Rust ", code("if"),
          ". To force a plain ", code("if"), ", read with ", code(".peek()"),
          " or compute the condition in a ", code("let"), " above the block."),
    },

    section(heading = "Reactive for") {
        code(rust, r##"
            ui! {
                for todo in todos, key = todo.id {
                    text { todo.title.clone() }
                }
            }
        "##),

        p("The iterator's type picks the lowering:"),
        list(
            ["A ", code("Signal<Vec<T>>"), " (or another signal of an iterable) → a keyed \
              reactive list. The ", code("key = …"),
             " clause is required; rows whose key survives an update keep their state."],
            ["Any other ", code("IntoIterator"), " (a ", code("Vec"),
             ", a slice, a map) → built once, no key needed."],
            ["A range whose bound reads a signal (", code("for i in 0..n.get()"),
             ") → a reactive list keyed by index."],
            ["A static range with a single-node body (", code("for i in 0..n"),
             ") → one batched repeat."],
        ),
    },

    section(heading = "Bringing your own front-end") {
        p(code("ui!"), " and ", code("jsx!"),
          " are sugar over the same set of runtime-core calls. Nothing about the \
           framework privileges either one: a third macro that emits the same calls \
           would slot in alongside them."),

        p("Building one means writing a ", code("proc_macro"),
          " that emits the shapes shown in the \"What ", code("ui!"),
          " actually emits\" section above — ", code("view(...)"), ", ",
          code("text(...)"), ", ", code("button(...)"), ", ", code("when(...)"),
          ", and ", code("BuildElement::build(Foo { … })"),
          " for component tags. The ", code("ui!"), " and ", code("jsx!"),
          " sources in ", code("crates/runtime/macros-expand/src/"),
          " are the working references."),
    },

    section(heading = "Recap: a finished component") {
        p("Tying everything together:"),

        code(rust, r##"
            use runtime_core::{component, signal, ui, Element};

            #[component]
            pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
                let value = signal(initial);

                #[method]
                fn reset() { value.set(0); }

                #[method]
                fn bump_by(n: i32) { value.update(|v| v + n); }

                ui! {
                    view {
                        text { format!("Count: {}", value.get()) }
                        button(label = "++", on_click = move || value.update(|n| n + 1))
                    }
                }
            }
        "##),

        p("What's happening here:"),
        list(
            [code("#[component]"), " turns ", code("Counter"),
             " into a tag and generates ", code("CounterProps"),
             " with a default of ", code("0"), " for ", code("initial"), "."],
            [code("signal(initial)"), " allocates a reactive state slot owned by the \
              component's scope."],
            [code("#[method]"), " declares ", code("reset"), " and ", code("bump_by"),
             ". The macro generates ", code("CounterHandle"), " and adds the ",
             code("bind_to"), " prop."],
            [code("ui!"),
             " lowers to plain runtime-core calls, with the text bound reactively to ",
             code("value"), "."],
        ),

        p("The parent calls this with:"),

        code(rust, r##"
            let counter = Ref::<CounterHandle>::new();
            ui! {
                Counter(bind_to = counter)
                button(label = "Reset", on_click = move || { counter.get().map(|c| c.reset()); })
            }
        "##),

        p("— ", code("Counter()"), " reads as a constructor, ", code("bind_to"),
          " fills the parent's ref when the component builds, and ",
          code("counter.get().map(|c| c.reset())"), " later fires the method."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Reactivity", to = "reactivity"),
             " — signals, effects, the substrate components run on."],
            [link("The UI DSL", to = "ui-dsl"), " — the full ", code("ui!"),
             " grammar, including styles, control flow, and trailing method chains."],
            [link("Refs", to = "refs"), " — ", code("Ref<H>"),
             " and the surface for built-in handles plus user-component handles via ",
             code("#[method]"), "."],
            [link("Dev tools", to = "dev-tools"), " — hot reload and what ",
             code("#[component]"), " does to make each function swappable at runtime."],
        ),
    },
}
