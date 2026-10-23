//! Building UI — primitives and `ui!`, components, children, state and
//! events, conditions and lists. Each step centers on a live fiddle whose
//! starting code lives in `crate::fiddles`.

use idea_ui::{typography_kind, Typography};
use runtime_core::{ui, Element};

use crate::common::{Callout, DocsLink, LessonPage, ReactNote, workspace};
use crate::fiddle::Fiddle;
use crate::routes::{
    UI_CHILDREN_ROUTE, UI_COMPONENTS_ROUTE, UI_FLOW_ROUTE, UI_PRIMITIVES_ROUTE, UI_STATE_ROUTE, UI_REACTIVE_PROPS_ROUTE,
};

pub fn primitives() -> Element {
    ui! {
        LessonPage(
            current = UI_PRIMITIVES_ROUTE.name(),
            title = "Primitives & ui!".to_string(),
            lead = "The built-in leaves every interface is made of, and the macro that arranges them.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/building_ui/primitives.rs")) }),
            learn = vec![
                "How a ui! tree becomes native views",
                "The core primitives, and how to pass props and children",
                "Why primitives are lowercase and components PascalCase",
            ],
        ) {
            Typography(
                content = "A primitive is one of the framework's built-in elements: view, text, \
                    button, image, text_input, toggle, slider, scroll_view and a few more. Each one \
                    becomes the platform's own widget \u{2014} a <button> on the web, a UIButton on \
                    iOS, a Button on Android \u{2014} so your app looks and behaves native \
                    everywhere without you writing per-platform code.".to_string()
            )
            Typography(
                content = "You arrange primitives with ui!. Inside it, a primitive is written as its \
                    name, then its props in parentheses as name = value pairs, then its children in \
                    braces. Either part can be left out: text { \"Hi\" } has no props, \
                    button(label = \"Go\") has no children.".to_string()
            )

            Typography(content = "Reading the example".to_string(), kind = typography_kind::H2)
            Typography(
                content = "view is the container: it holds children and lays them out, by default \
                    in a column. text shows a string, and \"Hello, {name}!\" is an f-string \u{2014} \
                    {name} names a variable in scope, and because name is a signal the text updates \
                    as you type. The input primitives each take a value (the signal they show) and \
                    an on_change callback (where the user's edit arrives); writing the edit back \
                    with set is what makes them move.".to_string()
            )
            Typography(
                content = "Try it: add an image(src = \"https://picsum.photos/200\") inside the inner \
                    view, or a second button. Remove the on_change from the toggle and run it again \
                    \u{2014} the switch draws, but flipping it changes nothing, because nothing \
                    writes the signal.".to_string()
            )

            ReactNote {
                Typography(
                    content = "ui! plays the part of JSX, written as Rust: props are name = value pairs \
                        and children go in braces instead of between tags. The case rule is \
                        React's \u{2014} lowercase view is a built-in like <div>, PascalCase is a \
                        component like <MyButton> \u{2014} but a primitive renders the platform's \
                        own widget, not an HTML element.".to_string(),
                    muted = true,
                )
            }

            Callout(label = "Lowercase is a primitive, PascalCase is a component".to_string()) {
                Typography(
                    content = "The case of a tag is how ui! tells them apart: view is the built-in \
                        primitive, View would be a component someone defined. You'll write your \
                        first component in the next step.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Every primitive, its props, and how each backend renders it.".to_string(),
                link_label = "Primitives reference".to_string(),
                doc_file = "primitives.md".to_string(),
            )
        }
    }
}

