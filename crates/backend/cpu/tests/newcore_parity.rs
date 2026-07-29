//! New-core adoption tests for the CPU-rasterizer backend: cross-core
//! render parity (rule-7 gate — the same scene must paint the same
//! PIXELS on both cores, byte-for-byte on `MemSurface`) plus op-level
//! coverage of the caps adoption's flush discipline (click → staged
//! writes → flush → paint).
//!
//! The pixel-parity suite on the real rasterizer is the live evidence
//! for this backend — there is no windowed host to screenshot; the
//! framebuffer IS the output surface, and `MemSurface::pixels()` is
//! compared byte-identical between `runtime_core::mount` (old walker)
//! and `newcore::start` (world + vocabulary handlers) for every scene.
//!
//! Harness notes: the tests install a queue-only scheduler (the same
//! drain-until-empty microtask semantics a real host loop provides).
//! `install_scheduler` is process-global/first-wins, but the queue
//! state is thread-local, so each test thread drains only its own
//! tasks.

#![cfg(feature = "new-core")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_cpu::{ClickOutcome, CpuBackend, MemSurface};
use runtime_core::{
    Color, Gradient, GradientKind, GradientStop, Length, StyleApplication, StyleRules, StyleSheet,
    Tokenized, Transform,
};

// ===========================================================================
// Queue-only test scheduler (a host loop's microtask semantics)
// ===========================================================================

mod test_scheduler {
    use runtime_core::scheduling::{ScheduleHandle, Scheduler};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        static QUEUE: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
            RefCell::new(VecDeque::new());
    }

    struct NoopHandle;
    impl ScheduleHandle for NoopHandle {
        fn cancel(&mut self) {}
    }

    struct QueueScheduler;
    impl Scheduler for QueueScheduler {
        fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
        }
        fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn after_ms(&self, _delay_ms: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
    }

    pub fn ensure_installed() {
        if !runtime_core::scheduling::is_scheduler_installed() {
            runtime_core::scheduling::install_scheduler(Box::new(QueueScheduler));
        }
    }

    /// Drain-until-empty — the same posture as a host's per-frame loop.
    pub fn drain() {
        loop {
            let next = QUEUE.with(|q| q.borrow_mut().pop_front());
            match next {
                Some(task) => task(),
                None => break,
            }
        }
    }
}

// ===========================================================================
// Harness
// ===========================================================================

const W: u32 = 120;
const H: u32 = 80;
/// The backend's default clear color — used by the liveness guard so a
/// "parity" of two blank frames can't pass silently.
const CLEAR: [u8; 4] = [0, 0, 0, 255];

fn fresh_backend() -> Rc<RefCell<CpuBackend>> {
    test_scheduler::ensure_installed();
    let backend = Rc::new(RefCell::new(CpuBackend::new(W, H)));
    // Seeds the old-core viewport TLS (which the new core's per-world
    // ctx reads at creation) — the pre-mount half of the viewport seam.
    backend.borrow_mut().set_viewport(W, H);
    backend
}

fn render(backend: &Rc<RefCell<CpuBackend>>) -> Vec<u8> {
    let mut surface = MemSurface::new(W, H);
    backend.borrow_mut().render(&mut surface);
    surface.pixels().to_vec()
}

/// Byte-exact frame comparison, with the first divergent pixel's
/// coordinates + channel values on failure.
fn assert_pixels_identical(name: &str, old: &[u8], new: &[u8]) {
    assert_eq!(old.len(), new.len(), "{name}: framebuffer size");
    for (i, (a, b)) in old.chunks_exact(4).zip(new.chunks_exact(4)).enumerate() {
        if a != b {
            let x = i as u32 % W;
            let y = i as u32 / W;
            panic!("{name}: pixel divergence at ({x},{y}): old {a:?} vs new {b:?}");
        }
    }
}

/// Liveness guard: at least one pixel must differ from the clear color,
/// proving the scene actually painted (blank-vs-blank parity is vacuous).
fn assert_painted(name: &str, pixels: &[u8]) {
    assert!(
        pixels.chunks_exact(4).any(|p| p != CLEAR),
        "{name}: scene painted nothing — every pixel is the clear color"
    );
}

fn render_old(app: impl Fn() -> runtime_core::Element + 'static) -> Vec<u8> {
    let backend = fresh_backend();
    let owner = runtime_core::mount(backend.clone(), app);
    test_scheduler::drain();
    let pixels = render(&backend);
    drop(owner);
    pixels
}

