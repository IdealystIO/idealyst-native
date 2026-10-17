//! Platforms — what each backend looks like and where it runs.

use runtime_core::{ui, Element};
use idea_ui::{Typography, Card};

use crate::shell::{PageBody, PageHeader, Section, PageTypographyProps, PageHeaderProps, SectionProps};

pub fn page() -> Element {
    ui! {
        PageBody {
            PageHeader(
                title = "Platforms".to_string(),
                description = "Idealyst targets web, iOS, Android, macOS, Linux, Windows, Roku, and \
                               more — same Rust crate, different backend.".to_string(),
            )

            Section(
                title = "How backends fit".to_string(),
                body = "Every backend implements the `Host` trait plus the capability \
                        traits — `create_view`, `create_text`, navigator constructors, and \
                        so on. The framework's realize pass calls those functions to \
                        materialize the cross-platform tree into native nodes. Your app \
                        crate has no idea which backend it's running under.".to_string(),
            )

            Card {
                Typography(content = "Web (WASM)".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "`backend-web` compiles to WebAssembly and reaches the browser through \
                               the framework's own `web-glue` bindings. \
                               Primitives map to DOM nodes; stylesheets compile to CSS \
                               classes; navigators integrate with the History API so browser \
                               back/forward and deep links work. The dev server serves the \
                               WASM bundle and connects a hot-reload WebSocket for instant \
                               patching.".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "iOS (UIKit)".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "`backend-ios-mobile` (on `backend-ios-core`) binds to UIKit \
                               via the `objc2` crate. Views become `UIView`s; the stack \
                               navigator presents through `UINavigationController`. \
                               `idealyst build --ios` produces the static library from a \
                               generated wrapper crate under \
                               `target/idealyst/<project>/ios/wrapper/`; `idealyst run ios` \
                               builds, installs, and launches it. tvOS is not supported \
                               (`backend-ios-tv` is an empty skeleton).".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "Android".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "`backend-android-mobile` (on `backend-android-core`) bridges \
                               to the native View hierarchy via JNI. The Rust crate \
                               compiles to a `.so` the Android app loads; the framework \
                               populates the layout when the activity hands it a root \
                               container. The stack navigator presents screens in a \
                               view-level container with the platform's Back handling. \
                               Android TV is not supported (`backend-android-tv` is an \
                               empty skeleton).".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "Roku (BrightScript)".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "Roku has no Rust runtime — `backend-roku` is a command \
                               emitter: each capability call becomes a JSON command. \
                               `idealyst build --roku` runs your `app()` once, snapshots \
                               the commands, and packages them with a small BrightScript \
                               runtime (in \
                               `crates/tools/build/roku/runtime/`) that replays it against \
                               SceneGraph, plus your `#[method]` functions transpiled to \
                               BrightScript. Experimental: there is no dev loop, and button \
                               events are not yet sent back from the device.".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "runtime-server — App-as-Server".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "runtime-server is the default mode of `idealyst dev`. Your app \
                               runs in one sidecar process on your machine and ships a \
                               wire-format command stream to thin clients on each platform; \
                               the sidecar owns state, clients render. Fresh clients receive \
                               a snapshot (a `SceneModel`) rather than the full command log, \
                               so reconnects are O(scene) not O(history). A body-only save \
                               is hot-patched into the running sidecar and its tree re-runs \
                               with signal values carried over; a shape change rebuilds and \
                               restarts the sidecar while clients keep their connections. \
                               It is a development tool, not a production deployment \
                               model.".to_string(),
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