pub fn components() -> Element {
    ui! {
        LessonPage(
            current = UI_COMPONENTS_ROUTE.name(),
            title = "Your first component".to_string(),
            lead = "Name a piece of UI once, give it props, and use it anywhere.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/building_ui/components.rs")) }),
            learn = vec![
                "Declare a component with #[component] and inline props",
                "Give a prop a default with #[prop(default = …)]",
                "Why each use of a component gets its own state",
            ],
        ) {
            Typography(
                content = "A component is a function that returns an Element, marked with \
                    #[component]. Its name is PascalCase and its parameters are its props. Once \
                    it's defined, you use it in ui! exactly like a primitive \u{2014} Greeting(name \
                    = \"world\") \u{2014} with props passed by name.".to_string()
            )

            Typography(content = "Reading the example".to_string(), kind = typography_kind::H2)
            Typography(
                content = "#[component] turns the parameter list into a props type behind the \
                    scenes, which is what lets a call site name its props in any order and leave \
                    some out. A prop marked #[prop(default = 1)] takes that value when the caller \
                    doesn't pass one; any other prop left out takes its type's default (an empty \
                    String, a zero).".to_string()
            )
            Typography(
                content = "Every prop is reactive. Pass a plain value and it is fixed; pass a \
                    signal and the component follows it \u{2014} that's why the third greeting \
                    changes as you type. Inside the component, a prop works in an f-string as-is \
                    (\"{name}\"); call .get() on it when you need the value in Rust code.".to_string()
            )
            Typography(
                content = "A component's own signals belong to that one instance. The two Counters \
                    each create a count when they're built, so pressing one leaves the other \
                    alone.".to_string()
            )
            Typography(
                content = "Try it: give Counter a second prop, step: i32 with #[prop(default = 1)], \
                    and use it in the update. Then pass step = 10 to one of them.".to_string()
            )

            ReactNote {
                Typography(
                    content = "A component function runs once, when it is built, and never again: there is \
                        no re-render. A parent doesn't re-pass props either. When a prop is a \
                        signal, only the text or style that reads it updates, so code you write in \
                        the body before ui! is setup, not something that runs per update.".to_string(),
                    muted = true,
                )
            }

            Callout(label = "Two ways to write props".to_string()) {
                Typography(
                    content = "Props as parameters is the everyday form. For a component that needs \
                        documented props in the catalog or a hand-written Default, declare a struct \
                        with #[props] and take props: &BadgeProps instead. Call sites can't tell \
                        the difference.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "What #[component] generates, and the explicit #[props] form.".to_string(),
                link_label = "The UI layer \u{2014} components".to_string(),
                doc_file = "ui-layer.md".to_string(),
            )
        }
    }
}

pub fn children() -> Element {
    ui! {
        LessonPage(
            current = UI_CHILDREN_ROUTE.name(),
            title = "Children & composition".to_string(),
            lead = "Components that wrap other UI, the way view does.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/building_ui/children.rs")) }),
            learn = vec![
                "Take a children prop and place it with a bare children",
                "Wrap one, several, or nested children",
                "Why children are built inside ui!, not pushed into a Vec",
            ],
        ) {
            Typography(
                content = "Some components hold other UI: a card, a section, a screen layout. Give \
                    the component a parameter named children of type Vec<Element>, and whatever a \
                    caller writes between the braces arrives there. In the body, write children on \
                    its own where the content should go.".to_string()
            )

            Typography(content = "Reading the example".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Card takes a title prop and its children, and renders the title above \
                    them inside a bordered view. The callers decide what goes in: one text, \
                    several mixed elements, or another Card. Because a component is used exactly \
                    like a primitive, nesting works with no extra rules \u{2014} that is \
                    composition.".to_string()
            )
            Typography(
                content = "children is written bare, with no braces around it. Inside ui!, a bare \
                    name in a child position means \"put these elements here\", which is how one \
                    component hands its content on to another.".to_string()
            )
            Typography(
                content = "Try it: add a footer: String prop to Card and render it after children. \
                    Or write a Row component that lays its children out horizontally \u{2014} \
                    you'll need flex_direction from the Styling track for that, so peek ahead.".to_string()
            )

            Callout(label = "Build children where they render".to_string()) {
                Typography(
                    content = "Write children inline inside ui!, as the example does, rather than \
                        pushing elements into a Vec and passing it along. Inside the macro, \
                        conditions and loops stay reactive and keyed; a hand-built Vec loses \
                        both. The next two steps cover that.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Components, children, and how both DSLs hand elements to a component.".to_string(),
                link_label = "The UI layer".to_string(),
                doc_file = "ui-layer.md".to_string(),
            )
        }
    }
}