fn render_new(build: impl FnOnce() -> runtime_scene::Element) -> Vec<u8> {
    let backend = fresh_backend();
    let app = backend_cpu::newcore::start(backend.clone(), |_| {}, build);
    test_scheduler::drain();
    let pixels = render(&backend);
    app.stop();
    pixels
}

/// Build `StyleRules` with a few fields set (`StyleRules` has 40+
/// optional fields; the closure shape keeps scenes readable). The SAME
/// rules value feeds both cores — old via a static `StyleSheet`, new
/// via the builder's `.style(rules)`.
fn rules(f: impl FnOnce(&mut StyleRules)) -> StyleRules {
    let mut s = StyleRules::default();
    f(&mut s);
    s
}

fn sheet(r: StyleRules) -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::r#static(r)))
}

fn sized(width: f32, height: f32, background: &str) -> StyleRules {
    rules(|s| {
        s.width = Some(Tokenized::Literal(Length::Px(width)));
        s.height = Some(Tokenized::Literal(Length::Px(height)));
        s.background = Some(Tokenized::Literal(background.into()));
    })
}

// ===========================================================================
// 1. Full-scene pixel parity: styled view trees
// ===========================================================================

/// The rule-7 gate for the view path: nested views exercising every
/// paint feature the rasterizer has — backgrounds, per-side borders,
/// corner radii, a linear gradient, opacity (composited against the
/// parent), and a static translate — must produce byte-identical
/// framebuffers on both cores.
#[test]
fn newcore_styled_view_tree_pixel_parity() {
    fn root_rules() -> StyleRules {
        sized(W as f32, H as f32, "#101020")
    }
    fn bordered_rules() -> StyleRules {
        rules(|s| {
            *s = sized(60.0, 30.0, "#334455");
            s.border_top_width = Some(Tokenized::Literal(2.0));
            s.border_right_width = Some(Tokenized::Literal(3.0));
            s.border_bottom_width = Some(Tokenized::Literal(2.0));
            s.border_left_width = Some(Tokenized::Literal(1.0));
            s.border_top_color = Some(Tokenized::Literal(Color("rgb(255, 255, 0)".into())));
            s.border_right_color = Some(Tokenized::Literal(Color("rgb(0, 255, 255)".into())));
            s.border_bottom_color = Some(Tokenized::Literal(Color("rgb(255, 0, 255)".into())));
            s.border_left_color = Some(Tokenized::Literal(Color("rgb(255, 128, 0)".into())));
            s.border_top_left_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            s.border_bottom_right_radius = Some(Tokenized::Literal(Length::Px(6.0)));
        })
    }
    fn gradient_rules() -> StyleRules {
        rules(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(40.0)));
            s.height = Some(Tokenized::Literal(Length::Px(20.0)));
            s.opacity = Some(Tokenized::Literal(0.5));
            s.background_gradient = Some(Gradient {
                kind: GradientKind::Linear { angle_deg: 90.0 },
                stops: vec![
                    GradientStop { offset: 0.0, color: Color("rgb(255, 0, 0)".into()) },
                    GradientStop { offset: 1.0, color: Color("rgb(0, 255, 0)".into()) },
                ],
            });
        })
    }
    fn translated_rules() -> StyleRules {
        rules(|s| {
            *s = sized(30.0, 10.0, "rgb(0, 255, 0)");
            s.transform = Some(vec![
                Transform::TranslateX(Length::Px(8.0)),
                Transform::TranslateY(Length::Px(4.0)),
            ]);
        })
    }

    let old = render_old(|| {
        use runtime_core::{view, IntoElement};
        view(vec![
            view(vec![]).with_style(sheet(bordered_rules())).into_element(),
            view(vec![]).with_style(sheet(gradient_rules())).into_element(),
            view(vec![]).with_style(sheet(translated_rules())).into_element(),
        ])
        .with_style(sheet(root_rules()))
        .into_element()
    });
    let new = render_new(|| {
        use runtime_vocabulary::builders::view;
        view()
            .style(root_rules())
            .child(view().style(bordered_rules()))
            .child(view().style(gradient_rules()))
            .child(view().style(translated_rules()))
            .build()
    });
    assert_painted("styled_view_tree", &old);
    assert_pixels_identical("styled_view_tree", &old, &new);
}

// ===========================================================================
// 2. Text: fg color + font-size scale
// ===========================================================================

