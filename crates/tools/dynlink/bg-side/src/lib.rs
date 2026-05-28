use wasm_bindgen::prelude::*;
#[wasm_bindgen]
extern "C" { #[wasm_bindgen(js_namespace = console)] fn log(s: &str); }
#[wasm_bindgen]
pub fn side_hello(n: i32) { log(&format!("side: format! n={}", n)); }
