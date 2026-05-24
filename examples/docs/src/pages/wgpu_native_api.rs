//! wgpu Native API page — built via the `docs!` macro.
//!
//! Documents the public surface a native shell uses to drive the
//! wgpu render backend: the four-crate layout, the `EventSink`
//! trait, the event vocabulary, `DeviceProfile` /
//! `DeviceProfile`, the redraw hook, and the minimal skeleton
//! for writing a new native shell or a new render backend on top
//! of the same contract.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{code_block, page_header, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "wgpu-native-api",
    title = "wgpu Native API",
    category = Advanced,
    description = "The public surface a native shell uses to drive the wgpu render backend.",
    related = ["backends", "writing-a-backend", "primitives"],
    concepts = [],

    section(heading = "Intro") {
        p("The wgpu preview stack isn't one crate — it's four layers, \
           deliberately. The split exists so a new host shell (a web \
           canvas shim, a UIKit shim, an Android NDK shim) can be \
           written without touching the renderer; a new render \
           backend (Skia, vello, a CPU rasterizer) can be slotted in \
           under the same shell; and a new platform skin can be added \
           without recompiling either. The layers communicate through \
           a small, frozen contract crate and a `Painter` trait."),
        p("The layout, under ", code("crates/"), ":"),
        code(text, r##"
            render/api              render-api     No wgpu, no winit. Pure contract types.
            render/wgpu             render-wgpu    Renderer + Painter trait + interaction host. Depends on api + wgpu.
            ios-sim                 ios-sim        iOS skin (UISwitch/Slider/TextField/spinner/keyboard).
            android-sim             android-sim    Android Material 3 skin.
            host/winit              host-winit     Winit shell. Depends on api + render-wgpu + winit + wgpu.
            native/{phone,tablet,tv}               Device-profile facades — pick a (host, renderer, skin, profile) tuple.
        "##),
        p("The contract crate is the seam. ", code("render-api"),
          " has no platform dependencies and no rendering dependencies — \
           it's just types (events, profile, simulated-platform enum) \
           plus the ", code("EventSink"), " trait. Any native shell \
           targets this crate; any render backend implements it."),
        p("This page documents that contract: what the trait promises, \
           what coordinate space events live in, and how a new shell or \
           a new render backend plugs in. If you only want to ship an \
           Idealyst app, you don't need this page — pick a variant \
           crate and call ", code("run(profile, app)"),
          ". This is the layer underneath that."),
    },

    section(heading = "The crates") {
        list(
            [code("render-api"), " — the contract. ",
             code("PointerEvent"), ", ", code("KeyEvent"), ", ",
             code("ScrollEvent"), ", ", code("DeviceProfile"),
             ", and the ", code("EventSink"),
             " trait. No wgpu, no winit, no platform-specific anything. \
              Every other layer depends on this; it depends on no one."],
            [code("render-wgpu"), " — the renderer + interaction host. \
              Implements ", code("runtime_core::Backend"), " (so the \
              framework can hand it a primitive tree) and ",
             code("EventSink"), " (so a shell can hand it events). Owns \
              the wgpu pipeline, the Taffy layout tree, the animator, \
              and the ", code("Painter"), " trait. Holds an ",
             code("Rc<dyn Painter>"), " for the lifetime of the host; \
              every widget and keyboard paint call routes through it."],
            [code("ios-sim"), " / ", code("android-sim"),
             " — concrete skins. Each implements ", code("Painter"),
             " with its palette + paint policy. Stateless unit \
              structs; instantiate once, wrap in ", code("Rc"),
             ". Adding a third skin is one new crate."],
            [code("host-winit"), " — the winit shell. Owns the \
              winit event loop and the wgpu surface. Translates ",
             code("winit::WindowEvent"), " values into the api crate's \
              event types and forwards them through ", code("EventSink"),
             ". Takes the skin as a constructor argument."],
            [code("variant-phone"), " / ", code("-tablet"), " / ",
             code("-tv"), " — variant facades. Each fixes a ",
             code("DeviceProfile"), " (window size, color scheme) and \
              calls ", code("host-winit::run(profile, skin, app)"),
             ". The caller passes the skin. No render logic, no event \
              translation — just configuration."],
        ),
        p("Any future host shell (web, iOS-native, Android-native) is \
           one new crate that does the same job as ", code("host-winit"),
          ". Any future render backend is one new crate that implements \
           the same two traits as ", code("render-wgpu"),
          ". Any future skin (Fluent, Roku, design-system-X) is one \
           new crate implementing ", code("Painter"),
          ". No central registry to update."),
    },

    section(heading = "The EventSink trait") {
        p("The full surface a native shell calls. Defined in ",
          code("render_api"), ":"),

        code(rust, r##"
            use std::time::Instant;

            pub trait EventSink {
                fn pointer_down(&mut self, ev: PointerEvent);
                fn pointer_move(&mut self, ev: PointerEvent);
                fn pointer_up(&mut self, ev: PointerEvent);
                fn pointer_cancel(&mut self);
                fn scroll(&mut self, ev: ScrollEvent);
                fn key(&mut self, ev: &KeyEvent) -> bool;
                fn set_viewport(&mut self, w: f32, h: f32);
                fn tick(&mut self, now: Instant) -> bool;
            }
        "##),

        p("Eight methods. That's the entire contract a shell drives. \
           Coordinates are always in logical CSS pixels — the shell \
           does the physical-to-logical conversion (divide by the \
           platform's scale factor, normalize wheel-line deltas to \
           pixels, etc.). The render side never sees platform-specific \
           units."),
        p("Per-method conventions:"),
        list(
            [code("pointer_down"), " / ", code("pointer_move"), " / ",
             code("pointer_up"), " — the lifecycle of one pointer \
              interaction. Mouse uses ", code("PointerId::MOUSE"),
             "; touch uses the OS-reported finger id (Apple's ",
             code("UITouch"), " identifier, browser's ", code("pointerId"),
             "). The id is stable for the duration of an interaction."],
            [code("pointer_cancel"), " — OS-level abort: window lost \
              focus, OS-interrupted touch, system gesture took over. \
              The render side treats any in-flight gesture as aborted \
              without firing release actions. Takes no event because \
              there's no meaningful position by the time it fires."],
            [code("scroll"), " — wheel or two-finger pan. ",
             code("delta"),
             " is in logical pixels per axis; the shell is responsible \
              for any unit conversion. ", code("position"), " is where \
              the pointer is at the moment of the event, so the render \
              side can decide which scroll container under the cursor \
              receives it."],
            [code("key"), " — keyboard input. Returns ",
             code("true"), " if the render side consumed the key (a \
              focused TextInput accepted the character, Backspace was \
              handled). Shells can use the return value to decide \
              whether to let the key propagate to platform shortcuts."],
            [code("set_viewport"), " — tell the render side how big \
              the viewport is, in logical CSS pixels. Called on \
              startup and on resize. The render side uses it for \
              layout and for positioning the on-screen keyboard \
              against the bottom edge."],
            [code("tick"), " — advance per-frame animation state \
              (tweens, momentum scroll, keyboard slide, caret blink). \
              Returns ", code("true"), " if anything is still in \
              flight — the shell should ", code("request_redraw"),
             " so the next frame samples the next step. Returns ",
             code("false"), " when everything has settled and the \
              shell can go back to waiting for input."],
        ),
        p("There's no platform-flavor escape hatch. If you can't \
           express your event as one of these, the vocabulary needs \
           widening in ", code("render-api"),
          " so every shell benefits — not a private side-channel."),
    },

    section(heading = "Event types") {
        p("The vocabulary lives in ", code("render_api::input"),
          ". The same types flow through every shell."),
    },

    section(heading = "PointerEvent") {
        code(rust, r##"
            pub struct PointerEvent {
                pub id: PointerId,
                pub button: PointerButton,
                pub position: (f32, f32),
            }

            pub struct PointerId(pub u64);
            impl PointerId {
                pub const MOUSE: PointerId = PointerId(0);
            }

            pub enum PointerButton {
                Primary,
                Secondary,
                Middle,
                Other(u16),
            }
        "##),
        p("Mouse uses ", code("PointerId::MOUSE"),
          "; touch shells pass the OS's finger id. Multi-touch isn't \
           wired into the renderer yet, but the field is there so we \
           don't have to reshape the API when it lands."),
        p("Touch always reports ", code("PointerButton::Primary"),
          ". The other variants exist for desktop pointers: right-click \
           opens context menus, middle-click pastes on X11, back/forward \
           map to ", code("Other(3)"), " / ", code("Other(4)"), "."),
    },

    section(heading = "KeyEvent") {
        code(rust, r##"
            pub struct KeyEvent {
                pub key: Key,
                pub text: Option<String>,
                pub modifiers: KeyModifiers,
                pub pressed: bool,
            }

            pub struct KeyModifiers {
                pub shift: bool,
                pub ctrl: bool,
                pub alt: bool,
                pub meta: bool,  // Command on macOS, Win key on Windows, Meta on X11
            }

            pub enum Key {
                Character,
                Backspace, Delete, Enter, Escape, Tab,
                ArrowLeft, ArrowRight, ArrowUp, ArrowDown,
                Home, End,
                Unknown,
            }
        "##),
        p("The split between ", code("key"), " and ", code("text"),
          " is the IME story. Character-producing keys arrive as ",
          code("Key::Character"), " with the actual text (after IME / \
           dead-key processing) in ", code("text"),
          ". Named keys carry their semantic identity in ", code("key"),
          " and typically have ", code("text: None"),
          ". The render side switches on ", code("key"),
          " when it cares about intent (Backspace deletes a glyph; \
           ArrowLeft moves the caret) and reads ", code("text"),
          " when it needs the characters to insert."),
        p(code("pressed"), " distinguishes key-down from key-up. \
           Shells that only emit one of the two (UIKit's ",
          code("pressesBegan"),
          " fires for both) can always set ", code("true"), "."),
        p("The ", code("Key"), " enum is open-ended in spirit: add \
           variants as more shells need them. The render side matches \
           exhaustively so a missing case fails loudly at compile time."),
    },

    section(heading = "ScrollEvent") {
        code(rust, r##"
            pub struct ScrollEvent {
                pub position: (f32, f32),
                pub delta: (f32, f32),
            }
        "##),
        p("One event for both mouse wheels and trackpad two-finger \
           pans. ", code("delta"),
          " is in logical CSS pixels — the shell converts wheel-lines \
           or physical-pixel deltas before dispatching. ",
          code("position"), " tells the render side which scroll \
           container under the cursor should receive the event."),
        p("Sign convention: positive ", code("delta.y"),
          " scrolls content up (reveals content below). The winit shell \
           inverts winit's native sign to match this; other shells \
           should normalize the same way."),
    },

    section(heading = "DeviceProfile and Painter") {
        p("Two values the variant crates compose and pass to the shell."),

        code(rust, r##"
            pub struct DeviceProfile {
                pub logical_size: (u32, u32),
                pub title: String,
                pub color_scheme: ColorScheme,
            }

            // From render-wgpu:
            pub trait Painter { /* paint methods + keyboard rows */ }
        "##),

        p(code("DeviceProfile"),
          " is the shape of the window — how big in logical pixels, \
           what title, what color scheme to report on init. The ",
          code("phone"), " / ", code("tablet"), " / ", code("tv"),
          " crates each carry a different profile; that's the entirety \
           of what a variant crate does on this front."),
        p("The platform look (UIKit vs Material 3 vs anything else) \
           is a separate axis: a ", code("Rc<dyn Painter>"), " passed \
           alongside the profile. The variant crates don't pick a \
           default — the caller does. That keeps the variant crate \
           ignorant of which skins exist and makes adding a new skin \
           a no-recompile change for the variants."),
    },

    section(heading = "The redraw hook") {
        p("The render side has to be able to wake the native event \
           loop. A signal flip inside an effect, a tween reaching the \
           next frame, an ", code("apply_style"),
          " call mid-build — any of these can require a fresh paint, \
           and the render side doesn't know which event loop it's \
           living inside."),
        p("The solution is a closure the shell installs at startup. ",
          code("render_wgpu"), " exposes it through ",
          code("install_redraw_hook"), ":"),

        code(rust, r##"
            use render_wgpu::install_redraw_hook;

            // Inside the shell's startup:
            let proxy = event_loop.create_proxy();
            install_redraw_hook(Box::new(move || {
                let _ = proxy.send_event(AppEvent::Redraw);
            }));
        "##),

        p("The render side calls ",
          code("render_wgpu::request_redraw()"),
          " whenever it needs another frame; that invokes the \
           installed closure, which posts whatever wake event the \
           shell's event loop understands. The two layers never have \
           to know about each other's loop primitives."),
        p("Concrete forms per platform:"),
        list(
            ["winit — ", code("EventLoopProxy::send_event"),
             " with a custom ", code("Redraw"),
             " variant. The shell's ", code("user_event"),
             " handler calls ", code("window.request_redraw()"), "."],
            ["Browser — schedule a ",
             code("requestAnimationFrame"),
             " on the next tick. Coalesce multiple calls per frame."],
            ["UIKit — ", code("setNeedsDisplay"),
             " on the rendering view, or ", code("CADisplayLink"),
             " if you're running a continuous loop."],
            ["Android — ", code("View.postInvalidateOnAnimation"),
             " or a ", code("Choreographer"), " frame callback."],
        ),
        p("The hook lives in thread-local storage. The framework's \
           reactivity is single-threaded; cross-thread input (audio \
           callbacks, background networking) needs to post onto the \
           host thread before driving ", code("EventSink"), "."),
    },

    section(heading = "Writing a new native shell") {
        p("If you want to drive the wgpu render backend from a \
           platform that isn't winit — a browser canvas, UIKit, \
           Android NDK, a custom embedded windowing layer — you write \
           a new shell crate. Depend on ", code("render-api"),
          " for the contract and ", code("render-wgpu"),
          " for ", code("Host"), " + ", code("Renderer"),
          ". Don't depend on ", code("host-winit"),
          " — that one's winit-specific."),
        p("Minimal skeleton:"),

        code(rust, r##"
            use std::time::Instant;
            use render_api::DeviceProfile;
            use render_wgpu::{install_redraw_hook, Host, Renderer};
            use runtime_core::Primitive;

            pub fn run<F: FnOnce() -> Primitive + 'static>(
                profile: DeviceProfile,
                build_ui: F,
            ) {
                // 1. Wire the redraw hook into your platform's event loop.
                install_redraw_hook(Box::new(|| {
                    // post-a-redraw-message logic for your platform
                }));

                // 2. Build the render-side host + renderer.
                let mut host = Host::new(profile.platform, profile.color_scheme);
                let mut renderer = /* Renderer::new(&device, &queue, format) */;

                // 3. Hand the host the viewport and mount the app.
                host.set_viewport(profile.logical_size.0 as f32,
                                  profile.logical_size.1 as f32);
                host.mount(build_ui);

                // 4. Run your platform's event loop. For each native event,
                //    translate to api types and call into EventSink:
                //
                //    on touch-began:  host.pointer_down(PointerEvent { ... })
                //    on touch-moved:  host.pointer_move(...)
                //    on touch-ended:  host.pointer_up(...)
                //    on scroll:       host.scroll(ScrollEvent { ... })
                //    on key press:    host.key(&KeyEvent { ... })
                //    on resize:       host.set_viewport(w, h)
                //
                //    Each frame: renderer.render(&host, ...); then
                //    if host.tick(Instant::now()) { request_redraw(); }
            }
        "##),

        p("The winit shell at ",
          code("crates/backend/wgpu/native/src/app.rs"),
          " is the worked example. It's about 300 lines, most of which \
           is straightforward ", code("winit::WindowEvent"),
          " → api-type translation. A web shell would be the same \
           shape but reading DOM events; a UIKit shell would read ",
          code("UIEvent"), " / ", code("UIPress"),
          " — the translation layer is the bulk of the work, and it's \
           all platform-specific code with no framework dependency."),
    },

    section(heading = "Writing a new render backend") {
        p("If you want to keep the same native shell but swap the \
           renderer — Skia, vello, a CPU rasterizer, a remote-display \
           protocol — you write a render-backend crate. Depend on ",
          code("render-api"), " only. Don't depend on ",
          code("render-wgpu"),
          "; you are the replacement for it."),
        p("Two traits to implement:"),
        list(
            [code("runtime_core::Backend"),
             " — so the framework can hand you a primitive tree and \
              drive ", code("create_*"), " / ", code("insert"), " / ",
             code("apply_style"),
             " calls. See ",
             link("Writing your own backend", to = "writing-a-backend"),
             " for the full surface."],
            [code("render_api::EventSink"),
             " — so any shell on the api side can forward events to \
              you. The eight methods above."],
        ),
        p(code("render-wgpu"), "'s ", code("Host"),
          " is the worked example: it owns the per-interaction state \
           (focus, press capture, momentum-scroll velocity, keyboard \
           slide phase) and routes events through its hit-test logic \
           into the backend's reactive callbacks. A new render \
           backend would replace ", code("Host"),
          " with its own equivalent, but the trait surface it presents \
           to a shell stays the same."),
        p("Once both traits are implemented, your render backend \
           pairs with any existing native shell — they only \
           communicate through the api crate."),
    },

    section(heading = "Which crate to depend on") {
        p("A quick lookup for which crate goes in your ",
          code("Cargo.toml"), ":"),
        list(
            ["Shipping an app on a variant the framework provides — \
              depend on the variant crate (", code("variant-phone"),
             ", etc.). It pulls in everything it needs."],
            ["Writing a new native shell — depend on ",
             code("render-api"), " (for the contract) and ",
             code("render-wgpu"),
             " (for ", code("Host"), " + ", code("Renderer"),
             "). Skip ", code("host-winit"),
             " — that one's the winit shell, you're writing its \
              replacement."],
            ["Writing a new render backend — depend on ",
             code("render-api"), " only. You're the replacement \
              for ", code("render-wgpu"), "."],
            ["Writing a new variant — depend on the native + render \
              combo the variant ships, plus optionally ",
             code("render-api"),
             " directly if you want to construct a ", code("DeviceProfile"),
             " without going through a re-export."],
        ),
        p(code("render-wgpu"), " and ", code("host-winit"),
          " each re-export the api types they expose, so most consumers \
           don't need a direct ", code("render-api"),
          " dependency."),
    },

    section(heading = "Where to read more") {
        list(
            [link("The shipped backends", to = "backends"),
             " — the big-picture map. The wgpu backend is the preview \
              renderer used by ", code("idealyst dev"),
             "; the production backends are web, iOS, and Android."],
            [link("Writing your own backend", to = "writing-a-backend"),
             " — the full ", code("runtime_core::Backend"),
             " trait surface a render backend has to implement, \
              independent of the wgpu split."],
            [link("Primitives", to = "primitives"),
             " — the vocabulary the render backend has to know how to \
              put on screen. Each primitive corresponds to a method on \
              the ", code("Backend"), " trait."],
        ),
    },
}
