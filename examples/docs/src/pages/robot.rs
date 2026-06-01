//! Robot page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "robot",
    title = "Robot",
    category = Tools,
    description = "An introspection layer that lets another process drive a running Idealyst app.",
    related = ["dev-tools", "components", "primitives"],
    concepts = [Robot, TestId, McpServer],

    section(heading = "Overview") {
        p("Robot is an introspection layer that lets another process drive a \
           running Idealyst app — list the components on screen, find a button \
           by its label, click it, read a TextInput's frame, invoke an \
           imperative method on a mounted component. It's the framework's \
           answer to \"how do I test the app, or automate it, or let an AI \
           poke at it\" without writing a per-platform test harness."),
        p("When Robot is on, every backend exposes the same query surface, and \
           the same script can drive the web build, the iOS app on a simulator, \
           or the Android app on a device. You write the test once; it runs \
           against whichever backend you happen to be looking at."),
    },

    section(heading = "Enabling Robot") {
        p("Robot is feature-gated on ", code("runtime-core"),
          ". Enable it from your app's ", code("Cargo.toml"),
          " by forwarding the feature flag:"),

        code(toml, r##"
            [features]
            default = ["robot"]
            robot = ["runtime-core/robot"]
        "##),

        p("Once the feature is on, the usual CLI commands pick it up — ",
          code("idealyst dev"), ", ", code("idealyst build --release"),
          ", and so on. No CLI flag is needed; the feature flows through Cargo \
           the way any other workspace feature does."),

        p("The app then exposes a TCP bridge on port ", code("9718"), " by default. \
           Any process that speaks the bridge protocol can connect and start \
           issuing commands. The typical consumer is the ", code("idealyst mcp"),
          " server bundled with the framework — see the Dev tools page for \
           how to wire it into Claude Code, Claude Desktop, or another MCP client."),

        p("You don't have to use the MCP proxy. The bridge protocol is small \
           enough that you can write your own client in a few hundred lines of \
           any language with a TCP and JSON parser."),

        p("When the ", code("robot"), " feature is off, every cost — the bridge \
           thread, the introspection metadata, the ", code("test_id"), " slots \
           on each primitive — compiles to nothing. There's no production overhead."),
    },

    section(heading = "Test IDs") {
        p("Every primitive accepts an optional ", code("test_id"), " when Robot is on:"),

        code(rust, r##"
            ui! {
                button(
                    label = "Submit",
                    on_click = move || submit(),
                    test_id = "submit-button",
                )
            }
        "##),

        p(code("test_id"), " is the stable name your tests refer to. It doesn't \
           change when you tweak a label or restructure a screen, which makes it \
           more durable than matching by visible text."),

        p("You can also find elements by label, by label substring, or by \
           primitive kind (", code("Button"), ", ", code("Text"), ", ",
          code("TextInput"), ", …), so test IDs aren't strictly required. They \
           just give you the most reliable hook."),
    },

    section(heading = "The control surface") {
        p("Robot exposes four kinds of operations: query, interact, geometry, \
           and introspect components."),
    },

    section(heading = "Query the tree") {
        list(
            [code("find_element"), " — first element matching ", code("test_id"),
             " / ", code("label"), " / ", code("label_contains"), " / ", code("kind"), "."],
            [code("find_all_elements"), " — every element matching the same criteria."],
            [code("get_snapshot"), " — the full component hierarchy as a tree of ",
             code("{id, kind, label, children}"), "."],
            [code("get_children"), " — direct children of an element."],
            [code("get_parent"), " — parent of an element."],
            [code("count_elements"), " — total mounted elements, optionally filtered by kind."],
        ),

        p("A typical scripted check is \"find by test_id, assert its children \
           look right.\" ", code("get_snapshot"), " is the heavy hammer when \
           you want to inspect the whole tree at once."),
    },

    section(heading = "Interact") {
        list(
            [code("click"), " — fire a button's or pressable's ", code("on_click"), "."],
            [code("type_text"), " — replace a TextInput's value with new text."],
            [code("set_toggle"), " — set a Toggle's ", code("Signal<bool>"), "."],
            [code("set_slider"), " — set a Slider's ", code("Signal<f32>"), "."],
            [code("focus"), " / ", code("blur"), " — move keyboard focus in/out of an element."],
        ),

        p("Each interact tool takes an ", code("element_id"), " from a prior \
           query. The flow is always: find → act → query → assert."),
    },

    section(heading = "Read geometry") {
        list(
            [code("get_frame"), " — bounding rect in the parent's coordinate system."],
            [code("get_absolute_frame"), " — bounding rect in viewport (window) coordinates."],
        ),

        p("Both return ", code("{x, y, width, height}"), " in pixels, or ",
          code("null"), " if the element exists but hasn't been laid out yet. \
           Use ", code("get_frame"), " to answer \"where is this relative to its \
           container\"; use ", code("get_absolute_frame"), " for \"where is this on screen\"."),
    },

    section(heading = "Inspect logs") {
        list(
            [code("get_logs"), " — captured log entries (framework, backend, stdout/stderr). \
             Each ", code("{ts, source, text}"), ". Supports ", code("since"), " for \
             polling, ", code("limit"), " for last-N."],
            [code("clear_logs"), " — drop the buffer. Useful before reproducing an issue."],
        ),

        p("Captured logs include ", code("eprintln!"), " from Rust, ", code("NSLog"),
          " from iOS, and the backend's own diagnostics. The buffer is a ring; \
           old entries are dropped if you don't drain."),
    },

    section(heading = "Drive component methods") {
        p("The most interesting part for testing apps that use ", code("methods!"), ":"),

        list(
            [code("list_components"), " — every mounted ", code("#[component]"),
             " instance that declared a ", code("methods!"), " block. Returns ",
             code("{instance_id, fn_name, methods: [{name, args}]}"), "."],
            [code("invoke_method"), " — call one of those methods with a JSON args \
             object keyed by parameter name."],
        ),

        p("So if you have:"),

        code(rust, r##"
            #[component]
            pub fn counter(props: &Props) -> Bindable<CounterHandle> {
                let value = signal!(0);
                methods! {
                    fn reset(&self) { value.set(0); }
                    fn bump_by(&self, n: i32) { value.update(|v| *v += n); }
                }
                // ...
            }
        "##),

        p("…a test script can call:"),

        code(jsonc, r##"
            // invoke_method
            {
                "instance_id": 12,
                "method": "bump_by",
                "args": { "n": 5 }
            }
        "##),

        p("…and the running app's counter increments by 5. Args JSON-deserialize \
           into the parameter types reported by ", code("list_components"), " — \
           anything ", code("serde"), " can decode works, including custom structs."),

        p("This is what makes ", code("methods!"), " actually useful for testing: \
           the parent's view of a component's imperative surface becomes the \
           test's view too."),
    },

    section(heading = "What a session looks like") {
        p("A test that bumps a counter and checks the result:"),

        code(jsonc, r##"
            // 1. Find the counter's display by test_id.
            { "tool": "find_element", "args": { "test_id": "count-display" } }
            // → { "element_id": 7, "kind": "Text", "label": "Count: 0" }

            // 2. Find the increment button.
            { "tool": "find_element", "args": { "test_id": "increment-btn" } }
            // → { "element_id": 9, "kind": "Button" }

            // 3. Click it twice.
            { "tool": "click", "args": { "element_id": 9 } }
            { "tool": "click", "args": { "element_id": 9 } }

            // 4. Re-read the display.
            { "tool": "find_element", "args": { "element_id": 7 } }
            // → { "element_id": 7, "kind": "Text", "label": "Count: 2" }
        "##),

        p("That's the entire shape of a Robot-driven test, modulo the JSON-RPC \
           envelope the bridge protocol wraps around it."),
    },

    section(heading = "Use cases") {
        p("Things people use Robot for:"),

        list(
            ["Automated UI tests. Write a test once that exercises the same \
              screen on web, iOS, and Android. The script doesn't change per platform."],
            ["Driving demos. Script a deterministic walkthrough that always \
              lands on the right screen with the right data."],
            ["Accessibility tooling. External processes can read the component \
              tree, frames, and labels — the building blocks of an alternate \
              input system."],
            ["Letting an LLM poke at the app. The MCP proxy bundled with the \
              framework was built for this case: connect Claude Desktop (or \
              another MCP client) to your running app, and the model can drive \
              it via natural language."],
        ),
    },

    section(heading = "The MCP server") {
        p(code("idealyst mcp"), " is the stdio MCP server that ships with the \
           framework. It speaks the Model Context Protocol on stdin/stdout and \
           the Robot bridge protocol over TCP. It finds a running app via the \
           per-process registration files under ", code("~/.idealyst/apps/"),
          ", or you can point it at a known bridge with ", code("--robot-port"),
          ". Any MCP client can then drive that app through it."),

        p("Wiring it into Claude Code or Claude Desktop is a config change \
           (", code("idealyst mcp install"), " writes this for you):"),

        code(json, r##"
            {
                "mcpServers": {
                    "idealyst": {
                        "command": "idealyst",
                        "args": ["mcp"]
                    }
                }
            }
        "##),

        p("…and the model gets every tool on this page as a callable function, \
           plus the static component catalog."),

        p("The full set of Robot MCP tools mirrors the bridge surface above: ",
          code("find_element"), ", ", code("click"), ", ",
          code("type_text"), ", ", code("get_snapshot"), ", ",
          code("invoke_method"), ", and so on."),
    },

    section(heading = "Constraints and notes") {
        list(
            ["Single thread, single arena. Robot reads from the same thread the \
              app runs on. Queries are synchronous and serialized with the \
              render walker."],
            ["The bridge is dev-mode. There's no auth on port ", code("9718"),
             ". Don't ship a Robot-enabled binary to end users. Leaving the \
             feature off (the default) is the production posture."],
            ["Generator backends are partial. Roku's Robot support is a subset \
              of what runtime backends provide — geometry queries in particular \
              depend on what the device-side runtime exposes. Web, iOS, and \
              Android have the full surface."],
            ["Custom primitives are visible. Anything that flows through the \
              standard ", code("Backend"), " create/insert/update calls shows \
              up in Robot's queries automatically. There's no separate \
              registration step for new primitives."],
        ),
    },

    section(heading = "Where to read more") {
        list(
            ["Dev tools — the MCP proxy in context, the bridge protocol on the \
              wire, how Robot fits with ", code("idealyst dev"), "."],
            ["Components — ", code("methods!"), " blocks, the source of ",
             code("list_components"), " / ", code("invoke_method"), " targets."],
            ["Primitives — the ", code("test_id"), " slot every primitive carries."],
        ),
    },
}
