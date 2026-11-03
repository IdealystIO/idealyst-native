//! Controls the relay serves itself, around the app rather than inside it:
//! `load_url` boots a fresh app instance at a URL, and `set_cookie` /
//! `get_cookies` / `clear_cookies` reach the cookie jar a page cannot
//! (an `HttpOnly` cookie is invisible to `document.cookie`).
//!
//! For a web app the relay drives its tab over CDP ([`cdp::with_page`]),
//! found by the page's pairing nonce like `screenshot`:
//!
//! - `load_url {url}` — `Page.navigate` to `url` resolved against the
//!   page's current address (so `/share/abc` keeps the dev server's
//!   origin), then waits for the NEW page's app to dial the relay and say
//!   `hello`. The old page's connection is gone by then; the next request
//!   reaches the new app. A URL that differs only in its `#fragment` would
//!   be a same-document navigation that boots nothing, so the page is
//!   reloaded instead.
//! - `set_cookie {name, value, url?, domain?, path?, http_only?, secure?,
//!   same_site?, expires?}` — `Network.setCookie`; with neither `url` nor
//!   `domain`, the cookie belongs to the page's origin. `expires` is
//!   seconds since the Unix epoch; absent, a session cookie.
//! - `get_cookies` — the cookies the page's URL sends (`Network.getCookies`),
//!   `HttpOnly` ones included.
//! - `clear_cookies {name?}` — every cookie in the browser
//!   (`Network.clearBrowserCookies`), or only `name` on the page's origin.
//!
//! A native app has no tab. `load_url` relaunches it through the dev
//! session's [`Relaunch`] hook when the session installed one (it is the
//! process that launched the app); the relay then waits for the new
//! process to dial in, as for a page. Everywhere else, and for cookies (a
//! native HTTP stack keeps its own store, out of the relay's reach), the
//! verb fails saying so.

use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{cdp, Inner};

/// The verbs this module serves (see the module docs).
pub const VERBS: &[&str] = &["load_url", "set_cookie", "get_cookies", "clear_cookies"];

/// How long `load_url` waits for the new instance when the request names
/// no `timeout_ms`: a page fetching and starting its wasm, or a cold app
/// start.
pub(crate) const LOAD_URL_WAIT: Duration = Duration::from_secs(60);

/// Relaunch a native app at a deep link: the dev session's hook, given the
/// platform the driven app reported (`android`, `ios`, …) and the link to
/// launch with. Returns once the relaunch is under way (the old process
/// gone); the relay waits for the new one to dial in. `Err` for a platform
/// the session cannot relaunch.
#[derive(Clone)]
pub struct Relaunch(pub Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>);

impl std::fmt::Debug for Relaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Relaunch(..)")
    }
}

/// The deep link a native relaunch is given for `url`: an absolute URL
/// as is; an app path (`/share/abc`) as `https://localhost/share/abc`,
/// whose route path (`runtime_shared::inbound_link::route_path`) is that
/// path — the screen the same `load_url` opens on web.
pub fn native_link(url: &str) -> String {
    if url.starts_with('/') {
        format!("https://localhost{url}")
    } else {
        url.to_string()
    }
}