pub fn state() -> Element {
    ui! {
        LessonPage(
            current = UI_STATE_ROUTE.name(),
            title = "State & events".to_string(),
            lead = "Signals hold what changes; handlers change it; the UI follows.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/building_ui/state.rs")) }),
            learn = vec![
                "Hold changing values in a signal",
                "Read with get, write with set and update from handlers",
                "Show live values with text f-strings",
                "Why a handler sees one moment",
            ],
        ) {
            Typography(
                content = "A signal is a value that can change over time. Create one with \
                    signal(initial), read it with .get(), and write it with .set(value) or \
                    .update(|current| next). Anywhere the UI reads a signal, it updates when the \
                    signal does \u{2014} just that text or that style, not the whole screen.".to_string()
            )

            Typography(content = "Reading the example".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Event props like on_click take a closure, and the closure writes \
                    signals. A move closure captures the signal itself, which is a small Copy \
                    handle, so the same signal can go into as many closures as you like.".to_string()
            )
            Typography(
                content = "There are two ways to show a signal. An f-string slot, \"Count: \
                    {count}\", names the signal and stays live by itself. A closure, text { move \
                    || ... }, can compute anything; it re-runs whenever a signal it read changes. \
                    The even/odd line reads count, so it re-runs on every count change, and only \
                    then.".to_string()
            )

            Typography(content = "A handler sees one moment".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Writes don't land the instant you make them: they take effect together \
                    when the handler finishes. Until then, .get() keeps returning the value from \
                    before the handler ran, so a handler always works from one consistent picture. \
                    The one place a pending write shows is update: its closure is handed the latest \
                    value, so the \"Add step twice\" button really adds twice. When the next value \
                    depends on the current one, reach for update.".to_string()
            )
            Typography(
                content = "Try it: add a Step 10 button, or a toggle that, when on, makes Reset set \
                    the count to 100 instead of 0.".to_string()
            )

            ReactNote {
                Typography(
                    content = "signal() is the counterpart of useState, without the re-render: setting it \
                        re-runs only the closures and text slots that read it. Writes batch like \
                        React 18's, committing when the handler returns, and nothing takes a \
                        dependency array \u{2014} whatever reads a signal is subscribed to it.".to_string(),
                    muted = true,
                )
            }

            Callout(label = "Why writes wait".to_string()) {
                Typography(
                    content = "Committing writes together means the screen never shows a half-applied \
                        update, and something reading several signals runs once per change, not once \
                        per write. The Reactivity track shows the mechanism, with live panels you can \
                        press.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Signals, the flush, effects and derived state in full.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    }
}

pub fn control_flow() -> Element {
    ui! {
        LessonPage(
            current = UI_FLOW_ROUTE.name(),
            title = "Conditions & lists".to_string(),
            lead = "Plain Rust if, match and for, inside ui!, and they keep up with your signals.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/building_ui/control_flow.rs")) }),
            learn = vec![
                "Branch with if and match inside ui!",
                "Render lists with for, keyed when they change",
                "How a keyed row is built once and kept",
            ],
        ) {
            Typography(
                content = "To show something only sometimes, or once per item in a list, write the \
                    Rust you already know inside ui!: if, if let, match and for. There's no separate \
                    template language. What the macro adds is reactivity: a condition that reads a \
                    signal is re-checked whenever the signal changes, and swaps what's on screen to \
                    match.".to_string()
            )

            Typography(content = "Conditions".to_string(), kind = typography_kind::H2)
            Typography(
                content = "if done.get() { text { \"(done)\" } } shows the label while the box is \
                    ticked. Because the condition calls .get(), ui! makes it live; an if over a plain \
                    bool is decided once, when the UI is built. match works the same way \u{2014} \
                    the count message at the bottom picks an arm from todos.get().len(), and the \
                    last arm binds the length to n for its f-string.".to_string()
            )

            Typography(content = "Lists".to_string(), kind = typography_kind::H2)
            Typography(
                content = "for over an ordinary Vec renders one copy of the body per item, once. for \
                    over a signal \u{2014} for todo in todos \u{2014} follows it: add an item and a \
                    row appears, remove one and it goes. A loop over a signal must say how to tell \
                    rows apart, with key = todo.id. When the list changes, rows whose key is still \
                    there are kept as they are, new keys get new rows, and missing keys are \
                    removed.".to_string()
            )
            Typography(
                content = "That is why ticking a todo survives adding another: each TodoRow keeps \
                    its own done signal for as long as its key stays in the list. It is also why a \
                    row doesn't redraw when its item changes under the same key \u{2014} anything a \
                    row shows that can change should be a signal the row reads.".to_string()
            )
            Typography(
                content = "Try it: add a \"Clear all\" button that sets todos to an empty Vec, and \
                    watch the match switch to its first arm.".to_string()
            )

            ReactNote {
                Typography(
                    content = "key = works like React's key prop, with one difference that matters: a row is \
                        built once per key and is never re-rendered with a new item. React would \
                        re-render the row with fresh props; here, the parts of a row that change \
                        must read signals.".to_string(),
                    muted = true,
                )
            }

            Callout(label = "Pick a key that belongs to the item".to_string()) {
                Typography(
                    content = "A key must be unique in the list and stay with its item: an id, not \
                        the item's position. Keying by position would hand a ticked box to whichever \
                        todo slid into that place after a removal.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Conditions and lists inside ui!, with the patterns around them.".to_string(),
                link_label = "Component patterns \u{2014} conditions and lists".to_string(),
                doc_file = "component-patterns.md".to_string(),
            )
        }
    }
}

