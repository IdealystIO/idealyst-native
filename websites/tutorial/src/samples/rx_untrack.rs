use runtime_core::{effect, untrack, Signal};

fn read_without_subscribing(query: Signal<String>, page: Signal<u32>) {
    effect!({
        let q = query.get(); // a dependency
        let p = untrack(|| page.get()); // read, but do NOT subscribe
        let _request = format!("{q}?page={p}");
    });
}