/// Serve one of [`VERBS`]. The `Ok` value is the whole response body
/// (`{"ok": …}`), as `forward` returns it.
pub(crate) fn serve(inner: &Arc<Inner>, cmd: &str, args: &Value, wait: Duration) -> Result<Value, String> {
    if inner.app_outbound.lock().unwrap().is_none() {
        return Err("no app connected to the relay".into());
    }
    let session = inner.app_session.load(Ordering::SeqCst);
    let nonce = inner.nonces.lock().unwrap().get(&session).cloned();
    let Some(nonce) = nonce else {
        return native(inner, session, cmd, args, wait);
    };
    let dir = inner
        .cdp_profiles_dir
        .as_ref()
        .ok_or_else(|| format!("{cmd}: this relay was started without CDP access"))?;
    let ports = cdp::browser_ports(dir);
    let on_page = |f: &mut dyn FnMut(&mut cdp::Page) -> Result<Value, String>| {
        cdp::with_page(&ports, &nonce, |p| f(p)).map_err(|e| {
            format!(
                "{cmd}: {e}. The relay reaches only a tab in a browser the dev session launched \
                 (`idealyst test --web`, or `idealyst dev --headless-client`)"
            )
        })
    };
    let ok = match cmd {
        "load_url" => {
            let url = str_arg(args, "url", cmd)?;
            let loaded = on_page(&mut |page| navigate(page, url))?;
            wait_for_new_app(inner, session, wait)?;
            json!({ "url": loaded })
        }
        "set_cookie" => on_page(&mut |page| set_cookie(page, args))?,
        "get_cookies" => on_page(&mut get_cookies)?,
        "clear_cookies" => on_page(&mut |page| clear_cookies(page, args))?,
        other => return Err(format!("unknown browser control {other}")),
    };
    Ok(json!({ "ok": ok }))
}

/// A native app: `load_url` through the dev session's relaunch hook.
fn native(inner: &Arc<Inner>, session: u64, cmd: &str, args: &Value, wait: Duration) -> Result<Value, String> {
    let platform = inner
        .apps
        .lock()
        .unwrap()
        .iter()
        .find(|(a, _)| a.id == session)
        .and_then(|(a, _)| a.platform.clone())
        .unwrap_or_else(|| "native".into());
    if cmd != "load_url" {
        return Err(format!(
            "{cmd} is unsupported on {platform}: cookies are set in a web page's browser; a native \
             app's HTTP stack keeps its own cookie store, which the robot does not reach"
        ));
    }
    let url = str_arg(args, "url", cmd)?;
    let Some(Relaunch(relaunch)) = &inner.relaunch else {
        return Err(format!(
            "load_url is unsupported on {platform}: booting a native app at a URL relaunches it with \
             a deep link, which this dev session cannot do for {platform} (it can for web and Android); \
             `open_link` delivers a link to the running app"
        ));
    };
    let link = native_link(url);
    relaunch(&platform, &link).map_err(|e| format!("load_url on {platform}: {e}"))?;
    wait_for_new_app(inner, session, wait)?;
    Ok(json!({ "ok": { "url": link } }))
}

fn str_arg<'a>(args: &'a Value, key: &str, cmd: &str) -> Result<&'a str, String> {
    args.get(key).and_then(|v| v.as_str()).ok_or_else(|| format!("{cmd} needs a string '{key}'"))
}

/// Wait until an app session newer than `after` has connected and said
/// `hello` — the instance a navigation or relaunch booted.
fn wait_for_new_app(inner: &Arc<Inner>, after: u64, wait: Duration) -> Result<(), String> {
    let deadline = Instant::now() + wait;
    loop {
        if inner.apps.lock().unwrap().iter().any(|(a, _)| a.id > after && a.platform.is_some()) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "load_url: no new app instance connected to the relay within {} ms (the load is under \
                 way; a cold build or slow boot needs a longer timeout)",
                wait.as_millis()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `url` resolved against the page's address, then navigated to.
fn navigate(page: &mut cdp::Page, url: &str) -> Result<Value, String> {
    let quoted = serde_json::to_string(url).expect("a string serializes");
    let target = page.evaluate(&format!("new URL({quoted}, location.href).href"))?;
    let target = target.as_str().ok_or_else(|| format!("load_url: {url:?} is not a URL"))?.to_string();
    let r = page.call("Page.navigate", json!({ "url": target }))?;
    if let Some(e) = r.get("errorText").and_then(|e| e.as_str()) {
        return Err(format!("load_url {target}: {e}"));
    }
    // No `loaderId`: a same-document (fragment) navigation, which boots no
    // new app. Reload so it does.
    if r.get("loaderId").is_none() {
        page.call("Page.reload", json!({}))?;
    }
    Ok(json!(target))
}

/// The CDP `Network.setCookie` parameters for a `set_cookie` request; the
/// page's origin fills in for a missing `url` and `domain`.
pub fn cookie_params(args: &Value, origin: impl FnOnce() -> Result<String, String>) -> Result<Value, String> {
    let name = str_arg(args, "name", "set_cookie")?;
    let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("");
    let mut p = json!({ "name": name, "value": value });
    for (key, cdp_key) in [("url", "url"), ("domain", "domain"), ("path", "path"), ("same_site", "sameSite")] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            p[cdp_key] = json!(v);
        }
    }
    for (key, cdp_key) in [("http_only", "httpOnly"), ("secure", "secure")] {
        if let Some(v) = args.get(key).and_then(|v| v.as_bool()) {
            p[cdp_key] = json!(v);
        }
    }
    if let Some(e) = args.get("expires").and_then(|v| v.as_f64()) {
        p["expires"] = json!(e);
    }
    if p.get("url").is_none() && p.get("domain").is_none() {
        p["url"] = json!(origin()?);
    }
    Ok(p)
}

