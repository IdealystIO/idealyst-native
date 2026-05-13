//! The shared sample tree, used by every backend.

use framework_core::{button, children, component, text, view, when, Primitive, Signal};

pub struct CardProps {
    pub title: String,
    pub children: Vec<Primitive>,
}

#[component(children)]
pub fn card(props: CardProps) -> Primitive {
    let CardProps { title, children } = props;
    view(vec![
        text(title),
        view(children),
    ])
}

/// Props for [`counter`]. No Clone — borrowed props is the experiment.
pub struct CounterProps {
    pub label: String,
    pub value: Signal<i32>,
    pub step: i32,
}

#[component(default(step = 1))]
pub fn counter(props: &CounterProps) -> Primitive {
    view(vec![
        text(format!("{} (+{}): {}", props.label, props.step, props.value.get())),
        button("Increment", move || {
            let step = props.step;
            props.value.update(move |n| *n += step)
        }),
    ])
}

#[component]
pub fn app() -> Primitive {
    let score = Signal::new(0);
    let lives = Signal::new(3);

    // Form 1: pre-built props bound to a name, passed by reference.
    // step must be specified here because struct construction requires
    // every field — Form 1 is the "raw" route that bypasses defaults.
    let score_a = CounterProps { label: "Score (view A)".into(), value: score.clone(), step: 1 };

    // Static at render time — these would be props/state in a real app.
    let logged_in = Signal::new(false);
    let show_lives = false;
    let names = vec!["Ada", "Grace", "Linus"];

    view(children![
        text("Hello from idealyst-native"),

        // Card via macro form, holding nested counters.
        card!(
            title = "Scores".into(),
            children = children![
                counter!(label = "Score (in card)".into(), value = score.clone()),
                counter!(label = "Lives (in card)".into(), value = lives.clone()),
            ],
        ),

        counter(&score_a),
        counter!(label = "Score (view B)".into(), value = score.clone()),
        counter!(
            label = "Score (view C, step=5)".into(),
            value = score.clone(),
            step = 5
        ),

        // Conditional: present only when `show_lives` is true.
        show_lives.then(|| counter!(label = "Lives".into(), value = lives.clone())),

        // Reactive conditional: cond reads logged_in; clicking the Login
        // button flips the signal, which re-runs the cond and swaps subtrees.
        // Each closure clones its captured signals explicitly — the
        // #[component] macro doesn't auto-clone locals into when() yet.
        when(
            {
                let logged_in = logged_in.clone();
                move || logged_in.get()
            },
            || text("Welcome back!"),
            {
                let logged_in = logged_in.clone();
                move || {
                    let logged_in = logged_in.clone();
                    button("Login", move || logged_in.set(true))
                }
            },
        ),

        // A whole Vec from iteration, flattened inline.
        names.iter().map(|n| text(format!("name: {}", n))).collect::<Vec<_>>(),

        text(format!("Echo: score={}, lives={}", score.get(), lives.get())),
    ])
}
