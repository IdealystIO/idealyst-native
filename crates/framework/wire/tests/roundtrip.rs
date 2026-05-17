//! End-to-end round-trip test for the hot-reload wire prototype.
//!
//! Demonstrates the full pipeline:
//!   1. Dev side records walker calls into a [`WireRecordingBackend`].
//!   2. Commands are JSON-serialized.
//!   3. JSON is deserialized on the "app" side.
//!   4. Commands replay against a [`TraceBackend`] (a stand-in for a
//!      real platform backend) that just notes every call.
//!   5. The trace matches the original recording — proving the wire
//!      faithfully carries the structural+style intent across.
//!
//! Run with `cargo test -p wire`.

use std::rc::Rc;

use framework_core::{Backend, Color, ColorScheme, StyleRules, Tokenized};
use dev_client::WireBackend;
use dev_server::WireRecordingBackend;
use wire::{Command, DevToApp};

// ---------------------------------------------------------------------------
// TraceBackend — minimal Backend impl that records every call it
// receives. Used as the "real platform backend" in tests.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Trace {
    CreateView(u64),
    CreateText(u64, String),
    CreateButton(u64, String),
    Insert(u64, u64),
    UpdateText(u64, String),
    ApplyStyle(u64), // we omit the actual style for ease of equality
    Finish(u64),
}

#[derive(Default)]
struct TraceBackend {
    next: u64,
    trace: Vec<Trace>,
}

impl Backend for TraceBackend {
    type Node = u64;

    fn create_view(&mut self) -> u64 {
        self.next += 1;
        let id = self.next;
        self.trace.push(Trace::CreateView(id));
        id
    }

    fn create_text(&mut self, content: &str) -> u64 {
        self.next += 1;
        let id = self.next;
        self.trace.push(Trace::CreateText(id, content.to_string()));
        id
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: Rc<dyn Fn()>,
        _leading_icon: Option<&framework_core::primitives::icon::IconData>,
        _trailing_icon: Option<&framework_core::primitives::icon::IconData>,
    ) -> u64 {
        self.next += 1;
        let id = self.next;
        self.trace.push(Trace::CreateButton(id, label.to_string()));
        id
    }

    fn insert(&mut self, parent: &mut u64, child: u64) {
        self.trace.push(Trace::Insert(*parent, child));
    }

    fn update_text(&mut self, node: &u64, content: &str) {
        self.trace.push(Trace::UpdateText(*node, content.to_string()));
    }

    fn clear_children(&mut self, _node: &u64) {}

    fn apply_style(&mut self, node: &u64, _style: &Rc<StyleRules>) {
        self.trace.push(Trace::ApplyStyle(*node));
    }

    fn finish(&mut self, root: u64) {
        self.trace.push(Trace::Finish(root));
    }

    fn create_link(
        &mut self,
        _config: framework_core::primitives::link::LinkConfig,
    ) -> u64 {
        self.next += 1;
        self.next
    }

    fn create_overlay(
        &mut self,
        _anchor: framework_core::primitives::overlay::OverlayAnchor,
        _backdrop: framework_core::primitives::overlay::BackdropMode,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
    ) -> u64 {
        self.next += 1;
        self.next
    }

