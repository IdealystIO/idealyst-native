//! iOS-backend handler stub for Drawer navigator. **Not yet wired** —
//! see `stack-navigator/src/ios.rs` for the port checklist; analog
//! applies here against `backend-ios-mobile/src/imp/tab_drawer.rs`'s
//! `create_drawer_navigator` impl (custom UIView overlay).

use backend_ios_mobile::IosBackend;

pub fn register(_backend: &mut IosBackend) {}
