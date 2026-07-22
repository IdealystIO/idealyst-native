//! AWS Lambda build orchestration for `idealyst build --serverless-lambda`.
//!
//! Mirror of `crates/tools/build/ssr` (and `web`/`ios`/`macos`): the user's
//! app crate stays platform-agnostic — it only exposes the usual
//! `pub fn app() -> Element` and its `#[server]` fns. Everything
//! Lambda-specific lives in an **ephemeral generated wrapper** under:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/serverless-lambda/wrapper/
//! ```
//!
//! whose `src/main.rs` hands `server::router()` to the AWS Lambda runtime via
//! the `server-aws` adapter (`server_aws::run()`).
//!
//! ## Why the force-link line matters
//!
//! `#[server]` fns register themselves through `inventory::submit!`. Per
//! inventory's contract the binary must *reference something* from the crate
//! that holds those registrations, or the linker never pulls the crate into
//! the link graph and `server::router()` registers **zero** routes — every
//! `/_srv/<fn>` then 404s with no build error. The generated `main.rs`
//! references `<lib>::app` for exactly this reason (the same anchor the SSR
//! wrapper uses); `server::router()` also warns at startup when it finds no
//! routes, as a backstop.
//!
//! ## Local testing = the real image (RIE), not LocalStack
//!
//! `build()` runs `cargo lambda build`, producing a `bootstrap` for the
//! `provided.al2023` custom runtime, and stages it next to a generated
//! `Dockerfile` FROM the AWS base image. That base image bundles the Lambda
//! **Runtime Interface Emulator**, so the *exact deployable image* runs and is
//! invoked locally the way Lambda invokes it — image-fidelity testing with no
//! AWS-service emulator.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

/// Target CPU architecture. `provided.al2023` runs on both; arm64 (Graviton)
/// is cheaper and the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    Arm64,
    X86_64,
}

