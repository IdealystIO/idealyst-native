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
    let doctor_ref: Ref<ViewHandle> = Ref::new();
    let next_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: prereqs, label: "Prerequisites" },
        TocEntry { handle: install_ref, label: "Install" },
        TocEntry { handle: verify_ref, label: "Verify" },
        TocEntry { handle: per_platform_ref, label: "Per-platform tooling" },
        TocEntry { handle: doctor_ref, label: "Diagnose with doctor" },
        TocEntry { handle: next_ref, label: "Next steps" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Install the CLI",
                blurb = "The `idealyst` CLI is the entry point for scaffolding projects, \
                 running the dev server, building per-platform releases, and \
                 diagnosing your toolchain. It's installed from source via cargo.",
            )
            PageSection(handle = prereqs) { prerequisites() }
            PageSection(handle = install_ref) { install() }
            PageSection(handle = verify_ref) { verify() }
            PageSection(handle = per_platform_ref) { per_platform() }
            PageSection(handle = doctor_ref) { doctor() }
            PageSection(handle = next_ref) { next_steps() }
        }
    };
    layout_with_toc(content, toc)
}

fn prerequisites() -> Element {
    let snippet = "# rustup is the standard Rust installer\n\
                   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh";
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Prerequisites".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "You need a Rust toolchain (stable 1.78+) and git. \
                The CLI itself has no platform dependencies \u{2014} per-platform tooling \
                (Xcode, Android NDK, wasm-pack) is only required when you actually \
                build for that target.".to_string())
        },
        ui! {
            Typography(content = "If you don't have Rust yet, install it via rustup:".to_string())
        },
        ui! { CodePanel(src = snippet) },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn install() -> Element {
    let snippet = "cargo install --git https://github.com/IdealystIO/idealyst-native idealyst-cli";
    let pin_snippet = "# Pin to a specific commit / tag / branch:\n\
                      cargo install --git https://github.com/IdealystIO/idealyst-native --rev <sha>    idealyst-cli\n\
                      cargo install --git https://github.com/IdealystIO/idealyst-native --tag <tag>    idealyst-cli\n\
                      cargo install --git https://github.com/IdealystIO/idealyst-native --branch <br>  idealyst-cli";
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Install".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Fetch the latest commit on master, compile in release mode, \
                and drop the `idealyst` binary into `~/.cargo/bin/` (which is on your PATH if \
                you set up Rust through rustup):".to_string())
        },
        ui! { CodePanel(src = snippet) },
        ui! {
            Typography(content = "To pin to a specific revision instead of master:".to_string())
        },
        ui! { CodePanel(src = pin_snippet) },
        ui! {
            Typography(content = "Pass `--force` if you're upgrading over an existing copy of the CLI.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn verify() -> Element {
    let snippet = "idealyst --help";
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Verify".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Confirm the binary is on your PATH and prints the \
                subcommand list:".to_string())
        },
        ui! { CodePanel(src = snippet) },
        ui! {
            Typography(content = "You should see `new`, `init`, `dev`, `build`, `run`, \
                `doctor`, and a few others.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn per_platform() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Per-platform tooling".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "You only need a platform's tooling when you actually \
                build for that platform. The CLI is platform-agnostic; `idealyst doctor` \
                tells you what each enabled target is missing.".to_string())
        },
        ui! { Typography(content = "iOS".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Xcode (App Store) + Xcode Command Line Tools. Both \
                ship together. `xcrun simctl` and `xcodebuild` need to be available on \
                your PATH \u{2014} they are by default once Xcode is installed.".to_string())
        },
        ui! { Typography(content = "Android".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Android Studio (or the SDK + NDK installed separately). \
                The CLI looks for `ANDROID_HOME` and `ANDROID_NDK_ROOT`; if neither is \
                set, `idealyst doctor` will tell you. You also need `adb` on your PATH.".to_string())
        },
        ui! { Typography(content = "Web".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Nothing extra. The CLI pulls in wasm-pack as part of \
                its own build, and the wasm32 target compiles via your existing Rust \
                toolchain.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn doctor() -> Element {
    let snippet = "idealyst doctor";
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Diagnose with `doctor`".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "When something goes wrong, `idealyst doctor` walks \
                each enabled target's toolchain and reports what's missing, with \
                pointers to the install steps for each.".to_string())
        },
        ui! { CodePanel(src = snippet) },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn next_steps() -> Element {
    let title = ui! { Typography(content = "Next steps".to_string(), kind = idea_ui::typography_kind::H2) };
    let para = ui! {
        Typography(content = "With the CLI installed, scaffold your first project and \
            run it on all three platforms in a few commands.".to_string())
    };
    let cta = ui! {
        Link(route = &QUICKSTART_ROUTE, params = ()) {
            Typography(content = "Go to the Quickstart \u{2192}".to_string())
        }
    };
    let children: Vec<Element> = vec![title, para, cta];
    ui! { Stack(gap = StackGap::Md) { children } }
}
