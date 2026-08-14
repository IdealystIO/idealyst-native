//! `idealyst::entry!` — the app's platform entry point.
//!
//! # Why this is a macro at all
//!
//! Almost nothing here needs to be. The per-platform boot sequences
//! that the CLI used to *generate* into wrapper crates are ordinary
//! functions in `idealyst::boot`, so they can be read, tested and
//! stepped through like any other code. Two things resist that:
//!
//! 1. **Manifest metadata.** `[package.metadata.idealyst.app]` (app
//!    name, bundle id, mount selector, window size, …) lives in the
//!    *consuming* crate's `Cargo.toml`. A library function can't see
//!    it; a proc macro can, because `CARGO_MANIFEST_DIR` at expansion
//!    time is the consumer's directory.
//! 2. **`main` itself.** Only the binary crate can define it.
//!
//! So the macro reads the manifest, bakes an [`AppConfig`] literal, and
//! emits a `main` that calls into `idealyst::boot`. That's it — the
//! expansion is a couple of dozen lines regardless of platform.
//!
//! # Escape hatch
//!
//! The macro is a convenience, not a requirement. Anything it does can
//! be written by hand:
//!
//! ```ignore
//! // A wasm app that wants the small allocator declares it itself —
//! // the macro has no privileged access, it just reads the manifest.
//! #[global_allocator]
//! static ALLOCATOR: idealyst::alloc::Small = idealyst::alloc::small();
//!
//! fn main() {
//!     idealyst::boot::run::<MyExtensions>(
//!         || my_app::app(),
//!         idealyst::AppConfig { name: "My App", ..Default::default() },
//!     );
//! }
//! ```
//!
//! That matters because a generated wrapper you can't see is exactly
//! what this refactor exists to delete. Don't replace it with a macro
//! you can't see either.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};

/// Declare the app's entry point.
///
/// ```ignore
/// // src/main.rs
/// idealyst::entry!(my_app);
/// ```
///
/// where `my_app` is the app's library crate — the one exposing
/// `pub fn app() -> Element` and `pub fn register_scene_extensions`.
///
/// Platform selection is **config, not code**: the target triple picks
/// web/iOS/Android, and for the several shells that share a triple
/// (a macOS terminal app and a macOS window app are both
/// `target_os = "macos"`) the `idealyst` crate's feature selects
/// between them. Nothing in this macro's input names a platform, so
/// one `src/main.rs` serves every target.
#[proc_macro]
pub fn entry(input: TokenStream) -> TokenStream {
    let EntryInput { app_crate } = syn::parse_macro_input!(input as EntryInput);

    let config = match read_app_config() {
        Ok(config) => config,
        Err(message) => {
            return syn::Error::new(proc_macro2::Span::call_site(), message)
                .to_compile_error()
                .into()
        }
    };
    let AppConfigTokens { name, bundle_id, mount_selector, cell_size, primitives, allocator } =
        config;

    // The builtin-primitive set. A declared list becomes a local
    // `builtin_set!` type; the default is the full vocabulary. This
    // replaces the CLI's old `--primitives` flag: the set is a property
    // of the app, not of one invocation of a build tool, and it has to
    // be a *type* at the boot call site for the dead-code elimination to
    // fire at all.
    let (builtin_decl, builtin_ty) = match primitives {
        None => (
            proc_macro2::TokenStream::new(),
            quote! { ::idealyst::runtime_vocabulary::AllBuiltins },
        ),
        Some(keep) => (
            quote! {
                ::idealyst::runtime_vocabulary::builtin_set!(pub __IdealystBuiltins: #keep);
            },
            quote! { __IdealystBuiltins },
        ),
    };

    // rustc has no idea this expansion read `Cargo.toml`, so a metadata
    // edit alone would not invalidate the cached expansion and the app
    // would keep booting with a stale name/bundle id. `include_str!`
    // registers a real file dependency, which is the supported way to
    // tell rustc "re-run me if this changes".
    let manifest_dep = quote! {
        const _: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    };

    quote! {
        #manifest_dep

        #builtin_decl

        #allocator

        /// The app's scene-registry seam, lifted to a type so it can
        /// cross a generic boundary.
        ///
        /// `register_scene_extensions` is generic over the backend
        /// `H`, and a generic function can't be passed as a value —
        /// each platform's boot has to monomorphize it against its own
        /// backend. Wrapping it in a zero-sized type implementing
        /// `SceneExtensions` lets `boot::run` stay one uniform
        /// signature across every platform.
        struct __IdealystExtensions;

        impl ::idealyst::SceneExtensions for __IdealystExtensions {
            fn register<H>(registry: &mut ::idealyst::runtime_scene::Registry<H>)
            where
                H: ::idealyst::SceneHost,
            {
                #app_crate::register_scene_extensions(registry);
            }
        }

        fn main() {
            ::idealyst::boot::run::<__IdealystExtensions, #builtin_ty>(
                || #app_crate::app(),
                ::idealyst::AppConfig {
                    name: #name,
                    bundle_id: #bundle_id,
                    mount_selector: #mount_selector,
                    cell_size: #cell_size,
                },
            );
        }
    }
    .into()
}

