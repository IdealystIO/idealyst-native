//! Android-backend handler stub for Tab navigator. **Not yet wired** —
//! see `stack-navigator/src/android.rs` for the port checklist; analog
//! applies here against
//! `backend-android-mobile/src/imp/primitives/tab_drawer.rs`'s
//! `create_tab_navigator` impl (BottomNavigationView).

use backend_android_mobile::AndroidBackend;

pub fn register(_backend: &mut AndroidBackend) {}
