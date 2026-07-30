//! Render-parity tests for the CPU-rasterizer backend: the rule-7 gate
//! (a scene must paint the exact PIXELS the OLD core painted,
//! byte-for-byte on `MemSurface`) plus op-level coverage of the caps
//! adoption's flush discipline (click → staged writes → flush → paint).
//!
//! The pixel-parity suite on the real rasterizer is the live evidence for
//! this backend — there is no windowed host to screenshot; the
//! framebuffer IS the output surface.
//!
//! Harness notes: the tests install a queue-only scheduler (the same
//! drain-until-empty microtask semantics a real host loop provides).
//! `install_scheduler` is process-global/first-wins, but the queue state
//! is thread-local, so each test thread drains only its own tasks.
//!
//! # The frozen corpus is the contract
//!
//! Every scene below compares against a **frozen old-core framebuffer**
//! committed under `tests/goldens/` (lossless RGBA8 PNG), written by the
//! old walker before it was deleted. A mismatch is a real rendering
//! change, NOT a stale artifact: `IDEALYST_FREEZE_GOLDENS=1` can now only
//! RE-BASELINE against the current renderer, permanently discarding the
//! old core's testimony — see `tests/goldens/README.md`.

use std::cell::RefCell;
use std::rc::Rc;

use backend_cpu::{ClickOutcome, CpuBackend, MemSurface};
use runtime_shared::{
    Color, Gradient, GradientKind, GradientStop, Length, StyleRules, Tokenized, Transform,
};

// ===========================================================================
// Queue-only test scheduler (a host loop's microtask semantics)
// ===========================================================================