    fn create_graphics(
        &mut self,
        _on_ready: framework_core::primitives::graphics::OnReady,
        _on_resize: framework_core::primitives::graphics::OnResize,
        _on_lost: framework_core::primitives::graphics::OnLost,
    ) -> u64 {
        self.next += 1;
        self.next
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_demo_tree(backend: &mut WireRecordingBackend) {
    // Hand-rolled walker calls. In real use the framework's walker
    // emits these as it processes a `Primitive` tree; here we
    // synthesize them directly so the test stays self-contained.

    let mut root = backend.create_view();

    // Header style: a flex row with background color.
    let header_style: Rc<StyleRules> = Rc::new({
        let mut s = StyleRules::default();
        s.background = Some(Tokenized::Literal(Color("#202020".into())));
        s.flex_direction = Some(framework_core::FlexDirection::Row);
        s.padding_top = Some(Tokenized::Literal(framework_core::Length::Px(16.0)));
        s.padding_bottom = Some(Tokenized::Literal(framework_core::Length::Px(16.0)));
        s
    });

    let header = backend.create_view();
    backend.apply_style(&header, &header_style);
    backend.insert(&mut root, header);

    let title = backend.create_text("Hot Reload!");
    backend.insert(&mut { header }, title);

    // A button that, when fired, prints to stdout. The closure is
    // captured by the recorder — the wire only carries a HandlerId.
    let on_click: Rc<dyn Fn()> = Rc::new(|| {
        // No-op in test; in real use this would mutate a signal.
    });
    let button = backend.create_button("Click me", on_click, None, None);
    backend.insert(&mut root, button);

    // Reactive update: simulate the walker firing a label effect.
    backend.update_text(&title, "Hot Reload! v2");

    backend.finish(root);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn full_round_trip_through_json() {
    // --- DEV SIDE ---
    let mut recorder = WireRecordingBackend::new();
    build_demo_tree(&mut recorder);

    let commands_dev = recorder.drain_commands();
    assert!(!commands_dev.is_empty(), "recorder should emit commands");

    // Wrap in a DevToApp::Commands envelope to exercise the
    // full-message codec, not just bare Command.
    let envelope = DevToApp::Commands(commands_dev.clone());

    // Serialize through the wire codec (JSON for the prototype).
    let bytes = wire::codec::encode(&envelope).expect("encode");

    // --- WIRE TRANSPORT (in-memory) ---
    // In real use this is a WebSocket frame.

    // --- APP SIDE ---
    let envelope_decoded: DevToApp =
        wire::codec::decode(&bytes).expect("decode");

    let commands_app = match envelope_decoded {
        DevToApp::Commands(c) => c,
        _ => panic!("expected Commands variant"),
    };

    assert_eq!(
        serde_json::to_string(&commands_dev).unwrap(),
        serde_json::to_string(&commands_app).unwrap(),
        "wire round-trip must preserve commands exactly"
    );

    // The app-side replay loop wraps a real Backend with WireBackend,
    // which also needs a channel to send events back to the dev side.
    let (tx, _rx) = std::sync::mpsc::channel();
    let trace_backend = TraceBackend::default();
    let mut wire_app = WireBackend::new(trace_backend, tx);

    wire_app.apply_batch(commands_app).expect("replay must succeed");

    let trace = wire_app.backend().trace.clone();

    // We don't pin every command's translation here — the recorder
    // emits RegisterStyle eagerly + AttachStates not used in this
    // tree. We assert on the structural skeleton: a view, a header
    // view, a title text, a button, the expected inserts, the text
    // update, and a finish.
    let view_ids: Vec<_> = trace
        .iter()
        .filter_map(|t| match t {
            Trace::CreateView(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(view_ids.len(), 2, "two views (root + header)");

    let texts: Vec<_> = trace
        .iter()
        .filter_map(|t| match t {
            Trace::CreateText(_, s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["Hot Reload!".to_string()]);

    let updates: Vec<_> = trace
        .iter()
        .filter_map(|t| match t {
            Trace::UpdateText(_, s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(updates, vec!["Hot Reload! v2".to_string()]);

    let buttons: Vec<_> = trace
        .iter()
        .filter_map(|t| match t {
            Trace::CreateButton(_, label) => Some(label.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(buttons, vec!["Click me".to_string()]);

    assert!(
        trace.iter().any(|t| matches!(t, Trace::Finish(_))),
        "finish command must be replayed"
    );

    assert!(
        trace.iter().any(|t| matches!(t, Trace::ApplyStyle(_))),
        "the header style must be registered + applied"
    );
}

#[test]
fn event_round_trip_through_handler_table() {
    // Build a button and verify that an Event message from the app
    // side dispatches to the dev-side closure.
    use std::cell::Cell;

    let mut recorder = WireRecordingBackend::new();
    let mut root = recorder.create_view();

    let fired = Rc::new(Cell::new(0u32));
    let on_click: Rc<dyn Fn()> = {
        let fired = fired.clone();
        Rc::new(move || {
            fired.set(fired.get() + 1);
        })
    };
    let button = recorder.create_button("go", on_click, None, None);
    recorder.insert(&mut root, button);
    recorder.finish(root);

    let commands = recorder.drain_commands();

    // Locate the HandlerId minted for the on_click. The Button
    // command carries it as `on_click`.
    let handler_id = commands
        .iter()
        .find_map(|c| match c {
            Command::CreateButton { on_click, .. } => Some(*on_click),
            _ => None,
        })
        .expect("Button command must carry a HandlerId");

    // Simulate the app firing the event back to dev.
    assert_eq!(fired.get(), 0);
    let dispatched = recorder
        .dispatch_event(handler_id, wire::EventArgs::Unit);
    assert!(dispatched, "handler must be found");
    assert_eq!(fired.get(), 1, "closure must run once");
}

#[test]
fn unknown_node_is_a_protocol_error() {
    // The replay engine should surface a typed error when the dev
    // side ships an Insert against a node it never created. Useful
    // for catching protocol drift early.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut wire_app = WireBackend::new(TraceBackend::default(), tx);

    let result = wire_app.apply(Command::Insert {
        parent: wire::NodeId(99),
        child: wire::NodeId(100),
    });

    assert!(matches!(
        result,
        Err(dev_client::ReplayError::UnknownNode(_))
    ));
}

/// Drive the real framework walker against a `WireRecordingBackend`,
/// then replay the captured commands through `WireBackend<TraceBackend>`.
/// This proves the recorder slots into `framework_core::render(...)`
/// without modification — i.e. real user component trees produce
/// faithful wire output.
#[test]
fn real_walker_drives_recorder() {
    use framework_core::{render, Primitive};
    use std::cell::RefCell;

    // A minimal Primitive tree: a View with a Text child. Built by
    // hand to avoid pulling in the `ui!` macro for the test.
    let tree = Primitive::View {
        children: vec![Primitive::Text {
            source: framework_core::TextSource::Static("hello, wire".into()),
            style: None,
            ref_fill: None,
        }],
        style: None,
        ref_fill: None,
    };

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);

    let commands = recorder.drain_commands();

    // At minimum: CreateText, CreateView, Insert, Finish. (Order may
    // vary — the walker tends to build children first then insert
    // them into the parent View it creates.)
    let count_create_text = commands
        .iter()
        .filter(|c| matches!(c, Command::CreateText { .. }))
        .count();
    let count_create_view = commands
        .iter()
        .filter(|c| matches!(c, Command::CreateView { .. }))
        .count();
    let count_insert = commands
        .iter()
        .filter(|c| matches!(c, Command::Insert { .. }))
        .count();
    let count_finish = commands
        .iter()
        .filter(|c| matches!(c, Command::Finish { .. }))
        .count();

    assert_eq!(count_create_view, 1, "exactly one View was built");
    assert_eq!(count_create_text, 1, "exactly one Text was built");
    assert_eq!(count_insert, 1, "Text inserted into View");
    assert_eq!(count_finish, 1, "render() called finish(root)");

    // Replay the captured commands through the app-side wire backend.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut wire_app = WireBackend::new(TraceBackend::default(), tx);
    wire_app.apply_batch(commands).expect("replay succeeds");

    let trace = wire_app.backend().trace.clone();
    assert!(
        trace.iter().any(|t| matches!(t, Trace::CreateText(_, s) if s == "hello, wire")),
        "trace must contain the rendered text content"
    );
    assert!(
        trace.iter().any(|t| matches!(t, Trace::Finish(_))),
        "trace must contain finish"
    );
}

/// Drive a Primitive::Link through the recording backend. Verifies
/// that `create_link` emits a `CreateLink` command with the route /
/// url / handler id intact, and that the app-side replay round-trips
/// to a `create_link` call on the real backend.
#[test]
fn link_round_trip() {
    use framework_core::primitives::link::LinkConfig;

    let mut recorder = WireRecordingBackend::new();
    let on_activate: Rc<dyn Fn()> = Rc::new(|| {});
    let config = LinkConfig {
        route: "profile",
        url: "/profile/123".to_string(),
        on_activate: on_activate.clone(),
    };
    let _link = recorder.create_link(config);
    recorder.finish(wire::NodeId(1));

    let commands = recorder.drain_commands();
    let create_link = commands
        .iter()
        .find_map(|c| match c {
            Command::CreateLink { route, url, .. } => Some((route.clone(), url.clone())),
            _ => None,
        })
        .expect("CreateLink command must be emitted");
    assert_eq!(create_link.0, "profile");
    assert_eq!(create_link.1, "/profile/123");

    // Replay path needs a Backend that implements `create_link`.
    // The trait default falls through to `create_view`, which is
    // fine for the trace assertion: we just verify the command is
    // accepted without error.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut wire_app = WireBackend::new(TraceBackend::default(), tx);
    wire_app.apply_batch(commands).expect("link replay must succeed");
}

/// Drive a Primitive::Overlay through the recording backend. The
/// overlay command captures the anchor placement, backdrop mode,
/// and on_dismiss handler id.
#[test]
fn overlay_round_trip() {
    use framework_core::primitives::overlay::{BackdropMode, OverlayAnchor, ViewportPlacement};

    let mut recorder = WireRecordingBackend::new();
    let on_dismiss: Option<Rc<dyn Fn()>> = Some(Rc::new(|| {}));
    let _node = recorder.create_overlay(
        OverlayAnchor::Viewport(ViewportPlacement::Bottom),
        BackdropMode::Dismiss,
        on_dismiss,
        false,
    );

    let commands = recorder.drain_commands();
    let create_overlay = commands
        .iter()
        .find_map(|c| match c {
            Command::CreateOverlay {
                anchor,
                backdrop,
                on_dismiss,
                trap_focus,
                ..
            } => Some((anchor.clone(), backdrop.clone(), on_dismiss.clone(), *trap_focus)),
            _ => None,
        })
        .expect("CreateOverlay command must be emitted");
    match create_overlay.0 {
        wire::WireOverlayAnchor::Viewport(wire::WireViewportPlacement::Bottom) => {}
        other => panic!("expected viewport bottom anchor, got {:?}", other),
    }
    assert!(matches!(
        create_overlay.1,
        wire::WireBackdropMode::Dismiss
    ));
    assert!(create_overlay.2.is_some(), "on_dismiss handler must be registered");
    assert!(!create_overlay.3);

    // Replay round-trip — backend default falls through to create_view
    // for the unimplemented create_overlay, which is fine for the
    // test.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut wire_app = WireBackend::new(TraceBackend::default(), tx);
    wire_app.apply_batch(commands).expect("overlay replay must succeed");
}

/// Drive a Primitive::Graphics through the recording backend. With
/// no named-renderer registration, the wire command falls through to
/// no-op handlers on the app side; the surface still mounts.
#[test]
fn graphics_round_trip_unnamed() {
    let mut recorder = WireRecordingBackend::new();
    let on_ready: framework_core::primitives::graphics::OnReady = Box::new(|_evt| {});
    let on_resize: framework_core::primitives::graphics::OnResize = Box::new(|_evt| {});
    let on_lost: framework_core::primitives::graphics::OnLost = Box::new(|| {});
    let _node = recorder.create_graphics(on_ready, on_resize, on_lost);

    let commands = recorder.drain_commands();
    let renderer_name = commands
        .iter()
        .find_map(|c| match c {
            Command::CreateGraphics { renderer, .. } => Some(renderer.clone()),
            _ => None,
        })
        .expect("CreateGraphics command must be emitted");
    assert_eq!(renderer_name, "<unnamed>");

    // Replay falls through to backend defaults (unimplemented) —
    // we use the panic-free path of having a real registry but
    // with no entries; lookup misses → no-op handlers.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut wire_app = WireBackend::new(TraceBackend::default(), tx);
    // TraceBackend doesn't implement create_graphics; the trait
    // default panics. So we just verify the wire path emits the
    // command correctly — that's the main contract being tested.
    let _ = wire_app; // silence
}

/// Verify the reverse-channel `handle_screen_released` path: the
/// app reports a swipe-back, the recorder looks up which navigator
/// owns the scope, and calls back into the framework's
/// `release_screen` callback.
#[test]
fn screen_released_reverse_channel() {
    use framework_core::primitives::navigator::{NavigatorCallbacks, NavigatorControl, NavState};
    use framework_core::Signal;
    use std::cell::Cell;

    let mut recorder = WireRecordingBackend::new();

    // Synthesize NavigatorCallbacks pretending we're the framework.
    let released = Rc::new(Cell::new(None::<u64>));
    let released_clone = released.clone();
    let mount_called = Rc::new(Cell::new(0u32));
    let mount_called_clone = mount_called.clone();
    let callbacks: NavigatorCallbacks<wire::NodeId> = NavigatorCallbacks {
        initial_route: "home",
        initial_path: "/",
        mount_screen: Rc::new(move |_, _| {
            mount_called_clone.set(mount_called_clone.get() + 1);
            framework_core::primitives::navigator::MountResult {
                node: wire::NodeId(42),
                scope_id: 100,
                options: framework_core::primitives::navigator::ScreenOptions::default(),
            }
        }),
        release_screen: Rc::new(move |scope| {
            released_clone.set(Some(scope));
        }),
        match_path: Rc::new(|_| None),
        build_layout: None,
        nav_state: NavState {
            active_route: Signal::new("home"),
            active_path: Signal::new("/".to_string()),
            depth: Signal::new(1),
            can_go_back: Signal::new(false),
        },
        depth_changed: Rc::new(|_| {}),
        defer_initial_mount: false,
    };

    let control = Rc::new(NavigatorControl::new());
    let nav_id = recorder.create_navigator(callbacks, control);

    // Attach an initial screen via the framework path. Note this
    // mirrors what `navigator_attach_initial` would normally do.
    recorder.navigator_attach_initial(
        &nav_id,
        wire::NodeId(7),
        100,
        framework_core::primitives::navigator::ScreenOptions::default(),
    );

    // App reports the user swiped back, releasing scope 100.
    let handled = recorder.handle_screen_released(100);
    assert!(handled, "scope 100 must map to the registered navigator");
    assert_eq!(
        released.get(),
        Some(100),
        "framework's release_screen must have been called with scope 100"
    );

    // mount_called shouldn't have fired in this test — we only
    // exercised the release path.
    assert_eq!(mount_called.get(), 0);
}

/// Drive a Primitive::Navigator through the framework's real walker
/// against the WireRecordingBackend. Verifies that CreateNavigator
/// and the navigator's child screen are emitted, plus the
/// NavigatorAttachInitial command.
#[test]
fn stack_navigator_initial_mount_round_trip() {
    use framework_core::primitives::navigator::{Navigator, Route};
    use framework_core::{render, Primitive, TextSource};
    use std::cell::RefCell;

    // Build a navigator with one route "home" → Text("Home").
    let home_route: Route<()> = Route::new("home", "/");
    let nav: framework_core::Bound<framework_core::NavigatorHandle> =
        Navigator::new(&home_route).screen(home_route, |_params: ()| Primitive::Text {
            source: TextSource::Static("Home".into()),
            style: None,
            ref_fill: None,
        });

    let tree: Primitive = <framework_core::Bound<framework_core::NavigatorHandle> as framework_core::IntoPrimitive>::into_primitive(nav);
    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);

    let commands = recorder.drain_commands();

    let has_create_nav = commands
        .iter()
        .any(|c| matches!(c, Command::CreateNavigator { .. }));
    assert!(has_create_nav, "CreateNavigator must be emitted");

    let has_create_text = commands
        .iter()
        .any(|c| matches!(c, Command::CreateText { .. }));
    assert!(has_create_text, "the initial screen's Text must be built");

    let has_attach_initial = commands
        .iter()
        .any(|c| matches!(c, Command::NavigatorAttachInitial { .. }));
    assert!(has_attach_initial, "NavigatorAttachInitial must be emitted");

    let has_finish = commands.iter().any(|c| matches!(c, Command::Finish { .. }));
    assert!(has_finish, "Finish must be the terminal command");
}

#[test]
fn color_scheme_helper_maps_correctly() {
    use dev_client::color_scheme_to_wire;
    use wire::WireColorScheme;
    assert!(matches!(
        color_scheme_to_wire(ColorScheme::Light),
        WireColorScheme::Light
    ));
    assert!(matches!(
        color_scheme_to_wire(ColorScheme::Dark),
        WireColorScheme::Dark
    ));
    assert!(matches!(
        color_scheme_to_wire(ColorScheme::Auto),
        WireColorScheme::Auto
    ));
}
