//! Response cookies for server-fn handlers — the server half of the web
//! BFF auth pattern.
//!
//! A handler calls [`set_cookie`] to attach a `Set-Cookie` to the HTTP
//! response. The canonical use is an **httpOnly** session cookie: a
//! `login` server fn validates the user and sets a cookie the browser
//! sends automatically on later calls, but that JS can never read — so the
//! session secret never enters the client. This is what makes secure web
//! auth possible without a (false-pretense) client-side secret store; see
//! the `credentials` SDK.
//!
//! Mechanism: the dispatcher seeds each request's [`Context`] with a
//! [`CookieJar`] before running the handler, then drains it into
//! `Set-Cookie` response headers afterward. `set_cookie` pushes into the
//! current request's jar via the task-local context — so it works from
//! anywhere in a handler's call tree, no threading required.
//!
//! [`Context`]: crate::extract::Context

use std::sync::{Arc, Mutex};

use crate::extractors::CURRENT_CONTEXT;

/// The `SameSite` attribute. `Lax` is the safe default for session
/// cookies; `Strict` for maximum CSRF resistance; `None` (which *requires*
/// `Secure`) only for genuine cross-site needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl SameSite {
    fn as_str(self) -> &'static str {
        match self {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }
}

/// A response cookie. Defaults are the safe ones for a session secret:
/// `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/`. Override with the
/// builder methods.
#[derive(Clone, Debug)]
pub struct Cookie {
    name: String,
    value: String,
    http_only: bool,
    secure: bool,
    same_site: Option<SameSite>,
    path: Option<String>,
    domain: Option<String>,
    /// Lifetime in seconds. `None` = a session cookie (cleared when the
    /// browser closes). `Some(0)` expires it immediately (delete).
    max_age: Option<i64>,
    /// Emit `Expires=` at the Unix epoch. Set only by [`Cookie::clearing`]:
    /// `Max-Age=0` is what current browsers act on, the past `Expires` is
    /// for user agents that predate `Max-Age` (RFC 6265 §5.3 lets
    /// `Max-Age` win when both are present, so they never disagree).
    expired: bool,
}