/// Text parity through the 8x8 bitmap-font path: default-scale white
/// text and a `font_size: 16px` (scale-2) colored line must rasterize
/// identical glyph pixels on both cores.
#[test]
fn newcore_text_color_and_font_scale_pixel_parity() {
    fn root_rules() -> StyleRules {
        sized(W as f32, H as f32, "#202020")
    }
    fn small_rules() -> StyleRules {
        rules(|s| {
            s.color = Some(Tokenized::Literal(Color("rgb(255, 255, 255)".into())));
            s.width = Some(Tokenized::Literal(Length::Px(96.0)));
            s.height = Some(Tokenized::Literal(Length::Px(8.0)));
        })
    }
    fn big_rules() -> StyleRules {
        rules(|s| {
            s.color = Some(Tokenized::Literal(Color("rgb(255, 200, 0)".into())));
            s.font_size = Some(Tokenized::Literal(Length::Px(16.0)));
            s.width = Some(Tokenized::Literal(Length::Px(112.0)));
            s.height = Some(Tokenized::Literal(Length::Px(16.0)));
        })
    }

    let old = render_old(|| {
        use runtime_core::{text, view, IntoElement};
        view(vec![
            text("hello cpu").with_style(sheet(small_rules())).into_element(),
            text("BIG").with_style(sheet(big_rules())).into_element(),
        ])
        .with_style(sheet(root_rules()))
        .into_element()
    });
    let new = render_new(|| {
        use runtime_vocabulary::builders::{text, view};
        view()
            .style(root_rules())
            .child(text().content("hello cpu").style(small_rules()))
            .child(text().content("BIG").style(big_rules()))
            .build()
    });
    assert_painted("text_scale", &old);
    assert_pixels_identical("text_scale", &old, &new);
}

// ===========================================================================
// 3. Interactive primitives: button + pressable + scroll_view
// ===========================================================================

/// Button (label through the Text paint path), pressable (View paint +
/// on_click slot), and a scroll_view that CLIPS an oversized child to
/// its own frame — the three interactive/container kinds the backend
/// implements natively — paint identically on both cores.
#[test]
fn newcore_button_pressable_scrollview_pixel_parity() {
    fn root_rules() -> StyleRules {
        sized(W as f32, H as f32, "#101020")
    }
    fn button_rules() -> StyleRules {
        rules(|s| {
            *s = sized(48.0, 12.0, "#446688");
            s.color = Some(Tokenized::Literal(Color("rgb(255, 255, 255)".into())));
        })
    }
    fn press_rules() -> StyleRules {
        sized(40.0, 14.0, "rgb(160, 40, 40)")
    }
    fn press_child_rules() -> StyleRules {
        sized(16.0, 6.0, "rgb(240, 240, 240)")
    }
    fn scroll_rules() -> StyleRules {
        sized(50.0, 24.0, "#223322")
    }
    // Wider AND taller than the scroll frame — pixels outside the
    // 50x24 scroll box must be clipped identically.
    fn scroll_child_rules() -> StyleRules {
        sized(90.0, 40.0, "rgb(80, 200, 120)")
    }

    let old = render_old(|| {
        use runtime_core::{button, pressable, scroll_view, view, IntoElement};
        view(vec![
            button("Go", || {}).with_style(sheet(button_rules())).into_element(),
            pressable(
                vec![view(vec![]).with_style(sheet(press_child_rules())).into_element()],
                || {},
            )
            .with_style(sheet(press_rules()))
            .into_element(),
            scroll_view(vec![
                view(vec![]).with_style(sheet(scroll_child_rules())).into_element(),
            ])
            .with_style(sheet(scroll_rules()))
            .into_element(),
        ])
        .with_style(sheet(root_rules()))
        .into_element()
    });
    let new = render_new(|| {
        use runtime_vocabulary::builders::{button, pressable, scroll_view, view};
        view()
            .style(root_rules())
            .child(button().label("Go").on_press(|| {}).style(button_rules()))
            .child(
                pressable(|| {})
                    .style(press_rules())
                    .child(view().style(press_child_rules())),
            )
            .child(
                scroll_view()
                    .style(scroll_rules())
                    .child(view().style(scroll_child_rules())),
            )
            .build()
    });
    assert_painted("button_pressable_scroll", &old);
    assert_pixels_identical("button_pressable_scroll", &old, &new);
}

// ===========================================================================
// 4. Reactive control flow: Dyn branch (both initial states) + keyed list
// ===========================================================================

