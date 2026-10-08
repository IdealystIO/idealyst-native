//! Install the CLI — prerequisites, install command, verify, per-platform tooling.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::QUICKSTART_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let prereqs: Ref<ViewHandle> = Ref::new();
    let install_ref: Ref<ViewHandle> = Ref::new();
    let verify_ref: Ref<ViewHandle> = Ref::new();
    let per_platform_ref: Ref<ViewHandle> = Ref::new();
    let next_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: prereqs, label: "Prerequisites" },
        TocEntry { handle: install_ref, label: "Install" },
        TocEntry { handle: verify_ref, label: "Verify" },
        TocEntry { handle: per_platform_ref, label: "Per-platform tooling" },
        TocEntry { handle: next_ref, label: "Next steps" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Install the CLI",
                blurb = "The `idealyst` CLI is the entry point for scaffolding projects, \
                 running the dev server, and building per-platform releases. \
                 It's built from source by cargo, from the framework's registry.",
            )
            PageSection(handle = prereqs) { prerequisites() }
            PageSection(handle = install_ref) { install() }
            PageSection(handle = verify_ref) { verify() }
            PageSection(handle = per_platform_ref) { per_platform() }
            PageSection(handle = next_ref) { next_steps() }
        }
    };
    layout_with_toc(content, toc)
}

fn prerequisites() -> Element {
    let snippet = "# rustup is the standard Rust installer\n\
                   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Prerequisites".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "You need a Rust toolchain (stable 1.78+). The CLI itself \
                has no platform dependencies \u{2014} per-platform tooling (Xcode, Android \
                NDK, wasm-pack) is only required when you actually build for that \
                target.".to_string())
            Typography(content = "If you don't have Rust yet, install it via rustup:".to_string())
            CodePanel(src = snippet)
        }
    }
}

fn install() -> Element {
    let snippet = "cargo install idealyst-cli --index sparse+https://crates.idealyst.io/index/ --locked";
    let update_snippet = "idealyst update          # rebuild the newest release over this binary\n\
                          idealyst update --check  # only report whether one exists";
    let git_snippet = "# An unreleased commit, straight from git:\n\
                       export CARGO_REGISTRIES_IDEALYST_INDEX=sparse+https://crates.idealyst.io/index/\n\
                       cargo install --git https://github.com/IdealystIO/idealyst-native --rev <sha> idealyst-cli";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Install".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "The CLI is published to the framework's registry. This \
                builds the newest release and drops the `idealyst` binary into \
                `~/.cargo/bin/` (which is on your PATH if you set up Rust through \
                rustup) \u{2014} no other configuration needed:".to_string())
            CodePanel(src = snippet)
            Typography(content = "To move to a newer release later, let the CLI update \
                itself. It installs into the same place the running binary came from:".to_string())
            CodePanel(src = update_snippet)
            Typography(content = "To try a commit that hasn't been released yet, install \
                from git. The environment variable tells cargo where the framework's \
                own crates live:".to_string())
            CodePanel(src = git_snippet)
        }
    }
}

fn verify() -> Element {
    let snippet = "idealyst --help";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Verify".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Confirm the binary is on your PATH and prints the \
                subcommand list:".to_string())
            CodePanel(src = snippet)
            Typography(content = "You should see `new`, `init`, `dev`, `build`, `run`, \
                `update`, and a few others.".to_string())
        }
    }
}

fn per_platform() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Per-platform tooling".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "You only need a platform's tooling when you actually \
                build for that platform. The CLI is platform-agnostic; when a build is \
                missing something for a target, the platform builder reports what it \
                couldn't find.".to_string())
            Typography(content = "iOS".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "Xcode (App Store) + Xcode Command Line Tools. Both \
                ship together. `xcrun simctl` and `xcodebuild` need to be available on \
                your PATH \u{2014} they are by default once Xcode is installed.".to_string())
            Typography(content = "Android".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "Android Studio (or the SDK + NDK installed separately). \
                The CLI looks for `ANDROID_HOME` and `ANDROID_NDK_ROOT`; if neither is \
                set, the Android build reports what it couldn't find. You also need `adb` \
                on your PATH.".to_string())
            Typography(content = "Web".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "Nothing extra. The CLI pulls in wasm-pack as part of \
                its own build, and the wasm32 target compiles via your existing Rust \
                toolchain.".to_string())
        }
    }
}

fn next_steps() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Next steps".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "With the CLI installed, scaffold your first project and \
                run it on all three platforms in a few commands.".to_string())
            link(route = &QUICKSTART_ROUTE, params = ()) {
                Typography(content = "Go to the Quickstart \u{2192}".to_string())
            }
        }
    }
}
