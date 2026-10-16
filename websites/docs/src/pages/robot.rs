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
           running Idealyst app — find a button by its test id, tap it, type \
           into a field, read a toggle's state, call a function the app \
           exposes for tests, invoke a method on a mounted component. It's the \
           framework's answer to \"how do I test the app, or automate it, or \
           let an AI poke at it\" without writing a per-platform test harness."),
        p("Every backend answers the same verbs, so one Playwright-style suite \
           runs unchanged on web, macOS, iOS, Android, Linux and Windows. A \
           test that passes on one platform and fails on another has found a \
           real difference between the backends."),
    },

    section(heading = "Enabling Robot") {
        p("Every dev build carries it: ", code("idealyst dev"), " and ",
          code("idealyst test"), " turn on the framework's ", code("robot"),
          " features themselves. Release builds carry none of it, and an app \
           declares no feature of its own."),

        p("Everything an app writes to be testable — a ", code("test_id"),
          ", ", code("robot::watch_signal"), ", a ", code("#[robot_fn]"),
          " registration, ", code("robot::is_test_driven()"),
          " — compiles in every build and does nothing outside a robot build, \
           so it needs no ", code("cfg"), ". (An app-side ",
          code("#[cfg(feature = \"robot\")]"), " would be off in dev builds \
           too: the CLI turns on the framework's feature, not the app's.)"),

        p("A robot build exposes a TCP bridge (port ", code("9718"),
          " by default) and registers itself under ", code("~/.idealyst/apps/"),
          ". The typical consumers are ", code("idealyst test"), " and the ",
          code("idealyst mcp"), " server — see the Dev tools page for wiring \
           it into an MCP client."),
    },

    section(heading = "Test IDs") {
        p("Every element can carry a ", code("test_id"), ":"),

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
           element kind (", code("Button"), ", ", code("Text"), ", ",
          code("TextInput"), ", …), so test IDs aren't strictly required. They \
           just give you the most reliable hook. Every element reports ",
          code("visible"), ", ", code("in_viewport"), " and its own ",
          code("state"), " — a field's text, a toggle's checked state, ",
          code("disabled"), ", ", code("focused"), ", a scroller's offset — so \
           most assertions need nothing from the app."),
    },

    section(heading = "The control surface") {
        p("Robot exposes six kinds of operations: query, interact, functions, \
           geometry, logs, and component methods."),
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
            [code("click"), " — press an element inside the framework: a button, pressable or link runs its press, and a view's ", code("on_touch"), " gets a synthetic tap. A tap on a Text inside a clickable row reaches the row. It skips the OS event path, so overlays and hit testing don't take part."],
            [code("tap_native"), " — tap the element's center with real platform input, through the toolkit's own event path: hit testing, overlays, gesture recognizers and the native disabled state all take part. It refuses the way a finger would fail — the element isn't visible, its center is outside the window, or something covers it (the error names what's on top)."],
            [code("drag"), " — move one pointer along ", code("points"), " (", code("[x, y]"), " in px from the element's top-left): ", code("Began"), ", ", code("Moved"), "…, ", code("Ended"), " into the element's ", code("on_touch"), " or the nearest ancestor's. Draws on a signature pad, drives a pan or drag-to-reorder. On a Slider each point's x along the track sets the value. Pass ", code("hold_ms"), " to press and hold at the first point before moving: drag-and-drop on phones and tablets picks up after a 200 ms hold. ", code("drag_native"), " does the same with real input."],
            [code("type_text"), " — replace a TextInput's value with new text."],
            [code("set_toggle"), " — set a Toggle's ", code("Signal<bool>"), "."],
            [code("set_slider"), " — set a Slider's ", code("Signal<f32>"), "."],
            [code("focus"), " / ", code("blur"), " — move keyboard focus in/out of an element."],
            [code("open_link"), " — deliver a deep link the way the OS would: observers, interceptors, then the navigators."],
        ),

        p("Each element action takes an ", code("element_id"), " from a prior \
           query, and a disabled control refuses it. The flow is always: find → \
           act → query → assert."),

        p("How ", code("tap_native"), " reaches the toolkit: DOM pointer and \
           mouse events on web, posted ", code("NSEvent"), "s on macOS, ",
          code("MotionEvent"), "s on Android, window messages on Windows. iOS \
           apps can't create touches, so there UIKit's hit test checks the \
           point and then the press runs. Linux answers ", code("unsupported"),
          " for now; use ", code("click"), "."),
    },

    section(heading = "Functions and test controls") {
        p("Some things a test needs have no element to act on: seed data, \
           fake a location, take the device offline, read what notifications \
           the app posted. The app registers them as functions, and a test \
           calls them by name:"),

        code(rust, r##"
            /// Fill the inbox with `count` sample messages.
            #[robot_fn(name = "demo.seed")]
            fn seed_inbox(count: u32) -> Result<Vec<String>, String> { /* … */ }

            // At boot, on the UI thread (no cfg):
            runtime_vocabulary::robot_fn::register(seed_inbox::robot());
        "##),

        list(
            [code("list_functions"), " — what is registered, with each function's doc comment and signature."],
            [code("call_function"), " — call one with a JSON object keyed by parameter name; the return value comes back as JSON. An ", code("async fn"), " answers at once and is polled with ", code("poll_function"), " until done."],
        ),

        p("The device SDKs ship fakes a test turns on the same way, so a test \
           assumes an answer instead of meeting a dialog it can't drive: ",
          code("location.set"), ", ", code("connectivity.set"), ", ",
          code("permissions.set"), ", ", code("notifications.log"),
          " — each registered with that SDK's ", code("register_robot_controls()"),
          ". The fakes compile only into robot builds."),
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
        p("The most interesting part for testing apps that use ", code("#[method]"), ":"),

        list(
            [code("list_components"), " — every mounted ", code("#[component]"),
             " instance that declared ", code("#[method]"), " fns. Returns ",
             code("{instance_id, fn_name, methods: [{name, args}]}"), "."],
            [code("invoke_method"), " — call one of those methods with a JSON args \
             object keyed by parameter name."],
        ),

        p("So if you have:"),

        code(rust, r##"
            #[component]
            pub fn Counter() -> Element {
                let value = signal(0);

                #[method]
                fn reset() { value.set(0); }
                #[method]
                fn bump_by(n: i32) { value.update(|v| v + n); }

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

        p("This is what makes ", code("#[method]"), " actually useful for testing: \
           the parent's view of a component's imperative surface becomes the \
           test's view too."),
    },

    section(heading = "Writing a suite") {
        p("Host-side tests live in ", code("tests/robot.rs"), " with ",
          code("robot-test"), " as a dev-dependency, and read like Playwright. \
           Every action waits for its element to exist and accept it, and every \
           assertion retries until it holds, so a test never sleeps:"),

        code(rust, r##"
            use robot_test::{json, robot_test, App};

            #[robot_test]
            fn increment_updates_count(app: &mut App) {
                app.call_function("demo.set_count", json!({ "value": 0 }));
                app.test_id("count-display").assert_text("Count: 0");
                app.test_id("increment-btn").click();
                app.test_id("increment-btn").click();
                app.test_id("count-display").assert_text("Count: 2");
            }
        "##),

        p(code("click()"), " here is native-first: it taps with real input \
           and falls back to the in-framework press only where the backend has \
           no native tier. Run the suite on any platform with ",
          code("idealyst test --web"), " (or ", code("--macos"), ", ",
          code("--ios"), ", ", code("--android"), ", ", code("--linux"), ", ",
          code("--windows"), "): it builds and launches the app, waits for it, \
           and runs ", code("cargo test --test robot"), " against it."),
    },

    section(heading = "Use cases") {
        p("Things people use Robot for:"),

        list(
            ["Automated UI tests. Write a test once that exercises the same \
              screen on every platform. The script doesn't change per platform."],
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

        p("The Robot MCP tools mirror the bridge surface above: ",
          code("find_element"), ", ", code("get_state"), ", ", code("click"),
          ", ", code("tap_native"), ", ", code("type_text"), ", ",
          code("list_functions"), " / ", code("call_function"), ", ",
          code("open_link"), ", ", code("get_snapshot"), ", ",
          code("list_component_instances"), " / ", code("invoke_method"),
          " (the MCP's ", code("list_components"), " is the static catalog), \
           and ", code("call_verb"), " for anything without its own tool. The \
           server's ", code("testing"), " guide walks an agent through writing \
           a suite."),
    },

    section(heading = "Constraints and notes") {
        list(
            ["Single thread. Robot reads from the thread the app's UI runs on; \
              each request is answered between frames, and staged reactive work \
              settles after every action."],
            ["The bridge is dev-mode. There's no auth on port ", code("9718"),
             ". Release builds compile none of it."],
            ["Native input varies by backend. ", code("tap_native"),
             " is real input on web, macOS, Android and Windows, a hit-test \
              check on iOS, and unsupported on Linux. Roku's Robot support is a \
              subset of the runtime backends' — geometry queries depend on what \
              the device-side runtime exposes."],
            ["Builtin primitives register themselves. A third-party primitive's \
              nodes show up once its mount handler calls ",
             code("robot_ext::register_extension_node"), "."],
        ),
    },

    section(heading = "Where to read more") {
        list(
            ["Dev tools — the MCP proxy in context, the bridge protocol on the \
              wire, how Robot fits with ", code("idealyst dev"), "."],
            ["Components — ", code("#[method]"), " fns, the source of ",
             code("list_components"), " / ", code("invoke_method"), " targets."],
            ["Primitives — the ", code("test_id"), " slot every primitive carries."],
            [code("docs/robot.md"), " in the repository — every verb, the native \
              input mechanism per backend, and the SDK test controls in full."],
        ),
    },
}
