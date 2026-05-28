//! Platforms — what each backend looks like and where it runs.

use runtime_core::{ui, Primitive};
use idea_ui::{Typography, Card};

use crate::shell::{PageBody, PageHeader, Section, PageTypographyProps, PageHeaderProps, SectionProps};

pub fn page() -> Primitive {
    ui! {
        PageBody {
            PageHeader(
                title = "Platforms".to_string(),
                description = "Idealyst targets web, iOS, Android, Roku, and runtime-server — same Rust \
                               crate, different backend.".to_string(),
            )

            Section(
                title = "How backends fit".to_string(),
                body = "Every backend implements the `Backend` trait — `create_view`, \
                        `create_text`, navigator/drawer/tabs constructors, and so on. The \
                        framework's render walker calls those functions to materialize the \
                        cross-platform tree into native nodes. Your app crate has no idea \
                        which backend it's running under.".to_string(),
            )

            Card {
                Typography(content = "Web (WASM)".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "`backend-web` compiles to WebAssembly via `wasm-bindgen`. \
                               Primitives map to DOM nodes; stylesheets compile to CSS \
                               classes; navigators integrate with the History API so browser \
                               back/forward and deep links work. The dev server serves the \
                               WASM bundle and connects a hot-reload WebSocket for instant \
                               patching.".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "iOS (UIKit)".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "The `backend-ios-*` family (`-core`, `-mobile`, `-tv`) \
                               binds to UIKit via the `objc2` crate. Views become \
                               `UIView`s; navigators wrap `UINavigationController`; \
                               tabs wrap `UITabBarController`; the drawer is hand-rolled \
                               because UIKit doesn't ship a stock drawer. Build via the CLI \
                               (`idealyst build --ios`) or open the materialized \
                               Xcode project under `target/idealyst/ios/`.".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "Android".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "The `backend-android-*` family (`-core`, `-mobile`, `-tv`) \
                               bridges to the native View hierarchy via JNI. The Rust \
                               crate compiles to a `.so` the Android app loads; the \
                               framework populates the layout when the activity hands \
                               it a root container. Drawer maps to `DrawerLayout`, tabs \
                               to `BottomNavigationView`, stack to `FragmentManager`.".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "Roku (BrightScript)".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "Roku has no Rust runtime — `backend-roku` is a code generator. \
                               Your Rust app is rebuilt as a declarative model, serialized, \
                               and shipped to the device alongside generated BrightScript \
                               glue that replays the tree against Roku's SceneGraph. The \
                               model handles reactivity via a small in-Roku runtime in \
                               `crates/build/roku/runtime/`.".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "runtime-server — App-as-Server".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "runtime-server runs the Rust app on a server and ships a wire-format \
                               command stream to thin clients. The server owns state and \
                               business logic; clients render. Fresh clients receive a \
                               snapshot (a `SceneModel`) rather than the full command log, \
                               so reconnects are O(scene) not O(history). The dev server \
                               uses runtime-server internally for hot-reload — your saves rebuild a \
                               dylib, the runtime-server server reloads it, and connected clients see \
                               the diff with no full reload.".to_string(),
                    muted = true,
                )
            }

            Section(
                title = "Picking a target".to_string(),
                body = "Most apps start on the web — `idealyst dev` is the fastest feedback \
                        loop. Add native targets when you need platform-specific surfaces \
                        (camera, push notifications, App Store distribution). The same \
                        `app()` function compiles into all of them; only the wrapper crates \
                        differ.".to_string(),
            )
        }
    }
}
