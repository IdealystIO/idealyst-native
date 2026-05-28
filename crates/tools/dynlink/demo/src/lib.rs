//! Minimal demo for `idealyst build --web --dynamic-split`.
//!
//! The shell `Text` lives in the main bundle; the `lazy! { … }` body is
//! compiled into its own PIC `--shared` side module and dynamically linked
//! on first mount. Build with:
//!
//! ```text
//! idealyst build --web --dynamic-split   # (run from this dir)
//! ```

use runtime_core::{lazy, ui, Element, IntoElement};

pub fn app() -> Element {
    ui! {
        View {
            Text { "Always loaded shell (main bundle)" }
            {
                lazy! {
                    ui! {
                        View {
                            Text { "Hello from a dynamically-linked lazy chunk!" }
                        }
                    }
                }
                .into_element()
            }
        }
    }
}

/// SDK-handler registration hook the CLI-generated wrapper invokes before
/// mount. This demo registers no third-party SDKs.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
