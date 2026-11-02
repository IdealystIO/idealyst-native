//! `#[wasm_split(module, fallible)]` off wasm32: the body is compiled in and
//! the wrapper returns `Ok`. (The wasm32 `Err` path is covered by the loader
//! unit tests in `src/lib.rs` plus the `#[component(lazy)]` tests that drive
//! a failing loader through the vocabulary.)

// The macro's expansion names the runtime as `wasm_split::…`.
use wasm_splitter as wasm_split;

#[wasm_split::wasm_split(module_a, fallible)]
async fn add((a, b): (u32, u32)) -> u32 {
    a + b
}

#[wasm_split::wasm_split(module_b, fallible)]
async fn unit(_: ()) {}

#[test]
fn fallible_split_fn_returns_ok_natively() {
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    let mut fut = std::pin::pin!(add((2, 3)));
    let r: Result<u32, wasm_split::SplitLoaderError> = match fut.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(r) => r,
        std::task::Poll::Pending => panic!("native body resolves immediately"),
    };
    assert_eq!(r.unwrap(), 5);
    let mut fut = std::pin::pin!(unit(()));
    assert!(matches!(fut.as_mut().poll(&mut cx), std::task::Poll::Ready(Ok(()))));
}