/// `entry!(my_app)` — the app's library crate.
struct EntryInput {
    app_crate: syn::Path,
}

impl Parse for EntryInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let app_crate: syn::Path = input.parse()?;
        // Tolerate a trailing comma so `entry!(my_app,)` isn't a
        // baffling parse error.
        if input.peek(syn::Token![,]) {
            let _: syn::Token![,] = input.parse()?;
        }
        if !input.is_empty() {
            return Err(input.error(
                "entry! takes exactly one argument: the app's library crate, e.g. `entry!(my_app)`",
            ));
        }
        Ok(Self { app_crate })
    }
}

/// The manifest-derived values, already as tokens.
#[derive(Debug)]
struct AppConfigTokens {
    name: proc_macro2::TokenStream,
    bundle_id: proc_macro2::TokenStream,
    mount_selector: proc_macro2::TokenStream,
    cell_size: proc_macro2::TokenStream,
    /// The `primitives` keep-list as a comma-separated ident sequence,
    /// ready to paste into `builtin_set!`. `None` means "everything".
    primitives: Option<proc_macro2::TokenStream>,
    /// The `#[global_allocator]` item, or empty for the platform
    /// default. A whole item rather than a type name because the
    /// default case emits *nothing* — there is no "the std allocator"
    /// type to name.
    allocator: proc_macro2::TokenStream,
}