/// A conditional branch between static siblings — old-core `when` vs
/// new-core `dyn_keyed` — must paint identically for BOTH initial
/// states. Both cores are anchorless-incapable here (no splice, see the
/// splice-contract test), so both mount the branch under a reactive
/// anchor view; the anchor participates in flex layout on both sides.
#[test]
fn newcore_dyn_branch_pixel_parity_both_initial_states() {
    fn root_rules() -> StyleRules {
        sized(W as f32, H as f32, "#101020")
    }
    fn header_rules() -> StyleRules {
        sized(24.0, 6.0, "rgb(255, 255, 255)")
    }
    fn on_rules() -> StyleRules {
        sized(40.0, 16.0, "rgb(0, 200, 0)")
    }
    fn off_rules() -> StyleRules {
        sized(24.0, 10.0, "rgb(200, 0, 0)")
    }
    fn footer_rules() -> StyleRules {
        sized(24.0, 6.0, "rgb(0, 0, 255)")
    }

    fn old_scene(initial: bool) -> Vec<u8> {
        render_old(move || {
            use runtime_core::{signal, view, when, IntoElement};
            let show = signal(initial);
            view(vec![
                view(vec![]).with_style(sheet(header_rules())).into_element(),
                when(
                    move || show.get(),
                    || view(vec![]).with_style(sheet(on_rules())).into_element(),
                    || view(vec![]).with_style(sheet(off_rules())).into_element(),
                ),
                view(vec![]).with_style(sheet(footer_rules())).into_element(),
            ])
            .with_style(sheet(root_rules()))
            .into_element()
        })
    }
    fn new_scene(initial: bool) -> Vec<u8> {
        render_new(move || {
            use runtime_scene::dyn_keyed;
            use runtime_vocabulary::builders::view;
            use runtime_world::signal;
            let show = signal(initial);
            view()
                .style(root_rules())
                .child(view().style(header_rules()))
                .child(dyn_keyed(
                    move || show.get(),
                    |&on| {
                        if on {
                            view().style(on_rules()).build()
                        } else {
                            view().style(off_rules()).build()
                        }
                    },
                ))
                .child(view().style(footer_rules()))
                .build()
        })
    }

    let old_on = old_scene(true);
    let old_off = old_scene(false);
    assert_painted("dyn_branch(on)", &old_on);
    // The two states must actually render differently, or the parity
    // below proves nothing about the branch.
    assert!(old_on != old_off, "dyn branch states must differ visually");
    assert_pixels_identical("dyn_branch(on)", &old_on, &new_scene(true));
    assert_pixels_identical("dyn_branch(off)", &old_off, &new_scene(false));
}

/// A keyed list ([1, 2, 3], key = the number) between static header and
/// footer — old-core `each_keyed` vs new-core `keyed` — paints its rows
/// (size + color varying per key) identically on both cores.
#[test]
fn newcore_keyed_list_pixel_parity() {
    fn root_rules() -> StyleRules {
        sized(W as f32, H as f32, "#101020")
    }
    fn header_rules() -> StyleRules {
        sized(30.0, 6.0, "rgb(255, 255, 255)")
    }
    fn footer_rules() -> StyleRules {
        sized(30.0, 6.0, "rgb(0, 0, 255)")
    }
    fn row_rules(n: u32) -> StyleRules {
        sized(
            12.0 * n as f32,
            8.0,
            &format!("rgb({}, {}, 60)", 60 * n, 255 - 60 * n),
        )
    }

    let old = render_old(|| {
        use runtime_core::{each_keyed, signal, view, EachKey, EachRowBuild, IntoElement};
        let items = signal(vec![1u32, 2, 3]);
        view(vec![
            view(vec![]).with_style(sheet(header_rules())).into_element(),
            each_keyed(move || {
                items
                    .get()
                    .into_iter()
                    .map(|n| {
                        let build: EachRowBuild = Box::new(move || {
                            vec![view(vec![]).with_style(sheet(row_rules(n))).into_element()]
                        });
                        (EachKey::new(n), build)
                    })
                    .collect()
            }),
            view(vec![]).with_style(sheet(footer_rules())).into_element(),
        ])
        .with_style(sheet(root_rules()))
        .into_element()
    });
    let new = render_new(|| {
        use runtime_scene::keyed;
        use runtime_vocabulary::builders::view;
        use runtime_world::signal;
        let items = signal(vec![1u32, 2, 3]);
        view()
            .style(root_rules())
            .child(view().style(header_rules()))
            .child(keyed(
                move || items.get(),
                |n| *n,
                |n: u32| view().style(row_rules(n)).build(),
            ))
            .child(view().style(footer_rules()))
            .build()
    });
    assert_painted("keyed_list", &old);
    assert_pixels_identical("keyed_list", &old, &new);
}

