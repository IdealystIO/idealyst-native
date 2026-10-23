//! `POST /compile` to the fiddle server.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The fiddle server. The editor is served by it, so this is its own
/// origin unless the editor was built with `FIDDLE_SERVER_URL` pointing at
/// another one.
pub const SERVER: &str = match option_env!("FIDDLE_SERVER_URL") {
    Some(url) => url,
    None => "http://127.0.0.1:8787",
};

/// A finished build: the preview URL, or the compiler's report.
pub enum Built {
    /// The bundle's absolute URL, the build time (0 = cached), and any
    /// warnings rustc printed.
    Ok { url: String, ms: u64, warnings: String },
    /// rustc's renderings of every error, in the submitted file names.
    Failed(String),
}

#[derive(Deserialize)]
struct Diagnostic {
    level: String,
    rendered: String,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Reply {
    Built { url: String, ms: u64, diagnostics: Vec<Diagnostic> },
    Failed { diagnostics: Vec<Diagnostic>, log: String },
}

fn rendered(diagnostics: &[Diagnostic], level: &str) -> String {
    diagnostics
        .iter()
        .filter(|d| d.level == level)
        .map(|d| d.rendered.trim_end())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Post the project (submitted path → contents). `Err` is a transport or
/// server failure; a compile error is `Ok(Built::Failed)`.
pub async fn compile(files: BTreeMap<String, String>) -> Result<Built, String> {
    let response = net::Client::new()
        .post(format!("{SERVER}/compile"))
        .json(&serde_json::json!({ "files": files }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or(text));
    }
    match serde_json::from_str::<Reply>(&text).map_err(|e| format!("unreadable reply: {e}"))? {
        Reply::Built { url, ms, diagnostics } => Ok(Built::Ok {
            url: format!("{SERVER}{url}"),
            ms,
            warnings: rendered(&diagnostics, "warning"),
        }),
        Reply::Failed { diagnostics, log } => {
            let errors = rendered(&diagnostics, "error");
            Ok(Built::Failed(if errors.is_empty() { log } else { errors }))
        }
    }
}