pub fn reactive_props() -> Element {
    ui! {
        LessonPage(
            current = UI_REACTIVE_PROPS_ROUTE.name(),
            title = "Static vs reactive props".to_string(),
            lead = "Every prop can be live. Whether it stays live is up to the component.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/building_ui/reactive_props.rs")) }),
            learn = vec![
                "Why every data prop is Reactive<T>",
                "Pass a literal or a signal to the same prop",
                "Keep a prop live by routing it, not reading it once",
                "When to mark a prop #[prop(static)]",
            ],
        ) {
            Typography(
                content = "A component's props are reactive by default. #[component] wraps each one \
                    in Reactive<T>, which holds either a fixed value or a live one. The caller \
                    decides which: a literal like \"Grace\" arrives fixed, and a signal (or rx!(...), \
                    or a closure) arrives live. The component's code is the same either way.".to_string()
            )
            Typography(
                content = "Being able to change is only half of it. A prop updates the screen only \
                    if the component hands it to something that reads it again: a text, a style \
                    setter, a primitive's prop. Read it once with .get() while the component builds, \
                    and you have a copy of that moment \u{2014} it never changes again.".to_string()
            )

            Typography(content = "Reading the example".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The first NameTag gets the same signal for live and frozen. live goes \
                    straight into an f-string slot, \"Live: {live}\", which reads the prop each time \
                    it changes, so that line follows the box as you type. frozen is read with \
                    .get() in the component body. The body runs once, so snapshot holds whatever the \
                    signal said at that moment, and the Snapshot line never moves.".to_string()
            )
            Typography(
                content = "The second NameTag passes literals. Static values are the fast path: \
                    the text is built once, with no subscription and nothing left running.".to_string()
            )
            Typography(
                content = "Try it: change the snapshot line to read live instead of frozen, and \
                    run. It follows the box now. Then pass heading = name and run: the build \
                    fails, because a #[prop(static)] prop is a plain String and accepts no signal.".to_string()
            )

            Typography(content = "Choosing static".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Mark a prop #[prop(static)] when it can't sensibly change after the \
                    component is built \u{2014} an initial value, a layout choice made once \u{2014} \
                    or when its type isn't Clone. The prop is then the bare type: callers can't \
                    pass a signal, and the component can't forget to route it. #[prop(reactive)] \
                    goes the other way, for a type the default rules leave unwrapped. Handlers \
                    (Rc<dyn Fn()>), children (Vec<Element>) and signals themselves are never wrapped.".to_string()
            )

            ReactNote {
                Typography(
                    content = "In React a new prop value re-runs the component, so reading a prop \
                        anywhere in the body is fine. Here the body runs once: a prop read into a \
                        local at the top is a snapshot. Pass the prop itself to where it's shown.".to_string(),
                    muted = true,
                )
            }

            Callout(label = "Routing is the whole trick".to_string()) {
                Typography(
                    content = "When a value has to be computed from a prop, compute it where it's \
                        used \u{2014} text { move || format!(\"{} items\", count.get()) } \u{2014} \
                        not into a let before ui!. The closure reads the prop each time it runs, so \
                        the result stays live.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "How #[component] wraps props, and routing a reactive prop to its sink.".to_string(),
                link_label = "Implementing a component".to_string(),
                doc_file = "/crates/mcp/catalog/guides/idiomatic-components.md".to_string(),
            )
        }
    }
}
