//! `ota::config!()`: the calling crate's over-the-air settings, from its
//! `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.idealyst.ota]
//! url = "https://ota.example.com/my-app"   # where the app reads releases
//! bucket = "s3://my-app-ota/my-app"        # where `idealyst ota publish` writes (not compiled in)
//! public_keys = ["c5dcf42a…"]              # written by `idealyst ota init`
//! ```
//!
//! `IDEALYST_OTA_URL`, set when the app is compiled, replaces `url` (a
//! staging build). The manifest is read at compile time and tracked, so
//! editing it rebuilds the app.

use proc_macro::TokenStream;
use quote::quote;

#[proc_macro]
pub fn config(input: TokenStream) -> TokenStream {
    if !input.is_empty() {
        return error("`ota::config!()` takes no arguments: the settings live in Cargo.toml");
    }
    match expand() {
        Ok(t) => t.into(),
        Err(msg) => error(&msg),
    }
}

fn error(msg: &str) -> TokenStream {
    quote!(::core::compile_error!(#msg)).into()
}

const HELP: &str = "add to the app's Cargo.toml:\n\n  [package.metadata.idealyst.ota]\n  url = \"https://ota.example.com/my-app\"\n\n(`idealyst ota init` writes it)";

/// The settings in a `Cargo.toml`: the package name, `url`, `public_keys`.
struct Settings {
    name: String,
    url: Option<String>,
    keys: Vec<String>,
}

fn settings(text: &str) -> Result<Settings, String> {
    let manifest: toml::Value = text.parse().map_err(|e| format!("ota::config!: Cargo.toml: {e}"))?;
    let package = manifest.get("package").ok_or("ota::config!: Cargo.toml has no [package]")?;
    let name = package.get("name").and_then(|n| n.as_str()).ok_or("ota::config!: [package] has no name")?;
    let ota = package
        .get("metadata")
        .and_then(|m| m.get("idealyst"))
        .and_then(|m| m.get("ota"))
        .ok_or_else(|| format!("ota::config!: no over-the-air settings — {HELP}"))?;
    let url = ota.get("url").and_then(|u| u.as_str()).map(str::to_string);
    let keys: Vec<String> = match ota.get("public_keys") {
        None => Vec::new(),
        Some(toml::Value::Array(a)) => a
            .iter()
            .map(|k| k.as_str().map(str::to_string).ok_or("ota::config!: `public_keys` holds strings (hex)".to_string()))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("ota::config!: `public_keys` is a list of hex strings".into()),
    };
    for k in &keys {
        if k.len() != 64 || !k.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("ota::config!: public key `{k}` isn't 64 hex characters"));
        }
    }
    Ok(Settings { name: name.to_string(), url, keys })
}

fn expand() -> Result<proc_macro2::TokenStream, String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| "ota::config!: CARGO_MANIFEST_DIR is not set (build with cargo)".to_string())?;
    let path = std::path::Path::new(&dir).join("Cargo.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("ota::config!: read {}: {e}", path.display()))?;
    let Settings { name, url, keys } = settings(&text)?;
    let missing_url = format!("ota::config!: no `url` — set IDEALYST_OTA_URL, or {HELP}");
    let url = match &url {
        Some(u) => quote!(#u),
        None => quote!(::core::panic!(#missing_url)),
    };
    let tracked = path.to_string_lossy().into_owned();
    Ok(quote! {{
        // Rebuild when the manifest changes.
        const _: &[u8] = ::core::include_bytes!(#tracked);
        const __URL: &str = match ::core::option_env!("IDEALYST_OTA_URL") {
            ::core::option::Option::Some(u) => u,
            ::core::option::Option::None => #url,
        };
        ::ota::Config { url: __URL, public_keys: &[#(#keys),*], app: #name }
    }})
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "c5dcf42a15f9549735d43964d671e041632796d35e19398c6163f06fe5a3d99f";

    #[test]
    fn reads_the_settings() {
        let s = settings(&format!(
            "[package]\nname = \"shop\"\n[package.metadata.idealyst.ota]\nurl = \"https://ota.example.com\"\npublic_keys = [\"{KEY}\"]\n"
        ))
        .unwrap();
        assert_eq!((s.name.as_str(), s.url.as_deref(), s.keys.as_slice()), ("shop", Some("https://ota.example.com"), &[KEY.to_string()][..]));
    }

    #[test]
    fn says_what_is_wrong() {
        let err = |t: &str| settings(t).err().unwrap();
        assert!(err("[package]\nname = \"shop\"\n").contains("no over-the-air settings"));
        assert!(err("[package]\nname = \"shop\"\n[package.metadata.idealyst.ota]\npublic_keys = [\"ab\"]\n").contains("isn't 64 hex"));
        // No url is allowed here: IDEALYST_OTA_URL may supply it, checked at compile time.
        assert_eq!(settings("[package]\nname = \"shop\"\n[package.metadata.idealyst.ota]\n").unwrap().url, None);
    }
}
