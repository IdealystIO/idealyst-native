//! Ask the running `stream-serve` whether its current bundle fits the demo
//! app's props — the same check the demo runs before swapping a bundle in.
//!
//! `cargo run -p stream-spike --example check_served`

use stream_host::{Bundle, StreamEngine};
use stream_spike::{demo_props, fetch, host_exports};

fn main() {
    let url = std::env::var("STREAM_BUNDLE_URL").unwrap_or_else(|_| fetch::DEFAULT_URL.to_string());
    let served = fetch::fetch(&url).unwrap_or_else(|e| panic!("{e}"));
    let bundle = Bundle::load(&StreamEngine::new(), &served.wasm, host_exports()).unwrap_or_else(|e| panic!("{e}"));
    runtime_world::World::new().enter(|| {
        let props = demo_props(runtime_world::signal(0), runtime_world::signal(0));
        for (name, props) in props {
            match bundle.check(name, &props) {
                Ok(()) => println!("v{} {name}: compatible", served.version),
                Err(e) => println!("v{} {name}: REJECTED — {e}", served.version),
            }
        }
    });
}
