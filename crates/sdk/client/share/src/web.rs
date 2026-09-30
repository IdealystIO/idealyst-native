//! Web share via the **Web Share API** — `navigator.share({ title, text, url })`.
//!
//! `navigator.share` requires a secure context (https / localhost) **and** a
//! transient user activation (it must run inside a click/tap handler), and it
//! isn't implemented in every browser. Where it's missing we return
//! [`ShareError::NotSupported`] rather than silently falling back to, say, a
//! clipboard copy — a fake "share" that didn't open the OS sheet would be worse
//! than an honest "unavailable" the caller can branch on.
//!
//! File sharing (`navigator.share({ files })`) needs `File` objects, which we'd
//! have to materialize from bytes — out of scope here (this crate deals in
//! `PathBuf` references, which have no meaning on the web sandbox). So the web
//! backend shares `title`/`text`/`url`; `files` are ignored on web (documented
//! in the crate `## Scope`).
//!
//! `navigator.share` is two web-glue bindings declared here
//! (own-web-bindings phase 3): a feature probe and the call, which builds
//! the `ShareData` object in JS and hands back the Promise, awaited as a
//! `web_glue::JsFuture`.

use web_glue::{string, JsError, JsFuture, JsValue};

use crate::{ShareContent, ShareError, ShareOutcome};

web_glue::import! {
    // 1 when `navigator.share` is a function (absent in unsupporting
    // browsers and in insecure contexts).
    fn js_can_share() -> u32 =
        "() => typeof window !== 'undefined' && typeof window.navigator.share === 'function' ? 1 : 0";
    // `navigator.share({ title?, text?, url? })` → its Promise. `has` bits:
    // 1 title, 2 text, 4 url; an absent member is left off the object
    // rather than set to "" (an empty `url` is invalid to the spec).
    #[catch]
    fn js_share(has: u32, tp: usize, tl: usize, xp: usize, xl: usize, up: usize, ul: usize) -> u32 =
        "(has, tp, tl, xp, xl, up, ul) => { const d = {}; \
           if (has & 1) d.title = G.str(tp, tl); \
           if (has & 2) d.text = G.str(xp, xl); \
           if (has & 4) d.url = G.str(up, ul); \
           return G.add(window.navigator.share(d)); }";
}

pub(crate) async fn share(content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    // `navigator.share` is absent in unsupporting browsers / insecure contexts.
    if unsafe { js_can_share() } == 0 {
        return Err(ShareError::NotSupported);
    }

    // The ShareData object: { title?, text?, url? }. Web ignores our
    // `files` (PathBuf refs have no web meaning) — documented in `## Scope`.
    let field = |v: &Option<String>, bit: u32| match v {
        Some(s) => (bit, string::abi(s)),
        None => (0, (0, 0)),
    };
    let (t, (tp, tl)) = field(&content.title, 1);
    let (x, (xp, xl)) = field(&content.text, 2);
    let (u, (up, ul)) = field(&content.url, 4);
    let promise = unsafe { js_share(t | x | u, tp, tl, xp, xl, up, ul) }
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        .map(|h| unsafe { JsValue::from_raw(h) })
        .map_err(|e| ShareError::Backend(format!("navigator.share: {e}")))?;

    match JsFuture::new(&promise).await {
        Ok(_) => Ok(ShareOutcome::Completed),
        // The spec rejects with an `AbortError` `DOMException` when the user
        // dismisses the share UI; anything else is a genuine failure.
        Err(e) => {
            if reject_name(&e) == "AbortError" {
                Ok(ShareOutcome::Dismissed)
            } else {
                Err(ShareError::Backend(format!("navigator.share rejected: {e}")))
            }
        }
    }
}

/// The `name` of a rejected `DOMException` (e.g. `"AbortError"`), or empty.
fn reject_name(e: &JsError) -> String {
    e.value()
        .get("name")
        .ok()
        .and_then(|n| n.as_string())
        .unwrap_or_default()
}
