//! Opt-in locale packs: the strings for `lazy` locales, which are *not*
//! compiled into the binary. A pack is a flat `{ "message": "template" }`
//! map keyed by message name, installed either directly (SSR-inlined) or
//! by a loader fetching it on demand.

use runtime_core::Signal;
use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

thread_local! {
    /// code -> (message name -> template). One entry per installed pack.
    static PACKS: RefCell<HashMap<String, HashMap<String, String>>> =
        RefCell::new(HashMap::new());

    /// Codes whose loader has been kicked off and not yet resolved — so a
    /// burst of `ensure_pack_loaded` calls fetches once, not N times.
    static IN_FLIGHT: RefCell<HashSet<String>> = RefCell::new(HashSet::new());

    /// App-installed loader. Invoked with a locale code; responsible for
    /// (eventually) calling `install_pack`. Unopinionated about *how* —
    /// sync, async, network, embedded — that's the app's choice.
    static LOADER: RefCell<Option<Rc<dyn Fn(&str)>>> = const { RefCell::new(None) };

    /// Monotonic counter bumped on every pack install. Reactive scopes that
    /// read it (via `current_locale_code`) recompute when a pack arrives.
    static PACK_EPOCH: OnceCell<Signal<u64>> = const { OnceCell::new() };
}

fn epoch_signal() -> Signal<u64> {
    // Thread-lifetime global — `unscope` so a transient first-access scope
    // doesn't own (and later recycle) its arena slot. Same contract as the
    // locale + viewport signals.
    PACK_EPOCH.with(|cell| *cell.get_or_init(|| runtime_core::unscope(|| Signal::new(0u64))))
}

/// Subscribe the current reactive scope to pack installs. Called from
/// `current_locale_code` so message derives recompute on pack arrival.
pub(crate) fn subscribe_epoch() {
    let _ = epoch_signal().get();
}

fn bump_epoch() {
    let sig = epoch_signal();
    sig.set(sig.get().wrapping_add(1));
}

pub(crate) fn clear_in_flight(code: &str) {
    IN_FLIGHT.with(|f| {
        f.borrow_mut().remove(code);
    });
}

/// Install (or replace) the pack for `code` and notify reactive readers.
/// Clears any in-flight marker for that code.
pub fn install_pack(code: &str, entries: HashMap<String, String>) {
    PACKS.with(|p| {
        p.borrow_mut().insert(code.to_string(), entries);
    });
    clear_in_flight(code);
    bump_epoch();
}

/// Install a pack from a flat JSON object string (`{ "key": "value", … }`).
/// Convenience for SSR-inlined packs and fetched bodies. Returns the JSON
/// parse error if the body isn't a flat string map.
pub fn install_pack_json(code: &str, json: &str) -> Result<(), serde_json::Error> {
    let entries: HashMap<String, String> = serde_json::from_str(json)?;
    install_pack(code, entries);
    Ok(())
}

/// Whether a pack for `code` is currently installed.
pub fn has_pack(code: &str) -> bool {
    PACKS.with(|p| p.borrow().contains_key(code))
}

/// Look up a message template in an opt-in pack. `None` if the pack isn't
/// installed yet or doesn't contain the message — callers fall back to the
/// default-locale string.
pub fn opt_in_template(message: &str, code: &str) -> Option<String> {
    PACKS.with(|p| p.borrow().get(code).and_then(|m| m.get(message).cloned()))
}

/// Install the loader used to fetch opt-in packs on demand. The loader is
/// handed a locale code and must eventually call [`install_pack`] (or
/// [`install_pack_json`]). With the `lazy-fetch` feature, install
/// [`net_pack_loader`] for a ready-made network loader.
pub fn set_pack_loader<F>(loader: F)
where
    F: Fn(&str) + 'static,
{
    LOADER.with(|l| *l.borrow_mut() = Some(Rc::new(loader)));
}

/// Ensure the pack for `code` is (being) loaded: no-op if already present
/// or already in flight; otherwise marks it in-flight and invokes the
/// installed loader. Without a loader this is a no-op (the locale keeps
/// falling back to default until a pack is installed by other means).
pub fn ensure_pack_loaded(code: &str) {
    if has_pack(code) {
        return;
    }
    // `insert` returns false if the code was already present → already in flight.
    let newly_marked = IN_FLIGHT.with(|f| f.borrow_mut().insert(code.to_string()));
    if !newly_marked {
        return;
    }
    match LOADER.with(|l| l.borrow().clone()) {
        Some(loader) => loader(code),
        None => clear_in_flight(code),
    }
}

/// Ready-made network loader for opt-in packs: fetches `<base>/<code>.json`
/// via the `net` SDK and installs it. Drive on the framework async driver.
///
/// ```ignore
/// i18n::set_pack_loader(i18n::net_pack_loader("/locales"));
/// ```
#[cfg(feature = "lazy-fetch")]
pub fn net_pack_loader(base_url: impl Into<String>) -> impl Fn(&str) + 'static {
    use runtime_core::logging::{log, LogLevel};
    let base = base_url.into();
    move |code: &str| {
        let url = format!("{}/{}.json", base.trim_end_matches('/'), code);
        let code = code.to_string();
        runtime_core::driver::spawn_async(async move {
            let client = net::Client::new();
            match client.get(&url).send().await {
                Ok(resp) => match resp.json::<HashMap<String, String>>().await {
                    Ok(map) => install_pack(&code, map),
                    Err(e) => {
                        log(
                            LogLevel::Error,
                            &format!("i18n: failed to parse locale pack `{code}`: {e}"),
                        );
                        clear_in_flight(&code);
                    }
                },
                Err(e) => {
                    log(
                        LogLevel::Error,
                        &format!("i18n: failed to fetch locale pack `{code}` from {url}: {e}"),
                    );
                    clear_in_flight(&code);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_lookup() {
        let mut map = HashMap::new();
        map.insert("greeting".to_string(), "やあ、{name}".to_string());
        install_pack("ja", map);
        assert!(has_pack("ja"));
        assert_eq!(opt_in_template("greeting", "ja").as_deref(), Some("やあ、{name}"));
        assert_eq!(opt_in_template("missing", "ja"), None);
        assert_eq!(opt_in_template("greeting", "de"), None);
    }

    #[test]
    fn install_pack_json_parses_flat_map() {
        install_pack_json("xx", r#"{"a":"1","b":"2"}"#).unwrap();
        assert_eq!(opt_in_template("a", "xx").as_deref(), Some("1"));
        assert!(install_pack_json("yy", r#"{"a": 3}"#).is_err());
    }

    #[test]
    fn loader_invoked_once_until_resolved() {
        let calls = Rc::new(RefCell::new(0u32));
        let c = calls.clone();
        set_pack_loader(move |_code| {
            *c.borrow_mut() += 1;
        });
        // Use a code with no pack installed yet.
        ensure_pack_loaded("zz");
        ensure_pack_loaded("zz"); // in-flight: no second call
        assert_eq!(*calls.borrow(), 1);
        // Resolving the load clears in-flight; a later request can refetch.
        install_pack("zz", HashMap::new());
        // Already has a pack now → no further loader calls.
        ensure_pack_loaded("zz");
        assert_eq!(*calls.borrow(), 1);
    }
}