/// Read `[package.metadata.idealyst.app]` from the consuming crate.
///
/// Every field is optional — an app with no metadata block at all gets
/// sensible defaults (package name, `ai.idealyst.<name>`, `#app`), so
/// `entry!` works on a bare `cargo new` crate. Only a manifest that
/// can't be read or parsed is an error, because that means the build
/// is broken in a way the app author needs to hear about rather than
/// silently boot with defaults.
fn read_app_config() -> Result<AppConfigTokens, String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| "CARGO_MANIFEST_DIR is unset — entry! must be expanded by cargo".to_string())?;
    let path = std::path::Path::new(&dir).join("Cargo.toml");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    // Every message from here names the manifest it came from — the
    // author is reading it in `main.rs`, one file removed from the file
    // that actually holds the mistake.
    parse_app_config(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parse manifest source into the tokens `entry!` bakes in.
///
/// Split from [`read_app_config`] so the metadata rules are testable
/// against a string instead of a fixture crate on disk — the validation
/// (allocator names, primitive idents, `cell_size` arity) is the part
/// that can be wrong, and it has nothing to do with file IO.
fn parse_app_config(raw: &str) -> Result<AppConfigTokens, String> {
    let manifest: toml::Value =
        toml::from_str(raw).map_err(|e| format!("could not parse manifest: {e}"))?;

    let package_name = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("idealyst-app")
        .to_string();

    let app = manifest
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("idealyst"))
        .and_then(|i| i.get("app"));

    let string_field = |key: &str| -> Option<String> {
        app.and_then(|a| a.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };

    let name = string_field("name").unwrap_or_else(|| package_name.clone());
    let bundle_id = string_field("bundle_id")
        .unwrap_or_else(|| format!("ai.idealyst.{}", package_name.replace('-', ".")));
    // Where a web build mounts. Kept configurable because an app
    // embedded in a host page rarely owns `#app`.
    let mount_selector = string_field("mount_selector").unwrap_or_else(|| "#app".to_string());

    // Terminal cell size, in px per character cell — used to translate
    // the layout engine's pixel geometry into a character grid. Absent
    // means "1px = 1 cell", which is what terminal-only apps want.
    let cell_size = match app.and_then(|a| a.get("cell_size")) {
        None => quote! { None },
        Some(value) => {
            let pair = value
                .as_array()
                .filter(|a| a.len() == 2)
                .and_then(|a| Some((as_f32(&a[0])?, as_f32(&a[1])?)))
                .ok_or_else(|| {
                    "package.metadata.idealyst.app.cell_size must be two numbers, e.g. \
                     cell_size = [8.0, 16.0]"
                        .to_string()
                })?;
            let (w, h) = pair;
            quote! { Some((#w, #h)) }
        }
    };

    // `primitives = ["view", "text"]` — the builtin keep-list. Names are
    // emitted as idents for `builtin_set!`, which is what validates
    // them: an unknown primitive is a macro error naming the offender,
    // in the app's own crate, rather than something to re-validate here
    // against a list that would drift.
    let primitives = match app.and_then(|a| a.get("primitives")) {
        None => None,
        Some(value) => {
            let names = value.as_array().ok_or_else(|| {
                "package.metadata.idealyst.app.primitives must be an array of \
                 primitive names, e.g. primitives = [\"view\", \"text\"]"
                    .to_string()
            })?;
            if names.is_empty() {
                return Err("package.metadata.idealyst.app.primitives is empty — remove the \
                            key to register the full vocabulary"
                    .to_string());
            }
            let idents = names
                .iter()
                .map(|n| {
                    let name = n.as_str().ok_or_else(|| {
                        "every entry in package.metadata.idealyst.app.primitives must be a \
                         string".to_string()
                    })?;
                    syn::parse_str::<syn::Ident>(name)
                        .map_err(|_| format!("{name:?} is not a valid primitive name"))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Some(quote! { #(#idents),* })
        }
    };

    let allocator = parse_allocator(app.and_then(|a| a.get("allocator")))?;

    Ok(AppConfigTokens {
        name: quote! { #name },
        bundle_id: quote! { #bundle_id },
        mount_selector: quote! { #mount_selector },
        cell_size,
        primitives,
        allocator,
    })
}

/// `allocator = "small"` — the app's wasm global allocator.
///
/// Emits the `#[global_allocator]` item, or nothing for the default.
///
/// This is per-app metadata rather than a cargo feature on `idealyst`
/// because a `#[global_allocator]` is a property of the *binary*: there
/// is one per process and no per-app override. Features are additive and
/// unify across a workspace, so a feature would let one crate pick the
/// allocator for every app built alongside it, silently. Metadata is
/// read from the crate that owns `main`, which is the same granularity
/// the allocator itself has.
///
/// The item is `cfg`'d to wasm32: the choice only exists for the web
/// bundle (`std`'s wasm32 default is `dlmalloc`, and `Small` is a wasm
/// allocator), while native shells use the system allocator. Declaring
/// it in a manifest whose app also builds natively is therefore fine —
/// the native builds ignore it rather than failing to compile.
fn parse_allocator(value: Option<&toml::Value>) -> Result<proc_macro2::TokenStream, String> {
    let Some(value) = value else {
        return Ok(proc_macro2::TokenStream::new());
    };
    let name = value.as_str().ok_or_else(|| {
        "package.metadata.idealyst.app.allocator must be a string — \
         \"default\" or \"small\""
            .to_string()
    })?;
    match name {
        // Spelling the default explicitly is allowed, and means the same
        // as leaving the key out: no `#[global_allocator]`, so `std`'s
        // wasm32 default (`dlmalloc`) stands.
        "default" => Ok(proc_macro2::TokenStream::new()),
        "small" => Ok(quote! {
            /// The app's global allocator, from
            /// `[package.metadata.idealyst.app].allocator`.
            ///
            /// A free list: ~10KB less code than `dlmalloc` in exchange
            /// for a linear scan per allocation. See
            /// `idealyst::alloc` for when that trade is the right one.
            #[cfg(target_arch = "wasm32")]
            #[global_allocator]
            static __IDEALYST_ALLOCATOR: ::idealyst::alloc::Small = ::idealyst::alloc::small();
        }),
        other => Err(format!(
            "package.metadata.idealyst.app.allocator = {other:?} is not a known allocator — \
             valid values are \"default\" (std's wasm32 default, dlmalloc) and \"small\" \
             (a free list: smaller bundle, linear scan per allocation)"
        )),
    }
}

/// TOML numbers arrive as integer OR float depending on how they were
/// written; `cell_size = [8, 16]` should work as well as `[8.0, 16.0]`.
fn as_f32(value: &toml::Value) -> Option<f32> {
    match value {
        toml::Value::Float(f) => Some(*f as f32),
        toml::Value::Integer(i) => Some(*i as f32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest with `[package.metadata.idealyst.app]` carrying
    /// `body`, which is where each test puts the key under test.
    fn manifest(body: &str) -> String {
        format!(
            "[package]\nname = \"my-app\"\n\n[package.metadata.idealyst.app]\n{body}\n"
        )
    }

    fn config(body: &str) -> AppConfigTokens {
        parse_app_config(&manifest(body)).expect("manifest should parse")
    }

    #[test]
    fn no_allocator_key_emits_no_global_allocator() {
        // The default has to stay *absent*, not "explicitly dlmalloc":
        // std already installs its wasm32 default, and emitting an item
        // here is what would make the choice unavoidable.
        assert!(config("").allocator.is_empty());
    }

    #[test]
    fn allocator_default_emits_no_global_allocator() {
        assert!(config(r#"allocator = "default""#).allocator.is_empty());
    }

    #[test]
    fn allocator_small_emits_wasm_gated_global_allocator() {
        let emitted = config(r#"allocator = "small""#).allocator.to_string();
        assert!(emitted.contains("# [global_allocator]"), "emitted: {emitted}");
        assert!(
            emitted.contains(":: idealyst :: alloc :: small ()"),
            "emitted: {emitted}"
        );
        // Without the cfg the static would break every native build of
        // an app whose manifest names an allocator — `alloc` is a
        // wasm-only module.
        assert!(
            emitted.contains("target_arch = \"wasm32\""),
            "emitted: {emitted}"
        );
    }

    #[test]
    fn unknown_allocator_names_the_valid_values() {
        let err = parse_app_config(&manifest(r#"allocator = "mimalloc""#))
            .expect_err("unknown allocator should be rejected");
        assert!(err.contains("mimalloc"), "err: {err}");
        assert!(err.contains("\"default\""), "err: {err}");
        assert!(err.contains("\"small\""), "err: {err}");
    }

    #[test]
    fn non_string_allocator_is_rejected() {
        let err = parse_app_config(&manifest("allocator = true"))
            .expect_err("non-string allocator should be rejected");
        assert!(err.contains("must be a string"), "err: {err}");
    }

    /// The allocator key is optional and independent — adding it must
    /// not disturb the fields that were already there.
    #[test]
    fn allocator_coexists_with_the_other_app_metadata() {
        let cfg = config(
            "name = \"My App\"\nmount_selector = \"#root\"\n\
             primitives = [\"view\", \"text\"]\nallocator = \"small\"\n",
        );
        assert_eq!(cfg.name.to_string(), "\"My App\"");
        assert_eq!(cfg.mount_selector.to_string(), "\"#root\"");
        assert_eq!(
            cfg.primitives.expect("keep-list").to_string(),
            "view , text"
        );
        assert!(!cfg.allocator.is_empty());
    }

    #[test]
    fn defaults_come_from_the_package_name() {
        let cfg = config("");
        assert_eq!(cfg.name.to_string(), "\"my-app\"");
        assert_eq!(cfg.bundle_id.to_string(), "\"ai.idealyst.my.app\"");
        assert_eq!(cfg.mount_selector.to_string(), "\"#app\"");
        assert_eq!(cfg.cell_size.to_string(), "None");
        assert!(cfg.primitives.is_none());
    }
}
