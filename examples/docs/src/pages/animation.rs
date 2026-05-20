//! Animation page — built via the `docs!` macro.
//!
//! Long-form coverage of the imperative animation system: value
//! handles, animator factories, springs/tweens/decays, composition
//! primitives, and the backend seam.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{codeblock, pageheader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "animation",
    title = "Animation",
    category = Foundation,
    description = "Imperative, gesture-friendly motion with velocity-preserving handoff.",
    related = ["styles", "primitives", "reactivity", "overview"],
    concepts = [
        AnimatedValue, Animator, AnimatorFactory,
        Tween, Spring, Decay, Keyframes,
        AnimationLoop, AnimationSequence, Stagger,
        AnimProp, AnimationClock,
    ],

    section(heading = "Intro") {
        p("Two systems handle motion in the framework, and they solve different problems."),
        p(link("Style transitions", to = "styles#transitions"), " are declarative: \
           \"when this style property changes, interpolate over N milliseconds.\" \
           Great for hover, focus, theme swaps — anything where the author describes a \
           target and the browser/UI runtime does the interpolation work."),
        p("The ", code("animation"), " module covered on this page is imperative: \
           you hold a value, drive it with a spring or tween or decay, and feed the \
           per-frame samples to a backend property. Use this for gestures, custom \
           motion, anything that needs interruption with velocity preservation."),
    },

    section(heading = "The model in one paragraph") {
        p("An ", code("AnimatedValue<T>"), " is a small handle to a value of type ",
          code("T"), " that an ", code("Animator"), " can drive over time. Authors \
           construct an animator via a factory (", code("TweenTo::new(...)"),
          ", ", code("SpringTo::new(...)"),
          ", ", code("DecayFrom::new(...)"),
          ") and hand it to the value with ", code("value.animate(factory)"),
          ". A shared per-thread ", code("AnimationClock"),
          " ticks every live animator once per frame and updates its value. \
           Subscribers to the value (typically writing to a backend property) fire \
           every frame the value changes. When no animation is in flight the clock \
           idles to zero per-frame work."),
    },

    section(heading = "First look") {
        p("Build a value, subscribe it to a backend property, and animate it:"),

        code(rust, r##"
            use framework_core::animation::*;
            use std::time::Duration;

            let scale = AnimatedValue::new(1.0_f32);

            // Hook the value to a node's scale on the active backend.
            // `subscribe_and_apply` fires once immediately so the
            // backend reflects the value's starting position before
            // the first tick.
            let _sub = scale.subscribe_and_apply({
                let backend = backend.clone();
                let node = node.clone();
                move |v, _vel| {
                    backend
                        .borrow_mut()
                        .set_animated_f32(&node, AnimProp::Scale, *v);
                }
            });

            // Press: tween toward 1.1 with ease-out.
            scale.animate(TweenTo::new(1.1, Duration::from_millis(120)).ease_out());

            // Release mid-flight: the spring inherits the tween's
            // current velocity, so motion continues smoothly across
            // the swap. This is the move that makes drag-and-release
            // feel right.
            scale.animate(SpringTo::new(1.0).stiffness(280).damping(22));
        "##),

        p("The ", code("_sub"), " plumbing is by design — author code (or a \
           peripheral builder library) owns the subscription's lifetime. The core \
           surface is the value handle and the animator factories; backend wiring \
           is the seam you control."),
    },

    section(heading = "AnimatedValue") {
        p("The value handle. ", code("Clone"),
          "-able and cheap (an ", code("Rc"),
          " internally); clones share state. Same identity model as ", code("Signal"),
          "."),

        code(rust, r##"
            impl<T: Animatable> AnimatedValue<T> {
                pub fn new(initial: T) -> Self;

                pub fn get(&self) -> T;
                pub fn velocity(&self) -> T;

                pub fn set(&self, value: T);          // snap; zero velocity
                pub fn cancel(&self);                 // stop; preserve velocity
                pub fn animate<F: AnimatorFactory<T>>(&self, f: F);
                pub fn is_animating(&self) -> bool;

                pub fn subscribe<F: FnMut(&T, &T) + 'static>(&self, f: F)
                    -> Subscription<T>;
                pub fn subscribe_and_apply<F>(&self, f: F) -> Subscription<T>;
            }
        "##),

        p(code("set(v)"), " snaps to a value, zeroes velocity, and cancels any \
           in-flight animator — use it for gesture-drag updates. ",
          code("cancel()"), " stops the animator but preserves velocity, so a \
           subsequent ", code("animate(...)"),
          " can hand off cleanly. ", code("subscribe_and_apply"),
          " fires the listener once with the current state before registering — \
           closes the mount-to-first-tick gap when wiring to backends."),

        p("Listeners receive ", code("(value, velocity)"),
          ". They may freely call back into the same value's API (", code("get"),
          ", ", code("set"), ", ", code("animate"), ", ", code("cancel"),
          ", ", code("subscribe"),
          "). The dispatch loop snapshots listener handles and uses ",
          code("try_borrow_mut"), " to skip recursive self-invocation silently. \
           The only constraint is that a listener can't invoke itself recursively."),
    },

    section(heading = "Animatable") {
        p("Any value type can flow through the system by implementing ",
          code("Animatable"), ":"),

        code(rust, r##"
            pub trait Animatable: Clone + 'static {
                fn add_scaled(base: &Self, delta: &Self, scale: f32) -> Self;
                fn sub(a: &Self, b: &Self) -> Self;
                fn norm_sq(value: &Self) -> f32;
                fn zero() -> Self;

                fn lerp(a: &Self, b: &Self, t: f32) -> Self;  // default impl
            }
        "##),

        p(code("add_scaled(base, d, k) = base + d * k"),
          " is the integration step every animator uses. ", code("sub(a, b) = a - b"),
          " produces displacement (spring force, tween delta). ", code("norm_sq"),
          " is the squared magnitude, compared against a squared threshold so \
           springs don't pay a ", code("sqrt"), " every frame."),

        p("Implementations ship for ", code("f32"), ", fixed-arity ", code("f32"),
          " tuples (", code("(f32, f32)"), " through ", code("(f32, f32, f32, f32)"),
          "), and const-generic ", code("[f32; N]"),
          " arrays. The array impl covers ", code("[r, g, b, a]"),
          " for color animation. Implementing for your own struct is mechanical — \
           wire the four ops component-wise."),
    },

    section(heading = "Animator and Sample") {
        p("An animator is a per-frame motion source for a single value:"),

        code(rust, r##"
            pub trait Animator<T: Animatable>: 'static {
                fn sample(&mut self, dt: Duration) -> Sample<T>;
            }

            pub struct Sample<T: Animatable> {
                pub value: T,
                pub velocity: T,    // T per second
                pub finished: bool,
            }
        "##),

        p(code("velocity"),
          " is the load-bearing detail — it's what makes gesture handoff feel right. \
           Tweens compute it by finite difference between consecutive samples; \
           springs and decays integrate it directly. After ", code("finished: true"),
          " the animator must remain idempotent (return the same resting tuple)."),
    },

    section(heading = "AnimatorFactory") {
        p("Authors don't build animators directly. They build factories — ",
          code("TweenTo"),
          ", ", code("SpringTo"),
          ", ", code("DecayFrom"),
          " — and the framework constructs the underlying animator at attachment \
           time, supplying the value handle's current state:"),

        code(rust, r##"
            pub trait AnimatorFactory<T: Animatable> {
                fn build(self, current: T, velocity: T) -> Box<dyn Animator<T>>;
            }
        "##),

        p("This split between intent (factory) and seed state (passed at ",
          code("build"),
          ") is exactly what enables velocity-preserving handoff between animators \
           — the value handle's current velocity flows into every new spring that \
           replaces an in-flight tween."),
    },

    section(heading = "Tween — duration + curve") {
        code(rust, r##"
            TweenTo::new(target, Duration::from_millis(150))
                .ease_out()
        "##),

        p("Linear interpolation under an ", code("Easing"),
          " curve from the framework's transition vocabulary (", code("Linear"), ", ",
          code("Ease"), ", ", code("EaseIn"), ", ", code("EaseOut"), ", ",
          code("EaseInOut"),
          ", or a custom cubic-Bézier). Same easing functions back the ",
          link("style-transition system", to = "styles#transitions"),
          " — one solver, consistent timing across both."),

        p("Tween velocity is reported by finite difference between consecutive \
           samples, so handing a mid-flight tween off to a spring gives the spring \
           a sensible seed velocity. Tweens do not themselves preserve incoming \
           velocity — the curve dictates the motion."),
    },

    section(heading = "Spring — physics that feels alive") {
        code(rust, r##"
            SpringTo::new(target)
                .stiffness(280)
                .damping(22)
        "##),

        p("Semi-implicit Euler integration over current value + velocity. Spring \
           inherits the value handle's velocity by default; override explicitly \
           when the gesture system has a measured throw velocity the framework \
           doesn't know about:"),

        code(rust, r##"
            value.animate(
                SpringTo::new(rest_target).initial_velocity(release_velocity)
            );
        "##),

        p("Defaults: ", code("stiffness=170"), ", ", code("damping=26"),
          ", ", code("mass=1.0"), " — React Spring / Framer Motion defaults that \
           sit just under critical damping for a tiny overshoot and \"alive\" feel."),

        p("Settling thresholds (", code("rest_displacement"), ", ",
          code("rest_velocity"), ") are configurable. The defaults assume \
           normalized 0..1 ranges; bump them up for pixel-space work."),
    },

    section(heading = "Decay — fling and toss") {
        code(rust, r##"
            // User releases a drag with measured velocity.
            // The value drifts to rest on its own.
            value.animate(DecayFrom::new(release_velocity));
        "##),

        p("Velocity-driven exponential decay with no target. The resting value is \
           wherever momentum carries before friction wins. Frame-rate independent \
           (closed-form, not Euler-approximated)."),

        p("The pattern for flick-scroll, toss-to-dismiss, swipe-decelerate. Pair \
           with a ", code("Sequence"),
          " to add a follow-up spring if you want the decay to land *at* a \
           specific target after the fling."),
    },

    section(heading = "Sequence — back to back") {
        p("Run factories in order. Velocity flows across segment boundaries — a \
           tween into a spring continues smoothly:"),

        code(rust, r##"
            value.animate(
                SequenceFactory::new()
                    .then(TweenTo::new(1.0, Duration::from_millis(48)).linear())
                    .then(SpringTo::new(1.0).stiffness(180))
            );
        "##),

        p("When a segment finishes mid-frame the sequence advances to the next \
           segment using the same ", code("dt"),
          " slice. The advance loop caps at 64 segments per frame so a chain of \
           zero-duration ", code("SnapTo"), "s can't lock up."),
    },

    section(heading = "Loop — replay") {
        p("Replay an inner factory ", code("Repeat::Times(N)"), " or ",
          code("Repeat::Forever"), ":"),

        code(rust, r##"
            // Pulsing button — until cancelled.
            value.animate(LoopFactory::new(
                SequenceFactory::new()
                    .then(TweenTo::new(1.1, Duration::from_millis(120)).ease_out())
                    .then(TweenTo::new(1.0, Duration::from_millis(120)).ease_in()),
                Repeat::Forever,
            ));
        "##),

        p("Note there's no \"autoreverse\" flag — express ping-pong as a \
           two-segment sequence inside a loop. Springs and decays have no \
           canonical reverse, so the explicit form is the only one that works \
           for all factory types."),
    },

    section(heading = "Keyframes — multi-stop waypoints") {
        code(rust, r##"
            // Bounce-in: shoot past target, settle.
            value.animate(
                KeyframesTo::new(Duration::from_millis(400))
                    .stop(0.0, 0.0)
                    .stop(0.6, 1.1)
                    .stop(1.0, 1.0)
                    .curve(Easing::EaseOut)
            );
        "##),

        p("Stops are ", code("(offset, value)"), " pairs with offset in ",
          code("0..=1"),
          ". The framework sorts defensively if you feed them out of order. An \
           implicit ", code("(0.0, current)"),
          " is inserted if you don't anchor the start — so a single-stop \
           keyframes call reads as \"tween to target via this curve.\""),
    },

    section(heading = "Wait, SnapTo, and stagger") {
        p("The connective tissue:"),

        code(rust, r##"
            Wait::new(Duration::from_millis(200))  // hold value, finish after delay
            SnapTo::new(0.0_f32)                   // instant set, finish immediately
        "##),

        p("Both compose inside sequences and loops. ", code("SnapTo"),
          " is the rewind primitive when you want every loop iteration to start \
           from a known value; ", code("Wait"),
          " is how you express delays without a separate API."),

        p("The ", code("stagger(...)"), " helper applies a per-index delay across \
           a collection of values — internally a ", code("for"),
          " over ", code("(i, value)"), " that prepends ",
          code("Wait::new(step_delay * i)"), " to each factory:"),

        code(rust, r##"
            // 4 cards springing in, 40ms between each.
            stagger(&card_scales, Duration::from_millis(40), |_i| {
                SpringTo::new(1.0).stiffness(220).damping(20)
            });
        "##),
    },

    section(heading = "Velocity-preserving handoff") {
        p("The behavioural feature that makes this system different from a plain \
           interpolation library."),

        p("When ", code("animate(new_factory)"),
          " runs on a value that's already animating, the framework reads the \
           value handle's current ", code("(value, velocity)"),
          ", calls ", code("new_factory.build(value, velocity)"),
          ", and replaces the animator. The new animator's first frame reflects \
           the inherited motion."),

        p("Concretely: a gesture drag updates the value via ", code("set"),
          " each frame. On release, the gesture system measures throw velocity \
           and calls ", code("value.animate(SpringTo::new(rest).initial_velocity(v))"),
          ". The spring's first frame moves *in the direction of the throw*, \
           then settles toward the target. That's the \"iOS feels right\" \
           behaviour, derived from a single architectural seam."),

        p(code("cancel()"),
          " stops the running animator but preserves velocity in the value \
           handle — useful when you want to defer animator selection (\"which \
           spring depends on something we don't know yet\") but keep momentum \
           alive for the eventual ", code("animate(...)"), " call."),
    },

    section(heading = "Backend integration") {
        p("The vocabulary of animatable properties:"),

        code(rust, r##"
            pub enum AnimProp {
                // Scalar (f32)
                Opacity,
                TranslateX, TranslateY,
                Scale, ScaleX, ScaleY,
                RotateZ,
                // Color ([f32; 4] sRGB)
                BackgroundColor,
                ForegroundColor,
            }
        "##),

        p("Backends opt into animation support by implementing two trait methods:"),

        code(rust, r##"
            fn set_animated_f32(
                &mut self,
                node: &Self::Node,
                prop: AnimProp,
                value: f32,
            );

            fn set_animated_color(
                &mut self,
                node: &Self::Node,
                prop: AnimProp,
                value: [f32; 4],
            );
        "##),

        p("Both default to no-op so unmodified backends remain author-portable. \
           A value handle keeps ticking and its listener fires — the backend just \
           doesn't paint the change. Mis-routing a color prop through the f32 \
           path is a silent no-op too: programmer error, not a runtime crash."),

        p("Status across the bundled backends:"),

        list(
            [code("backend-web"),
             " — all props. Uses modern CSS individual transform properties (",
             code("translate"),
             ", ", code("scale"), ", ", code("rotate"),
             ") so opacity, transform components, and colors each write inline \
              via ", code("style.setProperty"),
             " with a small per-node cache for the composed pair properties."],
            [code("backend-ios-mobile"),
             " — all props. Per-view ", code("AnimatedTransformState"),
             " composes a ", code("CGAffineTransform"),
             " from translate/scale/rotate state and writes via ",
             code("setTransform:"),
             "; opacity, colors, tint go to ", code("setAlpha:"),
             " / ", code("setBackgroundColor:"), " / ", code("setTintColor:"),
             "."],
            [code("backend-android-mobile"),
             " — all scalar + ", code("BackgroundColor"), "; ",
             code("ForegroundColor"),
             " best-effort. Each ", code("AnimProp"),
             " maps to one ", code("View"),
             " setter via JNI (", code("setAlpha"), ", ", code("setTranslationX"),
             ", ", code("setScaleX"), ", ", code("setRotation"),
             ", …) — no composition state needed because Android exposes the \
              components individually."],
        ),

        p("Authors can wire any animated value to any backend property today:"),

        code(rust, r##"
            let _sub = value.subscribe_and_apply({
                let backend = backend.clone();
                let node = node.clone();
                move |v, _vel| {
                    backend
                        .borrow_mut()
                        .set_animated_f32(&node, AnimProp::Scale, *v);
                }
            });
        "##),

        p("Per-primitive sugar like ", code("view().scale_animated(&v)"),
          " is intentionally out of core — it's a peripheral builder concern that \
           composes on top of the core surface."),
    },

    section(heading = "animated! — value-handle constructor") {
        p(code("animated!(initial)"), " is sugar for ",
          code("AnimatedValue::new(initial)"),
          ". The type parameter is inferred from the initial \
           value, so it scales cleanly across scalar and color \
           handles:"),

        code(rust, r##"
            use framework_core::animated;

            let opacity = animated!(0.0_f32);                // AnimatedValue<f32>
            let scale   = animated!(1.0_f32);
            let color   = animated!((0.0, 0.0, 0.0, 1.0));    // AnimatedValue<(f32, f32, f32, f32)>
        "##),
    },

    section(heading = "timeline! — declarative multi-phase schedule") {
        p("The macro to reach for when you're animating several \
           properties across distinct moments in time. Each ",
          code("at => { ... }"),
          " clause fires every ", code("av: animator"),
          " pair simultaneously at that moment. The macro auto- \
           clones each handle into its task closure and returns \
           a ", code("Vec<ScheduledTask>"), " for the caller to \
           keep alive:"),

        code(rust, r##"
            use framework_core::{animated, effect, on_cleanup, timeline};
            use framework_core::animation::{SpringTo, TweenTo};
            use std::time::Duration;

            let opacity = animated!(0.0_f32);
            let scale   = animated!(0.95_f32);
            let translate_y = animated!(24.0_f32);

            effect!({
                let tasks = timeline! {
                    400 => {
                        opacity: TweenTo::new(1.0, Duration::from_millis(700)).ease_out(),
                        scale:   SpringTo::new(1.0).stiffness(170.0).damping(22.0),
                        translate_y: SpringTo::new(0.0).stiffness(170.0).damping(22.0),
                    },
                    2_400 => {
                        opacity: TweenTo::new(0.0, Duration::from_millis(500)).ease_in_out(),
                        translate_y: TweenTo::new(-28.0, Duration::from_millis(500)).ease_in_out(),
                    },
                };
                on_cleanup(move || drop(tasks));
            });
        "##),

        p("Each clause is one ", code("after_ms"),
          " task per ", code("av: animator"),
          " pair — the macro hides the per-task clone + push + \
           closure boilerplate that you'd otherwise write by \
           hand. The phase boundaries (the times on the left of ",
          code("=>"),
          ") read as visual chapter markers; the inner pairs \
           read like a struct literal."),

        p("Constraints worth knowing:"),
        list(
            [code("$av"), " must be a bare identifier (",
             code("opacity"), ", ", code("welcome_color"),
             "). The macro writes ", code("$av.clone()"),
             " — field-access expressions don't match the ident \
              pattern. For those, use the single-task ",
             code("animate_at!"), " primitive directly."],
            ["Time expressions on the left of ", code("=>"),
             " can be any ", code("i32"), "-yielding expression: \
              literals, ", code("act_2_start + 200"),
             ", named consts, whatever's in scope."],
            ["The returned Vec is yours to manage. Inside an ",
             code("effect!"), " the canonical pattern is ",
             code("on_cleanup(move || drop(tasks))"),
             " so the timers stay alive for the scope's lifetime \
              and are cancelled together when the scope drops."],
        ),
    },

    section(heading = "animate_at! — single-task primitive") {
        p("The building block ", code("timeline!"),
          " expands to. Use it when you want one scheduled \
           animation but not a full multi-phase block:"),

        code(rust, r##"
            use framework_core::{animate_at, on_cleanup};
            use framework_core::animation::SpringTo;

            effect!({
                let task = animate_at!(
                    800,
                    opacity,
                    SpringTo::new(1.0).stiffness(180.0).damping(20.0),
                );
                on_cleanup(move || drop(task));
            });
        "##),

        p("Returns a single ", code("ScheduledTask"),
          ". Unlike ", code("timeline!"),
          ", the AnimatedValue source can be any expression — \
           the clone is emitted as ", code("($av).clone()"),
          " around your expression. Reach for it when the av is \
           accessed through indirection (struct fields, closures) \
           that ", code("timeline!"), "'s ident pattern can't bind."),
    },

    section(heading = "AnimationClock and threading") {
        p("The clock is a per-thread tick registry. The first ",
          code("animate(...)"), " call on any value triggers an installation of ",
          code("Scheduler::raf_loop"),
          "; the loop walks every live animator each frame and unregisters those \
           that report ", code("finished"),
          ". When the last animation finishes the loop's handle drops — the system \
           idles to zero per-frame work."),

        p("Single-threaded by design. ", code("AnimatedValue"),
          " is ", code("Rc + RefCell"),
          " — same model as ", code("Signal"),
          ". Off-thread animation is a future-tier concern (delegating to the \
           platform compositor's own animator) and isn't wired today."),

        p("Tests can drive the clock without a scheduler installed via ",
          code("tick_for_test(dt)"), "."),
    },

    section(heading = "When to use which") {
        p("Quick decision table:"),

        list(
            ["Hover/focus/state chrome → ", link("style transitions", to = "styles#transitions"),
             ". Declarative, browser-driven on web."],
            ["Button press feedback / pulsing / loading idle → ",
             code("LoopFactory"), " + sequence."],
            ["Pull-to-dismiss / swipe-decline / drag-to-snap → gesture writes via ",
             code("value.set(...)"),
             "; release does ", code("value.animate(SpringTo::new(rest))"),
             " with the measured throw velocity."],
            ["Flick-scroll / toss / fling → ", code("DecayFrom::new(velocity)"), "."],
            ["Multi-step choreography (fade then scale then settle) → ",
             code("SequenceFactory"), " with the segments you want."],
            ["List intro / cascade → ", code("stagger(...)"),
             " over a collection of value handles."],
            ["Bounce/elastic/anticipation curve that you want to author by \
              waypoint → ", code("KeyframesTo"), "."],
        ),
    },

    section(heading = "What's intentionally outside core") {
        list(
            ["Per-primitive builder methods (", code("view().opacity_animated(&v)"),
             "). These touch the primitive enum and walker — best built as a \
              peripheral library above core."],
            ["Walker-integrated subscription lifetimes. Today the ",
             code("Subscription"),
             " returned by ", code("subscribe"),
             " is owned by your code; a wrapper could tie it to a scope's \
              lifetime."],
            ["Native-resident \"shared values\" à la Reanimated (skipping the \
              per-frame Rust↔backend round-trip entirely for gesture-bound \
              properties). That's a more ambitious tier that would reuse the \
              factory + ", code("AnimProp"),
             " vocabulary on top of what's here today."],
        ),
    },

    section(heading = "Module map") {
        p("Everything lives under ", code("framework_core::animation"),
          ". The pieces split as:"),

        list(
            [code("animatable"), " — the ", code("Animatable"), " trait + impls."],
            [code("animator"), " — ", code("Animator"), " / ",
             code("AnimatorFactory"), " / ", code("Sample"), "."],
            [code("tween"), ", ", code("spring"), ", ", code("decay"),
             " — the three built-in animator families."],
            [code("sequence"), ", ", code("repeat"), ", ", code("keyframes"),
             ", ", code("combinators"),
             " — composition primitives, ", code("Wait"),
             ", ", code("SnapTo"), ", ", code("stagger"), "."],
            [code("prop"), " — ", code("AnimProp"),
             " enum and family helpers."],
            [code("clock"), " — the per-thread tick registry and ",
             code("tick_for_test"), "."],
            [code("value"), " — ", code("AnimatedValue<T>"),
             " and ", code("Subscription<T>"), "."],
            [code("curve"), " — ", code("apply_easing"), " + Newton-Raphson cubic-Bézier \
             solver, shared with the style transition system."],
        ),
    },
}