impl Cookie {
    /// A cookie with secure session defaults: `HttpOnly`, `Secure`,
    /// `SameSite=Lax`, `Path=/`, session lifetime.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            http_only: true,
            secure: true,
            same_site: Some(SameSite::Lax),
            path: Some("/".to_string()),
            domain: None,
            max_age: None,
            expired: false,
        }
    }

    /// Set `HttpOnly` (default `true`). Keep this on for auth cookies — it
    /// stops JS (and any XSS) from reading the value.
    pub fn http_only(mut self, yes: bool) -> Self {
        self.http_only = yes;
        self
    }

    /// Set `Secure` (default `true`). Required for `SameSite=None`; means
    /// the cookie is only sent over HTTPS.
    pub fn secure(mut self, yes: bool) -> Self {
        self.secure = yes;
        self
    }

    /// Set the `SameSite` attribute.
    pub fn same_site(mut self, value: SameSite) -> Self {
        self.same_site = Some(value);
        self
    }

    /// Set the `Path` (default `/`).
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set the `Domain`.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Set `Max-Age` in seconds. `0` expires the cookie immediately.
    pub fn max_age_secs(mut self, secs: i64) -> Self {
        self.max_age = Some(secs);
        self
    }

    /// The `Set-Cookie` that **deletes** this cookie: same name, `Path`,
    /// `Domain`, `Secure`, `HttpOnly` and `SameSite`, an empty value,
    /// `Max-Age=0` and an `Expires` in the past.
    ///
    /// Derive the deletion from the SAME definition that set the cookie —
    /// build the cookie in one place and call `.clearing()` on it at
    /// logout. A browser only replaces (and so only deletes) a stored
    /// cookie whose name, path and domain all match, so a deletion with a
    /// different `Path`/`Domain` leaves the original in place; and a
    /// `Secure` deletion for a cookie that was set without `Secure` over
    /// plain `http://` is rejected outright. [`clear_cookie`] can't know
    /// any of that — it assumes [`Cookie::new`]'s defaults.
    ///
    /// ```ignore
    /// fn session_cookie(value: impl Into<String>) -> Cookie {
    ///     Cookie::new("session", value).path("/app").secure(false)
    /// }
    /// server::set_cookie(session_cookie(token));             // login
    /// server::set_cookie(session_cookie("").clearing());     // logout
    /// ```
    #[must_use = "a clearing cookie does nothing until passed to `server::set_cookie`"]
    pub fn clearing(&self) -> Cookie {
        Cookie {
            value: String::new(),
            max_age: Some(0),
            expired: true,
            ..self.clone()
        }
    }

    /// Render the `Set-Cookie` header value.
    fn to_header(&self) -> String {
        // Values may contain characters needing care, but cookie values
        // are author-controlled session ids/tokens here; we emit as-is and
        // rely on the caller using a URL-safe token. (No general
        // percent-encoding to keep round-trips with the client exact.)
        let mut s = format!("{}={}", self.name, self.value);
        if let Some(path) = &self.path {
            s.push_str("; Path=");
            s.push_str(path);
        }
        if let Some(domain) = &self.domain {
            s.push_str("; Domain=");
            s.push_str(domain);
        }
        if let Some(max_age) = self.max_age {
            s.push_str("; Max-Age=");
            s.push_str(&max_age.to_string());
        }
        if self.expired {
            s.push_str("; Expires=Thu, 01 Jan 1970 00:00:00 GMT");
        }
        if let Some(same_site) = self.same_site {
            s.push_str("; SameSite=");
            s.push_str(same_site.as_str());
        }
        if self.secure {
            s.push_str("; Secure");
        }
        if self.http_only {
            s.push_str("; HttpOnly");
        }
        s
    }
}

/// The `Set-Cookie` value, exactly as the dispatcher emits it — e.g.
/// `session=abc; Path=/; SameSite=Lax; Secure; HttpOnly`.
impl std::fmt::Display for Cookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_header())
    }
}

/// Build a cookie that **deletes** `name`, assuming it was set with
/// [`Cookie::new`]'s defaults — `Path=/`, no `Domain`, `Secure`,
/// `HttpOnly`, `SameSite=Lax`. Shorthand for
/// `Cookie::new(name, "").clearing()`.
///
/// For a cookie set with ANY other `Path`, `Domain` or `Secure`, this does
/// not delete it — the browser keeps the original (see
/// [`Cookie::clearing`] for why). Call `.clearing()` on the same `Cookie`
/// definition that set it instead.
#[must_use = "`clear_cookie` only builds the deletion — pass it to `server::set_cookie`"]
pub fn clear_cookie(name: impl Into<String>) -> Cookie {
    Cookie::new(name, "").clearing()
}

/// Attach `cookie` to the current request's HTTP response as a
/// `Set-Cookie` header. A no-op (logs nothing) if called outside a
/// server-fn handler context.
pub fn set_cookie(cookie: Cookie) {
    let header = cookie.to_header();
    let _ = CURRENT_CONTEXT.try_with(|c| {
        if let Some(jar) = c.get::<CookieJar>() {
            jar.push(header);
        }
    });
}

/// Per-request accumulator of `Set-Cookie` header values. Seeded into the
/// [`Context`](crate::extract::Context)'s extension map by the dispatcher;
/// drained by it after the handler runs. `Clone` shares the inner buffer
/// (so the dispatcher's retained handle sees what the handler pushed).
#[derive(Clone, Default)]
pub(crate) struct CookieJar(Arc<Mutex<Vec<String>>>);

impl CookieJar {
    pub(crate) fn push(&self, header: String) {
        self.0.lock().unwrap().push(header);
    }