mod test_scheduler {
    use runtime_shared::scheduling::{ScheduleHandle, Scheduler};
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
        if !runtime_shared::scheduling::is_scheduler_installed() {
            runtime_shared::scheduling::install_scheduler(Box::new(QueueScheduler));
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

// ---------------------------------------------------------------------------
// Frozen-artifact gate
// ---------------------------------------------------------------------------

fn goldens() -> parity_goldens::Goldens {
    parity_goldens::Goldens::new(env!("CARGO_MANIFEST_DIR"))
}

/// The gate: the rendered framebuffer must match the frozen old-core PNG
/// pixel-for-pixel.
fn check_new_frame(name: &str, new: &[u8]) {
    goldens().check_rgba(name, W, H, new);
}

/// Liveness guard: at least one pixel must differ from the clear color,
/// proving the scene actually painted (blank-vs-blank parity is vacuous).
fn assert_painted(name: &str, pixels: &[u8]) {
    assert!(
        pixels.chunks_exact(4).any(|p| p != CLEAR),
        "{name}: scene painted nothing — every pixel is the clear color"
    );
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
/// optional fields; the closure shape keeps scenes readable).
fn rules(f: impl FnOnce(&mut StyleRules)) -> StyleRules {
    let mut s = StyleRules::default();
    f(&mut s);
    s
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

    let new = render_new(|| {
        use runtime_vocabulary::builders::view;
        view()
            .style(root_rules())
            .child(view().style(bordered_rules()))
            .child(view().style(gradient_rules()))
            .child(view().style(translated_rules()))
            .build()
    });
    assert_painted("styled_view_tree", &new);
    check_new_frame("styled_view_tree.png", &new);
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

    let new = render_new(|| {
        use runtime_vocabulary::builders::{text, view};
        view()
            .style(root_rules())
            .child(text().content("hello cpu").style(small_rules()))
            .child(text().content("BIG").style(big_rules()))
            .build()
    });
    assert_painted("text_scale", &new);
    check_new_frame("text_scale.png", &new);
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
    assert_painted("button_pressable_scroll", &new);
    check_new_frame("button_pressable_scroll.png", &new);
}

// ===========================================================================
// 4. Reactive control flow: Dyn branch (both initial states) + keyed list
// ===========================================================================

/// A conditional branch between static siblings (`dyn_keyed`) must paint
/// the old core's frozen pixels for BOTH initial states. This backend is
/// anchorless-incapable (no splice, see the splice-contract test), so the
/// branch mounts under a reactive anchor view that participates in flex
/// layout — exactly as the old walker's `when` did.
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

    let new_on = new_scene(true);
    let new_off = new_scene(false);
    assert_painted("dyn_branch(on)", &new_on);
    // The two states must actually render differently, or the gate
    // below proves nothing about the branch.
    assert!(new_on != new_off, "dyn branch states must differ visually");
    check_new_frame("dyn_branch_on.png", &new_on);
    check_new_frame("dyn_branch_off.png", &new_off);
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
    assert_painted("keyed_list", &new);
    check_new_frame("keyed_list.png", &new);
}

// ===========================================================================
// 4b. Caps-breadth scene (guards the de-trait pass's default resolution)
// ===========================================================================

/// COVERAGE BREADTH: every remaining leaf primitive this backend has a
/// `create_*` for, in one frame — image, icon, activity indicator,
/// controlled text input, text area, toggle, slider — plus a `link`,
/// which the CPU backend does NOT implement and therefore resolves to a
/// **trait default** (`create_link` → `create_view`).
///
/// Why this scene exists: the deletion wave converts each backend's
/// caps impls off the `Backend` trait, at which point every
/// default-resolved method stops resolving to a `Backend` default and
/// starts resolving to a **caps** default. A silently-differing default
/// would change behavior invisibly. Freezing a frame that actually
/// paints these primitives makes such a change fail loudly here.
///
/// The CPU rasterizer renders each unsupported leaf as its diagnostic
/// placeholder string through the same bitmap-font path, so the frame
/// distinguishes all eight kinds by glyphs, and the `link` shows up as a
/// styled box (the default's `create_view`).
#[test]
fn newcore_caps_breadth_leaf_primitives_pixel_parity() {
    fn root_rules() -> StyleRules {
        rules(|s| {
            *s = sized(W as f32, H as f32, "#181818");
            s.gap = Some(Tokenized::Literal(Length::Px(1.0)));
        })
    }
    /// One row per leaf: a fixed box with a readable fg color so each
    /// placeholder string rasterizes distinctly.
    fn leaf_rules(color: &str) -> StyleRules {
        rules(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(116.0)));
            s.height = Some(Tokenized::Literal(Length::Px(8.0)));
            s.color = Some(Tokenized::Literal(Color(color.into())));
        })
    }
    fn link_rules() -> StyleRules {
        sized(24.0, 6.0, "rgb(90, 160, 255)")
    }
    fn test_icon() -> runtime_shared::IconData {
        runtime_shared::IconData {
            view_box: (24, 24),
            paths: &["M4 4 L20 20"],
            fill_rule: runtime_shared::FillRule::NonZero,
            filled: false,
        }
    }

    let new = render_new(|| {
        use runtime_vocabulary::builders::{
            activity_indicator, icon, image, link, slider, text_area, text_input, toggle, view,
        };
        use runtime_world::signal;
        let input = signal(String::from("typed"));
        let area = signal(String::from("multi\nline"));
        let flag = signal(true);
        let amount = signal(0.5f32);
        view()
            .style(root_rules())
            .child(
                image()
                    .src("https://example.test/a.png")
                    .alt("alt text")
                    .style(leaf_rules("rgb(255, 255, 255)")),
            )
            .child(icon().data(test_icon()).style(leaf_rules("rgb(255, 200, 0)")))
            .child(activity_indicator().style(leaf_rules("rgb(0, 220, 220)")))
            .child(
                text_input()
                    .value(input)
                    .placeholder("hint")
                    .style(leaf_rules("rgb(220, 220, 120)")),
            )
            .child(text_area().value(area).style(leaf_rules("rgb(180, 140, 255)")))
            .child(toggle().value(flag).style(leaf_rules("rgb(120, 255, 140)")))
            .child(slider().value(amount).style(leaf_rules("rgb(255, 140, 140)")))
            .child(
                link()
                    .url("https://example.test/")
                    .external(true)
                    .style(link_rules()),
            )
            .build()
    });
    assert_painted("caps_breadth_leaves", &new);
    check_new_frame("caps_breadth_leaves.png", &new);
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
        s.position = Some(runtime_shared::Position::Absolute);
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

    // Both frames of the click cycle against the frozen old-core
    // reference — the pre-click paint AND the post-flush paint — so a
    // regression in either the initial render or the commit path fails.
    check_new_frame("click_before.png", &before);
    check_new_frame("click_after.png", &after);
}

// ===========================================================================
// 6. Op-level caps adoption
// ===========================================================================

/// The structural seam must stay ANCHORED. `supports_child_splice` used
/// to be a `Backend` trait DEFAULT here (the CPU backend never overrode
/// it); `Host` makes it REQUIRED, so `newcore.rs` now carries an explicit
/// body reproducing that default — see
/// docs/runtime-v2-deletion-baseline.md §2.2. A silent flip to spliced
/// would move reactive regions out from under their anchor view and
/// change layout in every frozen PNG, so pin the literal here.
#[test]
fn newcore_host_splice_is_anchored() {
    use runtime_scene::Host;
    let b = CpuBackend::new(W, H);
    assert!(
        !Host::supports_splice(&b),
        "the CPU backend renders ANCHORED (the frozen framebuffers pin it)"
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