// ===========================================================================
// 5. Input events → staged writes → flush → paint (the driver contract)
// ===========================================================================

/// The click scene both the flush-discipline test and its old-core
/// reference leg mount: a reactive count readout over an absolutely
/// positioned pressable (fixed hit coordinates — the CPU backend has no
/// text-scan diagnostics like the terminal's grid rows).
const PRESS_X: u32 = 20;
const PRESS_Y: u32 = 50;

fn count_rules() -> StyleRules {
    rules(|s| {
        s.color = Some(Tokenized::Literal(Color("rgb(255, 255, 255)".into())));
        s.width = Some(Tokenized::Literal(Length::Px(96.0)));
        s.height = Some(Tokenized::Literal(Length::Px(8.0)));
    })
}

fn press_target_rules() -> StyleRules {
    rules(|s| {
        *s = sized(40.0, 20.0, "rgb(160, 40, 40)");
        s.position = Some(runtime_core::Position::Absolute);
        s.left = Some(Tokenized::Literal(Length::Px(10.0)));
        s.top = Some(Tokenized::Literal(Length::Px(40.0)));
    })
}

/// REGRESSION (flush discipline): on the new core a click handler's
/// `Signal::update` is STAGED — nothing observable until the flush
/// driver commits. The caps layer wraps `create_pressable`'s on_click,
/// so the closure `dispatch_click` returns IS the wrapped one: firing
/// it queues one deduped flush microtask; the host's drain then commits
/// it before the next paint. The test pins all three phases — pixels
/// UNCHANGED after the handler fires (still staged), changed after the
/// drain, and byte-identical to the old core after the same click.
#[test]
fn newcore_click_commits_via_flush_before_next_paint() {
    // --- new-core leg ---
    let backend = fresh_backend();
    let app = backend_cpu::newcore::start(backend.clone(), |_| {}, move || {
        use runtime_vocabulary::builders::{pressable, text, view};
        use runtime_world::signal;
        let count = signal(0i32);
        view()
            .style(sized(W as f32, H as f32, "#101020"))
            .child(
                text()
                    .content(move || format!("count: {}", count.get()))
                    .style(count_rules()),
            )
            .child(
                pressable(move || count.update(|c| c + 1)).style(press_target_rules()),
            )
            .build()
    });
    test_scheduler::drain();
    let before = render(&backend);
    assert_painted("click_flush(before)", &before);

    let outcome = backend.borrow_mut().dispatch_click(PRESS_X, PRESS_Y);
    let ClickOutcome::HandlerFired(h) = outcome else {
        panic!("click must land on the pressable, got {outcome:?}");
    };
    // Fire with the backend borrow released (host contract). The
    // wrapped handler stages the write and queues the flush.
    h();
    // Before the drain the write is UNCOMMITTED — the staged-write
    // model's whole point. A re-render must repaint the OLD count.
    let staged = render(&backend);
    assert_pixels_identical("click_flush(staged)", &before, &staged);

    // The host's drain runs the queued flush; the next paint sees the
    // committed value.
    test_scheduler::drain();
    let after = render(&backend);
    assert!(after != before, "flush must commit the click before the next paint");
    app.stop();

    // --- old-core reference leg: same scene, same click, same pixels ---
    let old_backend = fresh_backend();
    let owner = runtime_core::mount(old_backend.clone(), move || {
        use runtime_core::{pressable, signal, text, view, IntoElement};
        let count = signal(0i32);
        view(vec![
            text(move || format!("count: {}", count.get()))
                .with_style(sheet(count_rules()))
                .into_element(),
            pressable(vec![], move || count.set(count.get() + 1))
                .with_style(sheet(press_target_rules()))
                .into_element(),
        ])
        .with_style(sheet(sized(W as f32, H as f32, "#101020")))
        .into_element()
    });
    test_scheduler::drain();
    let old_before = render(&old_backend);
    assert_pixels_identical("click_flush(initial parity)", &old_before, &before);
    // Bind the outcome FIRST: a `match` on `borrow_mut().dispatch_click(..)`
    // keeps the scrutinee's borrow alive through the arms, and the old
    // core's text effect re-borrows the backend synchronously inside
    // `h()` — the exact "fire after releasing the borrow" host contract
    // the `ClickOutcome` docs spell out.
    let outcome = old_backend.borrow_mut().dispatch_click(PRESS_X, PRESS_Y);
    match outcome {
        ClickOutcome::HandlerFired(h) => h(),
        other => panic!("old-core click must land on the pressable, got {other:?}"),
    }
    test_scheduler::drain();
    let old_after = render(&old_backend);
    assert_pixels_identical("click_flush(post-click parity)", &old_after, &after);
    drop(owner);
}

