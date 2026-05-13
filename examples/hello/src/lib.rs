//! The shared sample tree, used by every backend.

use framework_core::{button, component, signal, text, ui, view, Primitive, Signal};

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
    let score = signal!(0);
    let lives = signal!(3);
    let logged_in = signal!(false);
    let names = vec!["Ada", "Grace", "Linus"];

    ui! {
        Text { "Hello from idealyst-native" }

        Card(title = "Scores") {
            Counter(label = "Score (in card)", value = score)
            Counter(label = "Lives (in card)", value = lives)
        }

        Counter(label = "Score (view B)", value = score)
        Counter(label = "Score (view C, step=5)", value = score, step = 5)


        if logged_in.get() {
            Text { "Welcome back!" }
        } else {
            Button(
                label = "Login",
                on_click = move || logged_in.set(true)
            )
        }

        for n in &names {
            Text { format!("name: {}", n) }
        }

        for i in 0..10 {
            Text { i.to_string() }
        }

        Text { format!("Echo: score={}, lives={}", score.get(), lives.get()) }
    }
}