impl Arch {
    /// The `cargo lambda build` architecture flag.
    fn cargo_lambda_flag(self) -> &'static str {
        match self {
            Arch::Arm64 => "--arm64",
            Arch::X86_64 => "--x86-64",
        }
    }

    /// The `docker build/run --platform` value for this arch, baked into the
    /// generated Dockerfile's usage comment so the image is built for the same
    /// arch the `bootstrap` was cross-compiled to.
    fn docker_platform(self) -> &'static str {
        match self {
            Arch::Arm64 => "linux/arm64",
            Arch::X86_64 => "linux/amd64",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm64",
            Arch::X86_64 => "x86_64",
        }
    }

    /// Parse the CLI `--arch` value. `None` (flag omitted) → the arm64 default.
    pub fn parse(s: Option<&str>) -> Result<Arch> {
        match s {
            None | Some("arm64") | Some("aarch64") => Ok(Arch::Arm64),
            Some("x86_64") | Some("x86-64") | Some("amd64") => Ok(Arch::X86_64),
            Some(other) => anyhow::bail!("unknown --arch {other:?}; use `arm64` or `x86_64`"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Build with the release profile. Lambda cold-start + cost both favour
    /// release; debug is offered only for a fast local compile check.
    pub release: bool,
    /// Target architecture (default arm64).
    pub arch: Arch,
    /// Where the wrapper Cargo.toml sources framework crates from.
    pub source: FrameworkSource,
    /// Extra cargo features to enable on the user crate, in addition to the
    /// always-on `server` feature the wrapper's dep line declares.
    pub user_features: Vec<String>,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// The `bootstrap` binary `cargo lambda build` produced (the Lambda
    /// handler for the `provided.al2023` custom runtime).
    pub bootstrap: PathBuf,
    /// Staging dir holding a copy of `bootstrap` + the generated `Dockerfile`
    /// — a self-contained `docker build` context and `cargo lambda deploy`
    /// source.
    pub deploy_dir: PathBuf,
    /// The generated wrapper crate dir (for debugging the generated source).
    pub wrapper_dir: PathBuf,
}

/// Build the user's project at `project_dir` as an AWS Lambda `bootstrap`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    ensure_cargo_lambda()?;

    let wrapper_dir = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join("serverless-lambda/wrapper");
    let bin_name = binary_name(&manifest.name);
    generate_wrapper(&wrapper_dir, &project_dir, &opts.source, &manifest)?;

    cargo_lambda_build(&wrapper_dir, &bin_name, &opts)?;

    // `cargo lambda build` writes to `<target>/lambda/<bin>/bootstrap`; the
    // wrapper's `.cargo/config.toml` redirects `<target>` to the shared
    // project/framework target dir (so deps aren't recompiled per wrapper).
    let bootstrap = opts
        .source
        .cargo_target_dir(&project_dir)
        .join("lambda")
        .join(&bin_name)
        .join("bootstrap");
    if !bootstrap.is_file() {
        anyhow::bail!(
            "cargo lambda build reported success but no bootstrap was produced at {}",
            bootstrap.display(),
        );
    }

    // Stage a self-contained deploy/test context under dist/.
    let deploy_dir = project_dir.join("dist").join("serverless-lambda");
    fs::create_dir_all(&deploy_dir)
        .with_context(|| format!("create deploy dir {}", deploy_dir.display()))?;
    let staged_bootstrap = deploy_dir.join("bootstrap");
    fs::copy(&bootstrap, &staged_bootstrap).with_context(|| {
        format!("stage bootstrap → {}", staged_bootstrap.display())
    })?;
    fs::write(deploy_dir.join("Dockerfile"), dockerfile(&bin_name, opts.arch))
        .with_context(|| format!("write Dockerfile in {}", deploy_dir.display()))?;

    Ok(BuildArtifact { bootstrap, deploy_dir, wrapper_dir })
}

/// Wrapper binary name. Suffixed with `-lambda` so it coexists with the other
/// per-target binaries the CLI generates (`<name>-ssr`, `<name>-macos`, …).
fn binary_name(project_name: &str) -> String {
    format!("{project_name}-lambda")
}

/// Fail early with an actionable message if `cargo lambda` isn't installed —
/// far friendlier than the raw "no such command: lambda" cargo emits.
fn ensure_cargo_lambda() -> Result<()> {
    let ok = Command::new("cargo")
        .args(["lambda", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        return Ok(());
    }
    anyhow::bail!(
        "`cargo lambda` is required for --serverless-lambda but was not found.\n\
         Install it with:  cargo install cargo-lambda   (or: brew install cargo-lambda)\n\
         Cross-compiling a Lambda from macOS also needs Zig (`brew install zig`), which \
         cargo-lambda uses as the linker."
    );
}

/// Materialize the wrapper crate at `wrapper_dir`. Idempotent — overwrites
/// whatever was there. Public so tests (and a future `idealyst scaffold`) can
/// drive the same generator.
pub fn generate_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let bin_name = binary_name(&manifest.name);
    // `server-aws` implies `server/server`; no extra features needed here.
    let saws_dep = source.dep("crates/api/server-aws", &[]);

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build --serverless-lambda`. Do not edit —
# rewritten every build.

[workspace]

[package]
name = "{bin_name}"
version = "0.0.1"
edition = "2021"

[[bin]]
name = "{bin_name}"
path = "src/main.rs"

[dependencies]
# The AWS Lambda adapter: wraps `server::router()` (a `tower::Service`) in the
# Lambda runtime. Pulls in `server/server`, so the user crate below compiles
# its real `#[server]` bodies.
server-aws = {saws_dep}
# User crate with the `server` feature ON — this is what compiles the
# `#[server]` bodies and emits their `inventory::submit!` route registrations.
{user_name} = {{ path = "{user_path}", features = ["server"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
{patch_block}
"#,
        bin_name = bin_name,
        saws_dep = saws_dep,
        user_name = manifest.name,
        user_path = project_dir.display(),
        patch_block = source.patch_block(),
    );

    let main_rs = format!(
        r#"//! GENERATED by `idealyst build --serverless-lambda`.
//!
//! Runs every linked `#[server]` fn as a single AWS Lambda, fronted by a
//! Function URL or API Gateway. HTTP `#[server]` fns (incl. `_batch`) port
//! as-is; `#[channel]`/`#[subscription]` (WebSockets) and `#[sse]` do NOT map
//! onto plain Lambda request/response and need separate adapters.
//!
//! FORCE-LINK: referencing `{lib}::app` puts the app lib into the link graph
//! so its `inventory::submit!` `#[server]` route registrations survive. Without
//! a reference into the lib the linker drops them, `server::router()` registers
//! zero routes, and every `/_srv/<fn>` 404s. (`server::router()` also warns at
//! startup when it finds no routes.)

#[tokio::main]
async fn main() -> Result<(), server_aws::Error> {{
    // Touch a symbol from the app lib; see FORCE-LINK note above.
    let _force_link = {lib}::app;
    let _ = _force_link;
    server_aws::run().await
}}
"#,
        lib = manifest.lib_name,
    );

    write_shared_target_config(wrapper_dir, project_dir, source)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

/// The Dockerfile staged alongside `bootstrap`. FROM the AWS base image, which
/// bundles the Lambda Runtime Interface Emulator (RIE) — so the same image
/// runs on real Lambda and locally.
pub fn dockerfile(bin_name: &str, arch: Arch) -> String {
    let platform = arch.docker_platform();
    format!(
        r#"# GENERATED by `idealyst build --serverless-lambda`.
#
# The AWS `provided.al2023` base image bundles the Lambda Runtime Interface
# Emulator (RIE), so THIS image is both the deployable artifact and a
# local test harness — no LocalStack needed:
#
#   docker build --platform {platform} -t {bin_name} .
#   docker run --rm --platform {platform} -p 9000:8080 {bin_name}
#
#   # Invoke it exactly the way Lambda does — POST a Function-URL event
#   # envelope to the RIE endpoint (here: a call to `/_srv/<fn>`):
#   curl -s "http://localhost:9000/2015-03-31/functions/function/invocations" \
#     -d '{{"version":"2.0","routeKey":"$default","rawPath":"/_srv/<fn>",
#          "requestContext":{{"http":{{"method":"POST","path":"/_srv/<fn>"}}}},
#          "headers":{{"content-type":"application/json"}},
#          "body":"{{\"input\":null}}","isBase64Encoded":false}}'
#
# Deploy the same bootstrap with `cargo lambda deploy` (see docs/serverless.md).
FROM public.ecr.aws/lambda/provided:al2023
COPY bootstrap ${{LAMBDA_RUNTIME_DIR}}/bootstrap
# Custom-runtime bootstraps ignore the handler arg; any value is fine.
CMD [ "handler" ]
"#,
    )
}

/// Redirect the wrapper crate's build output back into the shared `target/` so
/// common dependencies aren't recompiled per wrapper invocation.
fn write_shared_target_config(dir: &Path, project_dir: &Path, source: &FrameworkSource) -> Result<()> {
    let target_dir = source.cargo_target_dir(project_dir);
    let config = format!(
        "# GENERATED. Share the project's `target/` so common\n\
         # dependencies aren't recompiled per-wrapper.\n\
         \n\
         [build]\n\
         target-dir = \"{}\"\n",
        target_dir.display(),
    );
    fs::create_dir_all(dir.join(".cargo"))?;
    fs::write(dir.join(".cargo/config.toml"), config)?;
    Ok(())
}

fn cargo_lambda_build(wrapper_dir: &Path, bin_name: &str, opts: &BuildOptions) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["lambda", "build"]).current_dir(wrapper_dir);
    cmd.arg(opts.arch.cargo_lambda_flag());
    cmd.args(["--bin", bin_name]);
    if opts.release {
        cmd.arg("--release");
    }
    if !opts.user_features.is_empty() {
        cmd.arg("--features").arg(opts.user_features.join(","));
    }
    eprintln!(
        "[build serverless-lambda] cargo lambda build {} {}(in {})",
        opts.arch.cargo_lambda_flag(),
        if opts.release { "--release " } else { "" },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo lambda` — is cargo-lambda on your PATH?")?;
    if !status.success() {
        anyhow::bail!(
            "cargo lambda build failed for the serverless-lambda wrapper at {}",
            wrapper_dir.display(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod wrapper_template_tests {
    //! Shape regression for the generated wrapper + Dockerfile. Both are
    //! ephemeral generated artifacts, so drift is invisible until a user runs
    //! the build — these pin the load-bearing lines.

    use super::*;

    fn generated() -> (String, String) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("Cargo.toml"),
            "[package]\nname = \"demo-app\"\nversion = \"0.0.1\"\n",
        )
        .unwrap();
        let manifest = parse_manifest(&project_dir).expect("parse manifest");
        let source = FrameworkSource::Workspace { root: tmp.path().join("workspace") };
        generate_wrapper(&wrapper_dir, &project_dir, &source, &manifest).expect("generate");
        (
            fs::read_to_string(wrapper_dir.join("src/main.rs")).unwrap(),
            fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap(),
        )
    }

    #[test]
    fn wrapper_force_links_app_lib_and_runs_the_adapter() {
        let (main_rs, _cargo) = generated();
        // The force-link anchor — without it, zero routes register (silent 404s).
        assert!(
            main_rs.contains("let _force_link = demo_app::app;"),
            "wrapper must reference the app lib to keep #[server] registrations:\n{main_rs}",
        );
        assert!(
            main_rs.contains("server_aws::run().await"),
            "wrapper must hand the router to the Lambda runtime:\n{main_rs}",
        );
    }

    #[test]
    fn wrapper_cargo_pulls_server_aws_and_user_server_feature() {
        let (_main, cargo) = generated();
        assert!(cargo.contains("server-aws ="), "needs the AWS adapter dep:\n{cargo}");
        assert!(
            cargo.contains(r#"features = ["server"]"#),
            "user crate must compile with the `server` feature:\n{cargo}",
        );
        assert!(
            cargo.contains(r#"name = "demo-app-lambda""#),
            "bin name should be <project>-lambda:\n{cargo}",
        );
    }

    #[test]
    fn dockerfile_uses_rie_base_and_matches_arch() {
        let arm = dockerfile("demo-app-lambda", Arch::Arm64);
        assert!(arm.contains("public.ecr.aws/lambda/provided:al2023"), "RIE base image");
        assert!(arm.contains("COPY bootstrap"), "stages the handler binary");
        assert!(arm.contains("linux/arm64"), "arm64 platform in usage comment");

        let x86 = dockerfile("demo-app-lambda", Arch::X86_64);
        assert!(x86.contains("linux/amd64"), "x86_64 → linux/amd64");
    }

    #[test]
    fn arch_parses_aliases() {
        assert_eq!(Arch::parse(None).unwrap(), Arch::Arm64);
        assert_eq!(Arch::parse(Some("aarch64")).unwrap(), Arch::Arm64);
        assert_eq!(Arch::parse(Some("amd64")).unwrap(), Arch::X86_64);
        assert!(Arch::parse(Some("mips")).is_err());
    }
}