// ===========================================================================
// 6. Op-level caps adoption
// ===========================================================================

/// Host's structural seam must track the Backend splice contract, and
/// that contract is pinned at `false` (the CPU backend never overrides
/// the trait default): reactive regions mount ANCHORED on both cores.
/// If either half flips independently — or the default silently changes
/// — anchor placement (and therefore layout) diverges between cores.
#[test]
fn newcore_host_splice_matches_backend_and_stays_anchored() {
    use runtime_core::Backend;
    use runtime_scene::Host;
    let b = CpuBackend::new(W, H);
    assert_eq!(
        Host::supports_splice(&b),
        Backend::supports_child_splice(&b),
        "Host::supports_splice must delegate to the Backend contract"
    );
    assert!(
        !Host::supports_splice(&b),
        "the CPU backend rides the anchored default; a silent flip to spliced \
         would change reactive-region mounting on the new core only"
    );
}

/// Boot bookkeeping: `is_booted` tracks start/stop, and `stop` tears
/// down the flush driver so a post-stop flush attempt — including one
/// queued by a retained click handler firing late — is a no-op (no
/// dangling world; runtime-world dead-world writes are silent no-ops).
#[test]
fn newcore_stop_uninstalls_flush_driver() {
    let backend = fresh_backend();
    assert!(!backend_cpu::newcore::is_booted());
    let app = backend_cpu::newcore::start(backend.clone(), |_| {}, || {
        use runtime_vocabulary::builders::{pressable, view};
        use runtime_world::signal;
        let count = signal(0i32);
        view()
            .style(sized(W as f32, H as f32, "#101020"))
            .child(pressable(move || count.update(|c| c + 1)).style(press_target_rules()))
            .build()
    });
    assert!(backend_cpu::newcore::is_booted());
    test_scheduler::drain();
    let _ = render(&backend);
    // Capture the wrapped handler BEFORE teardown — a real host can be
    // holding one across a quit (event in flight when the app stops).
    let ClickOutcome::HandlerFired(h) = backend.borrow_mut().dispatch_click(PRESS_X, PRESS_Y)
    else {
        panic!("click must land on the pressable");
    };
    app.stop();
    assert!(!backend_cpu::newcore::is_booted());
    // Late fire: the author write hits a dead world (silent no-op) and
    // the wrapper's schedule_flush queues a flush that finds no world.
    // Must not panic.
    h();
    backend_cpu::newcore::schedule_flush();
    test_scheduler::drain();
    backend_cpu::newcore::flush_sync();
}

/// Viewport forwarding: a post-boot `set_viewport` reaches the mounted
/// world's viewport signal (pixels in, pixels out) once the deduped
/// flush commits — the resize path's staged-write discipline, observed
/// through the same world ctx the breakpoint memo derives from.
#[test]
fn newcore_set_viewport_forwards_into_world() {
    let backend = fresh_backend();
    let seen: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    let seen_for_build = seen.clone();
    let app = backend_cpu::newcore::start(backend.clone(), |_| {}, move || {
        use runtime_vocabulary::builders::{text, view};
        let seen = seen_for_build.clone();
        let vp = runtime_vocabulary::viewport::viewport_ctx().size_signal();
        runtime_world::effect(move || {
            let s = vp.get();
            seen.borrow_mut().push((s.width, s.height));
        });
        view().child(text().content("x")).build()
    });
    test_scheduler::drain();
    assert_eq!(
        seen.borrow().last().copied(),
        Some((W as f32, H as f32)),
        "boot seeds the world ctx from the pre-mount set_viewport"
    );
    backend.borrow_mut().set_viewport(200, 100);
    test_scheduler::drain();
    assert_eq!(
        seen.borrow().last().copied(),
        Some((200.0, 100.0)),
        "post-boot resize forwards through the viewport sink"
    );
    app.stop();
}
