use runtime_core::{ui, Element};

// `app()` is the root of your app. The platform shell calls it once at
// startup and mounts what it returns.
pub fn app() -> Element {
    ui! {
        view {
            text { "Hello, Idealyst!" }
        }
    }
}
