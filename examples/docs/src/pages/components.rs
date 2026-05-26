//! Components page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{code_block, page_header, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{typography, card, stack};

docs! {
    slug = "components",
    title = "Components",
    category = Foundation,
    description = "Rust functions, wrapped in compile-time machinery, that compose into trees.",
    related = ["reactivity", "ui-dsl", "refs", "styles"],
    concepts = [Component, ComponentMethods, Bindable, Bound, Props, Defaults, UiMacro, JsxMacro],

    section(heading = "Overview") {
        p("A component is a Rust function that returns a ", code("Primitive"),
          ". The framework wraps that function in some compile-time machinery so it \
           can be called from a DSL, reuse a stable identity for hot reload, expose \
           imperative methods, and rewrite reactive call sites for ergonomics. This \
           page covers all of that."),
    },

    section(heading = "The shape") {
        code(rust, r##"
            use runtime_core::{component, signal, ui, Primitive};

            pub struct CounterProps {
                pub initial: i32,
            }

            #[component]
            pub fn counter(props: &CounterProps) -> Primitive {
                let count = signal!(props.initial);

                ui! {
                    View {
                        Text { format!("Count: {}", count.get()) }
                        Button(
                            label = "Increment",
                            on_click = move || count.update(|n| *n += 1),
                        )
                    }
                }
            }
        "##),

        p("Three rules cover the basic shape:"),
        list(
            ["Annotate the function with ", code("#[component]"),
             ". This is what generates the per-component invocation macro, handles hot \
              reload, and rewrites the body for reactivity."],
            ["Take one parameter, by reference: ", code("props: &MyProps"),
             ". The props struct is a regular Rust struct you declare next to (or above) \
              the function. Field names become prop names in the invocation macro."],
            ["Return ", code("Primitive"), ". The framework wraps the returned value \
              through ", code("IntoPrimitive::into_primitive(...)"), ", so you can return \
              a bare ", code("Primitive"), ", a ", code("Bound<H>"), " from a primitive \
              constructor, or anything else that implements ", code("IntoPrimitive"), "."],
        ),
    },

    section(heading = "Calling a component") {
        p("The macro generates two ways to invoke a ", code("#[component]"), " function:"),

        code(rust, r##"
            // Inside a ui! / jsx! block (the normal case):
            ui! {
                counter(initial = 0)
            }

            // Directly, as a plain Rust call:
            let prim: Primitive = counter(&CounterProps { initial: 0 });
        "##),

        p("Inside a DSL, the call site reads like a constructor. Outside, it's a \
           function call with a struct-literal props argument. They produce the same ",
          code("Primitive"), "."),
    },

    section(heading = "Variants of the signature") {
        p("The shape above is the common case. Three legitimate variants:"),
        list(
            ["No props: ", code("pub fn header() -> Primitive"),
             ". The invocation macro accepts ", code("Header()"), " with no arguments."],
            ["By value: ", code("pub fn list_view(props: MyProps) -> Primitive"),
             ". Used when the component needs to take ownership of something in ",
             code("props"), " — typically a ", code("Vec<Primitive>"),
             " of children it consumes. The macro detects this and emits the right \
              ownership form."],
            ["Bindable return: ", code("pub fn counter(props: &Props) -> Bindable<CounterHandle>"),
             ". Used when the component exposes a ", code("methods!"),
             " block (see below). The DSL coerces it back to a ", code("Primitive"),
             " automatically."],
        ),
    },

    section(heading = "Defaults") {
        p("If you want some props to default to a value when the caller omits them, \
           declare them on the attribute:"),

        code(rust, r##"
            #[component(default(initial = 0, step = 1))]
            pub fn counter(props: &CounterProps) -> Primitive {
                // ...
            }
        "##),

        p("The invocation macro then accepts the call without those props (",
          code("counter()"), " or ", code("counter(initial = 5)"),
          "), filling in the defaults at the call site. Defaults are evaluated per \
           call, not once at component definition, so an expression like ",
          code("default(now = std::time::Instant::now())"), " does what you'd expect."),
    },

    section(heading = "Sub-macros at a glance") {
        p("Seven macros come with the framework. Each does one thing:"),
        list(
            [code("#[component]"),
             " — wraps a function as a component. Generates the invocation macro, \
              handles reactivity rewriting and hot reload. Covered on this page."],
            [code("signal!(value)"), " — shorthand for ", code("Signal::new(value)"),
             ". Covered in Reactivity."],
            [code("effect!({ … })"),
             " — shorthand for an ", code("Effect::new(...)"),
             " bound to the surrounding scope. Covered in Reactivity."],
            [code("children![ … ]"),
             " — builds a ", code("Vec<Primitive>"),
             " from a mixed-shape list (single primitives, ",
             code("Option<Primitive>"), ", ", code("Vec<Primitive>"),
             "). Used to assemble children outside ", code("ui!"), "."],
            [code("ui! { … }"),
             " — the primary UI DSL. Lowers to plain runtime-core calls. Covered \
              below and on the UI DSL page."],
            [code("jsx! { … }"),
             " — a JSX-flavored variant of ", code("ui!"),
             " with identical output. Covered below."],
            [code("stylesheet! { … }"), " — declares a themed stylesheet. Covered in Styles."],
        ),
        p(code("methods! { … }"),
          " is a sub-form recognized inside a ", code("#[component]"),
          " body — it declares imperative methods exposed through a handle. \
           Covered below."),
        p("Everything else in the framework is a plain function or type — no hidden \
           macro magic."),
    },

    section(heading = "methods! — imperative handles") {
        p("Sometimes a parent component needs to trigger an imperative action on a \
           child — focus an input, reset a counter, scroll a list to the top. The \
           reactive substrate isn't the right tool for \"do something now\"; an \
           imperative handle is."),

        p("You declare methods inside the component's body:"),

        code(rust, r##"
            use runtime_core::{component, signal, ui, Bindable, Primitive};

            #[derive(Default)]
            pub struct CounterProps {
                pub initial: i32,
            }

            #[component]
            pub fn counter(props: &CounterProps) -> Bindable<CounterHandle> {
                let value = signal!(props.initial);

                methods! {
                    fn reset(&self) {
                        value.set(0);
                    }
                    fn bump_by(&self, n: i32) {
                        value.update(|v| *v += n);
                    }
                }

                ui! {
                    View {
                        Text { format!("Count: {}", value.get()) }
                    }
                }
            }
        "##),

        p("The macro generates a ", code("CounterHandle"), " struct with ",
          code("reset"), " and ", code("bump_by"),
          " methods. The component now returns ", code("Bindable<CounterHandle>"),
          " instead of ", code("Primitive"), "."),

        p("The parent captures the handle via a ", code("Ref"), ":"),

        code(rust, r##"
            use runtime_core::Ref;

            #[component]
            pub fn parent_app() -> Primitive {
                let handle: Ref<CounterHandle> = Ref::new();

                ui! {
                    counter(initial = 0).bind(handle)

                    Button(
                        label = "Reset",
                        on_click = move || { handle.with(|h| h.reset()); },
                    )
                }
            }
        "##),

        p("A few rules:"),
        list(
            ["A component may have at most one ", code("methods!"), " block."],
            ["Each method takes ", code("&self"),
             " (cosmetic — captures come from the closure, not from struct fields) \
              plus zero or more typed parameters."],
            ["Method bodies return ", code("()"),
             ". (Returning a value from a method is a v1 limitation.)"],
            ["The handle name is derived from the function name: ", code("counter"),
             " → ", code("CounterHandle"),
             ". The macro converts snake_case to PascalCase and appends ", code("Handle"), "."],
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
                View(style = card_style()) {
                    Text { "Hello" }
                    Button(label = "Click me", on_click = move || println!("click"))

                    if logged_in.get() {
                        Text { "Welcome back!" }
                    } else {
                        Text { "Please log in." }
                    }

                    for item in items.iter() {
                        Text { item.name.clone() }
                    }
                }
            }
        "##),
    },

    section(heading = "jsx!") {
        code(rust, r##"
            jsx! {
                <View style={card_style()}>
                    <Text>"Hello"</Text>
                    <Button label="Click me" on_click={move || println!("click")} />

                    if logged_in.get() {
                        <Text>"Welcome back!"</Text>
                    } else {
                        <Text>"Please log in."</Text>
                    }

                    for item in items.iter() {
                        <Text>{item.name.clone()}</Text>
                    }
                </View>
            }
        "##),

        p("Both produce identical output. You can mix them in the same file — the \
           choice is per-call-site."),

        p("The mechanical differences are minor:"),
        list(
            [code("ui!"), ": parens for props, braces for children. ",
             code("style = expr"), ", ", code("Text { \"hi\" }"), ", ",
             code("Card(kind = Outlined) { Counter() }"), "."],
            [code("jsx!"), ": angle brackets. String attrs are bare (",
             code("label=\"x\""), "), expression attrs are braced (",
             code("value={signal}"), "). Closing tags must match (",
             code("<Foo>...</Foo>"), "); ", code("</>"),
             " is not supported. Text content goes through a ", code("Text"), " wrapper."],
        ),

        p("Pick whichever reads better to you."),
    },

    section(heading = "What ui! actually emits") {
        p("This is where it gets fun. ", code("ui!"),
          " is syntax sugar — the macro parses tokens and emits ordinary Rust calls \
           into runtime-core's primitive constructors. You can write the same \
           component without the macro, and the framework can't tell the difference."),

        p("Here's the counter from above in three forms."),
    },

    section(heading = "With ui!") {
        code(rust, r##"
            #[component]
            pub fn counter(props: &CounterProps) -> Primitive {
                let count = signal!(props.initial);

                ui! {
                    View {
                        Text { format!("Count: {}", count.get()) }
                        Button(
                            label = "Increment",
                            on_click = move || count.update(|n| *n += 1),
                        )
                    }
                }
            }
        "##),
    },

    section(heading = "With jsx!") {
        code(rust, r##"
            #[component]
            pub fn counter(props: &CounterProps) -> Primitive {
                let count = signal!(props.initial);

                jsx! {
                    <View>
                        <Text>{format!("Count: {}", count.get())}</Text>
                        <Button
                            label="Increment"
                            on_click={move || count.update(|n| *n += 1)}
                        />
                    </View>
                }
            }
        "##),
    },

    section(heading = "With no macro at all") {
        code(rust, r##"
            use runtime_core::{button, component, signal, text, view, IntoPrimitive, Primitive};

            #[component]
            pub fn counter(props: &CounterProps) -> Primitive {
                let count = signal!(props.initial);

                view(vec![
                    text(move || format!("Count: {}", count.get())).into_primitive(),
                    button("Increment", move || count.update(|n| *n += 1)).into_primitive(),
                ])
                .into_primitive()
            }
        "##),

        p("All three compile to the same code. The third form makes the \
           \"primitives are just functions\" claim concrete — you can drop the DSL \
           entirely, write your tree out of plain function calls, and the result is \
           indistinguishable. You might do this in places where the DSL gets in the \
           way (highly procedural generation, programmatic trees built from data), or \
           just to read what the macro is producing when debugging."),
    },

    section(heading = "The pieces being emitted") {
        p("Looking at the no-macro form, you can see what ", code("ui!"), " does:"),
        list(
            [code("View { ... }"), " → ", code("view(vec![...])"), ". The ",
             code("view"), " constructor takes a ", code("Vec<Primitive>"),
             ". The primitive constructors (", code("text"), ", ", code("button"),
             ", etc.) return ", code("Bound<H>"),
             " handles, so each child is coerced to a ", code("Primitive"), " via ",
             code(".into_primitive()"), " before joining the vec."],
            [code("Text { format!(\"...\", count.get()) }"),
             " → because the expression contains ", code(".get()"),
             ", the macro emits a reactive text: ", code("text(move || format!(...))"),
             ". A static ", code("Text { \"hi\" }"), " emits the non-reactive form: ",
             code("text(\"hi\")"), "."],
            [code("Button(label = \"...\", on_click = ...)"), " → ",
             code("button(label, on_click)"),
             ". Both arguments go through the framework's coercion traits (",
             code("IntoTextSource"), ", ", code("IntoAction"), ")."],
            ["The trailing coercion — ", code(".into_primitive()"), " on the outer ",
             code("view(...)"), " returns a ", code("Primitive"),
             ", which is what the function signature expects. The macro adds this \
              coercion automatically when the function's return type is ",
             code("Primitive"), "."],
        ),
    },

    section(heading = "Reactive if") {
        code(rust, r##"
            ui! {
                if count.get() > 0 {
                    Text { "positive" }
                } else {
                    Text { "zero or negative" }
                }
            }
        "##),

        p("…lowers to:"),

        code(rust, r##"
            when(
                Derived { /* ...captures count, evaluates count.get() > 0... */ },
                || text("positive"),
                || text("zero or negative"),
            )
        "##),

        p(code("when"), " is the reactive conditional primitive. The macro picks it \
           up when the ", code("if"), "'s condition contains ", code(".get()"),
          "; a plain boolean condition lowers to a regular Rust ", code("if"),
          " (the branch is decided once at construction)."),
    },

    section(heading = "Reactive for") {
        code(rust, r##"
            ui! {
                for item in items {
                    Text { item.name.clone() }
                }
            }
        "##),

        p("…lowers to a ", code("Repeat"), " primitive or a regular ",
          code("Vec<Primitive>"),
          " build, depending on whether the iterator is signal-backed. The macro \
           takes care of the dispatch."),
    },

    section(heading = "Bringing your own front-end") {
        p(code("ui!"), " and ", code("jsx!"),
          " are sugar over the same set of runtime-core calls. Nothing about the \
           framework privileges either one: a third macro that emits the same calls \
           would slot in alongside them."),

        p("Building one today means writing a ", code("proc_macro"),
          " that emits the shapes shown in the \"What ", code("ui!"),
          " actually emits\" section above — ", code("view(...)"), ", ",
          code("text(...)"), ", ", code("button(...)"), ", ", code("when(...)"),
          ", per-component ", code("name!(...)"), " invocations, and a final ",
          code(".into_primitive()"),
          " coercion. That's all that's required, and it's all there is — but it \
           does mean parsing tokens and emitting them by hand."),

        p("Tooling to make this easier (so you can describe a DSL's shape \
           declaratively rather than write a proc-macro from scratch) is on the \
           roadmap. Until that lands, the existing ", code("ui!"), " and ",
          code("jsx!"), " sources in ", code("crates/framework/macros/src/"),
          " are the working references."),
    },

    section(heading = "Recap: a finished component") {
        p("Tying everything together:"),

        code(rust, r##"
            use runtime_core::{component, signal, ui, Bindable, Primitive};

            #[derive(Default)]
            pub struct CounterProps {
                pub initial: i32,
            }

            #[component(default(initial = 0))]
            pub fn counter(props: &CounterProps) -> Bindable<CounterHandle> {
                let value = signal!(props.initial);

                methods! {
                    fn reset(&self) { value.set(0); }
                    fn bump_by(&self, n: i32) { value.update(|v| *v += n); }
                }

                ui! {
                    View {
                        Text { format!("Count: {}", value.get()) }
                        Button(label = "++", on_click = move || value.update(|n| *n += 1))
                    }
                }
            }
        "##),

        p("What's happening here:"),
        list(
            [code("#[component(default(initial = 0))]"),
             " registers the function as a component and declares a default for ",
             code("initial"), "."],
            [code("signal!"), " allocates a reactive state slot."],
            [code("methods!"), " declares ", code("reset"), " and ", code("bump_by"),
             " as imperative operations. The macro generates ", code("CounterHandle"),
             " and rewrites the return type from ", code("Primitive"), " to ",
             code("Bindable<CounterHandle>"), "."],
            [code("ui!"),
             " lowers to plain runtime-core calls, with the reactive text being \
              wrapped in an Effect and the trailing value coerced to ",
             code("Primitive"), "."],
        ),

        p("The parent calls this with:"),

        code(rust, r##"
            let handle = Ref::<CounterHandle>::new();
            ui! {
                counter().bind(handle)
                Button(label = "Reset", on_click = move || { handle.with(|h| h.reset()); })
            }
        "##),

        p("— and ", code("counter()"), " reads as a constructor, ", code("bind"),
          " attaches the handle to the parent's ref, and ",
          code("handle.with(|h| h.reset())"), " later fires the method."),
    },

    section(heading = "Where to read more") {
        list(
            ["Reactivity — signals, effects, the substrate components run on."],
            ["The UI DSL — the full ", code("ui!"),
             " grammar, including styles, control flow, refs, and the \
              trailing-method escape hatch."],
            ["Refs — ", code("Ref<H>"),
             " and the surface for built-in handles plus user-component handles via ",
             code("methods!"), "."],
            ["Hot reload — what ", code("#[component]"),
             " does to make each function swappable at runtime."],
            ["Building your own DSL — a worked example of a third front-end macro."],
        ),
    },
}
