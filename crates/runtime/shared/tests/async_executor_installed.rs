//! `driver::async_executor_installed` reports whether `spawn_async` will
//! return at the first `.await` (an executor) or block (the native fallback).
#![cfg(all(feature = "async-driver", not(target_arch = "wasm32")))]

use std::future::Future;
use std::pin::Pin;

use runtime_shared::driver::{async_executor_installed, install_async_executor, AsyncExecutor};

struct Dropping;

impl AsyncExecutor for Dropping {
    fn spawn(&self, _future: Pin<Box<dyn Future<Output = ()> + 'static>>) {}
}

#[test]
fn reports_whether_an_executor_is_installed() {
    assert!(!async_executor_installed(), "nothing installed yet in this process");
    install_async_executor(Box::new(Dropping));
    assert!(async_executor_installed());
}
