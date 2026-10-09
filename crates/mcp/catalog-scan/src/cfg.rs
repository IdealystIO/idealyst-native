//! `#[cfg(...)]` / `#[cfg_attr(...)]` evaluation.
//!
//! The scan has to keep exactly the items a host build of the crate would
//! compile: a component behind `#[cfg(target_arch = "wasm32")]` is absent
//! from the compiled catalog, and a pair of `cfg`-alternative definitions
//! must not both appear. So predicates are evaluated against the host's
//! `rustc --print cfg` set, the crate's enabled features, and
//! `debug_assertions` (the extractor is a dev build).
//!
//! Anything else is false: `test`, `doc`, and custom `--cfg` names a
//! project passes through `RUSTFLAGS` / `.cargo/config.toml`. The last is
//! the one place this can disagree with a build; catalog-macro items gated
//! on a custom cfg are not a pattern this framework uses.

use std::collections::BTreeSet;

use syn::punctuated::Punctuated;
use syn::{Attribute, Meta, Token};

/// The configuration predicates are evaluated against.
#[derive(Debug, Clone, Default)]
pub struct Cfg {
    names: BTreeSet<String>,
    pairs: BTreeSet<(String, String)>,
}

impl Cfg {
    /// Parse `rustc --print cfg` output (`unix`, `target_os="macos"`, …)
    /// and add `debug_assertions`, which `--print cfg` only reports when
    /// asked for a debug build.
    pub fn from_print_cfg(text: &str) -> Self {
        let mut cfg = Cfg::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            match line.split_once('=') {
                Some((k, v)) => {
                    cfg.pairs.insert((k.to_string(), v.trim_matches('"').to_string()));
                }
                None => {
                    cfg.names.insert(line.to_string());
                }
            }
        }
        cfg.names.insert("debug_assertions".to_string());
        cfg
    }

    /// The host configuration: `rustc --print cfg` (`$RUSTC` when set),
    /// run in `dir` so a `rust-toolchain.toml` there picks the compiler
    /// the project builds with.
    pub fn host_in(dir: &std::path::Path) -> std::io::Result<Self> {
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let out = std::process::Command::new(rustc).args(["--print", "cfg"]).current_dir(dir).output()?;
        if !out.status.success() {
            return Err(std::io::Error::other(format!(
                "rustc --print cfg failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(Self::from_print_cfg(&String::from_utf8_lossy(&out.stdout)))
    }

    /// This configuration plus `feature = "…"` for each of `features`.
    pub fn with_features<I: IntoIterator<Item = S>, S: Into<String>>(&self, features: I) -> Self {
        let mut cfg = self.clone();
        for f in features {
            cfg.pairs.insert(("feature".to_string(), f.into()));
        }
        cfg
    }

    /// Evaluate one predicate (`unix`, `feature = "x"`, `all(..)`, …).
    /// A predicate that does not parse is false, as an unknown name is.
    pub fn eval(&self, pred: &Meta) -> bool {
        match pred {
            Meta::Path(p) => p.get_ident().is_some_and(|i| self.names.contains(&i.to_string())),
            Meta::NameValue(nv) => {
                let (Some(key), syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(v), .. })) = (nv.path.get_ident(), &nv.value)
                else {
                    return false;
                };
                self.pairs.contains(&(key.to_string(), v.value()))
            }
            Meta::List(list) => {
                let Ok(args) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) else {
                    return false;
                };
                match list.path.get_ident().map(|i| i.to_string()).as_deref() {
                    Some("all") => args.iter().all(|m| self.eval(m)),
                    Some("any") => args.iter().any(|m| self.eval(m)),
                    Some("not") => args.len() == 1 && !self.eval(&args[0]),
                    _ => false,
                }
            }
        }
    }

    /// Whether an item carrying `attrs` is compiled: every `#[cfg(..)]` on
    /// it holds.
    pub fn enabled(&self, attrs: &[Attribute]) -> bool {
        attrs.iter().filter(|a| a.path().is_ident("cfg")).all(|a| a.parse_args::<Meta>().is_ok_and(|m| self.eval(&m)))
    }

    /// Replace each `#[cfg_attr(pred, a, b)]` with `#[a] #[b]` when `pred`
    /// holds and drop it when it does not — what rustc does before any
    /// attribute macro sees the item. Applied repeatedly, so a
    /// `cfg_attr` producing another `cfg_attr` resolves too.
    pub fn expand_cfg_attr(&self, attrs: &mut Vec<Attribute>) {
        while attrs.iter().any(|a| a.path().is_ident("cfg_attr")) {
            let mut out = Vec::with_capacity(attrs.len());
            for attr in attrs.drain(..) {
                if !attr.path().is_ident("cfg_attr") {
                    out.push(attr);
                    continue;
                }
                let Ok(args) = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) else {
                    continue;
                };
                let mut args = args.into_iter();
                let Some(pred) = args.next() else { continue };
                if !self.eval(&pred) {
                    continue;
                }
                for meta in args {
                    out.push(Attribute {
                        pound_token: attr.pound_token,
                        style: attr.style,
                        bracket_token: attr.bracket_token,
                        meta,
                    });
                }
            }
            *attrs = out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Cfg {
        Cfg::from_print_cfg("unix\ntarget_os=\"macos\"\ntarget_arch=\"aarch64\"\npanic=\"unwind\"\n").with_features(["catalog"])
    }

    fn eval(src: &str) -> bool {
        host().eval(&syn::parse_str::<Meta>(src).unwrap())
    }

    #[test]
    fn predicates_evaluate_like_rustc() {
        assert!(eval("unix"));
        assert!(!eval("windows"));
        assert!(eval("target_os = \"macos\""));
        assert!(!eval("target_arch = \"wasm32\""));
        assert!(eval("not(target_arch = \"wasm32\")"));
        assert!(eval("all(unix, feature = \"catalog\")"));
        assert!(!eval("all(unix, feature = \"server\")"));
        assert!(eval("any(windows, debug_assertions)"));
        assert!(!eval("test"));
        assert!(eval("all()"), "all() of nothing is true");
        assert!(!eval("any()"));
    }

    #[test]
    fn cfg_attr_expands_only_when_its_predicate_holds() {
        let item: syn::ItemStruct = syn::parse_quote! {
            #[cfg_attr(feature = "catalog", derive(IdealystSchema), schema(value_of = "ToneRef"))]
            #[cfg_attr(feature = "server", derive(Serialize))]
            #[cfg_attr(unix, cfg_attr(unix, doc = "nested"))]
            struct Foo;
        };
        let mut attrs = item.attrs;
        host().expand_cfg_attr(&mut attrs);
        let rendered: Vec<String> = attrs.iter().map(|a| quote::quote!(#a).to_string()).collect();
        assert_eq!(rendered, ["# [derive (IdealystSchema)]", "# [schema (value_of = \"ToneRef\")]", "# [doc = \"nested\"]"]);
    }
}