fn origin(page: &mut cdp::Page) -> Result<String, String> {
    page.evaluate("location.origin")?.as_str().map(str::to_string).ok_or_else(|| "the page has no origin".into())
}

fn set_cookie(page: &mut cdp::Page, args: &Value) -> Result<Value, String> {
    let params = cookie_params(args, || origin(page))?;
    let r = page.call("Network.setCookie", params.clone())?;
    if r.get("success") == Some(&json!(false)) {
        return Err(format!("set_cookie: the browser refused {params}"));
    }
    Ok(json!({ "name": params["name"] }))
}

fn get_cookies(page: &mut cdp::Page) -> Result<Value, String> {
    let href = page.evaluate("location.href")?;
    let r = page.call("Network.getCookies", json!({ "urls": [href] }))?;
    let cookies: Vec<Value> = r["cookies"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            json!({
                "name": c["name"], "value": c["value"], "domain": c["domain"], "path": c["path"],
                "http_only": c["httpOnly"], "secure": c["secure"], "same_site": c["sameSite"],
                "expires": c["expires"],
            })
        })
        .collect();
    Ok(json!(cookies))
}

fn clear_cookies(page: &mut cdp::Page, args: &Value) -> Result<Value, String> {
    match args.get("name").and_then(|v| v.as_str()) {
        Some(name) => {
            let url = origin(page)?;
            page.call("Network.deleteCookies", json!({ "name": name, "url": url }))?;
        }
        None => {
            page.call("Network.clearBrowserCookies", json!({}))?;
        }
    }
    Ok(json!("ok"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_path_relaunches_as_a_link_whose_route_is_that_path() {
        assert_eq!(native_link("/share/abc?x=1"), "https://localhost/share/abc?x=1");
        assert_eq!(native_link("myapp://c/tok"), "myapp://c/tok");
        assert_eq!(native_link("https://crewforge.app/fill/t"), "https://crewforge.app/fill/t");
    }

    #[test]
    fn a_cookie_defaults_to_the_page_origin_and_maps_its_flags() {
        let p = cookie_params(
            &json!({ "name": "device", "value": "k1", "http_only": true, "secure": false, "same_site": "Lax", "expires": 2.0e9 }),
            || Ok("http://127.0.0.1:4000".into()),
        )
        .unwrap();
        assert_eq!(
            p,
            json!({ "name": "device", "value": "k1", "url": "http://127.0.0.1:4000", "httpOnly": true,
                    "secure": false, "sameSite": "Lax", "expires": 2.0e9 })
        );
        // An explicit domain keeps the origin out of it.
        let p = cookie_params(&json!({ "name": "a", "domain": "example.com", "path": "/x" }), || {
            panic!("not asked for")
        })
        .unwrap();
        assert_eq!(p, json!({ "name": "a", "value": "", "domain": "example.com", "path": "/x" }));
        assert!(cookie_params(&json!({ "value": "v" }), || Ok(String::new())).unwrap_err().contains("'name'"));
    }
}