    /// Take the accumulated headers, leaving the jar empty.
    pub(crate) fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_has_secure_session_defaults() {
        let h = Cookie::new("session", "abc").to_header();
        assert!(h.starts_with("session=abc"));
        assert!(h.contains("; Path=/"));
        assert!(h.contains("; SameSite=Lax"));
        assert!(h.contains("; Secure"));
        assert!(h.contains("; HttpOnly"));
    }

    #[test]
    fn clear_cookie_expires() {
        let h = clear_cookie("session").to_header();
        assert!(h.starts_with("session=;"), "{h}");
        assert!(h.contains("; Max-Age=0"));
        assert!(h.contains("; Expires=Thu, 01 Jan 1970 00:00:00 GMT"), "{h}");
    }

    /// The attributes that decide whether a deletion lands, in the order
    /// the header renders them.
    fn attrs(h: &str) -> Vec<&str> {
        h.split("; ")
            .skip(1)
            .filter(|a| !a.starts_with("Max-Age=") && !a.starts_with("Expires="))
            .collect()
    }

    /// The reported bug (#12): the only deletion helper was
    /// `clear_cookie(name)`, always `Path=/`, `Secure`, `HttpOnly`,
    /// `SameSite=Lax`. A browser drops a `Secure` deletion for a cookie
    /// set without `Secure` over `http://`, and a deletion whose
    /// `Path`/`Domain` differ names a DIFFERENT cookie — so logout left the
    /// real session cookie in place. The clearing form must mirror every
    /// attribute of the cookie it deletes.
    #[test]
    fn regression_clearing_cookie_mirrors_the_cookie_it_clears() {
        let set = Cookie::new("sid", "tok")
            .secure(false)
            .http_only(false)
            .same_site(SameSite::Strict)
            .path("/app")
            .domain("example.com")
            .max_age_secs(3600);
        let clear = set.clearing().to_header();
        assert!(clear.starts_with("sid=;"), "empty value: {clear}");
        assert_eq!(attrs(&clear), attrs(&set.to_header()), "{clear}");
        assert!(!clear.contains("Secure"), "{clear}");
        assert!(clear.contains("; Path=/app"), "{clear}");
        assert!(clear.contains("; Domain=example.com"), "{clear}");
        assert!(clear.contains("; Max-Age=0"), "{clear}");
        assert!(clear.contains("; Expires=Thu, 01 Jan 1970 00:00:00 GMT"), "{clear}");
        assert!(!clear.contains("Max-Age=3600"), "{clear}");
    }

    /// `clear_cookie(name)` is exactly the clearing form of a default
    /// cookie — its documented assumption.
    #[test]
    fn clear_cookie_is_the_clearing_form_of_the_defaults() {
        assert_eq!(
            clear_cookie("session").to_header(),
            Cookie::new("session", "abc").clearing().to_header()
        );
    }

    #[test]
    fn display_is_the_set_cookie_value() {
        let c = Cookie::new("k", "v").path("/p");
        assert_eq!(c.to_string(), c.to_header());
    }

    #[test]
    fn opt_out_of_http_only_and_secure() {
        let h = Cookie::new("k", "v")
            .http_only(false)
            .secure(false)
            .same_site(SameSite::Strict)
            .to_header();
        assert!(!h.contains("HttpOnly"));
        assert!(!h.contains("Secure"));
        assert!(h.contains("; SameSite=Strict"));
    }

    /// The jar shares its buffer across clones — the property the
    /// dispatcher relies on (it keeps one clone, the handler pushes via
    /// another).
    #[test]
    fn jar_clone_shares_buffer() {
        let jar = CookieJar::default();
        let handle = jar.clone();
        jar.push("a=1".into());
        handle.push("b=2".into());
        let drained = handle.take();
        assert_eq!(drained, vec!["a=1".to_string(), "b=2".to_string()]);
        assert!(jar.take().is_empty(), "take leaves the jar empty");
    }
}
