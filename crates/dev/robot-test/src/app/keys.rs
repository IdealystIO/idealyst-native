//! Pressing keys: [`App::press_key`] (to whatever holds focus) and
//! [`Locator::press_key`] (focus the element, then press) — the bridge's
//! `key` verb, which sends the press through the platform's real key path.

use super::*;

/// Split a Playwright key string into its key and modifiers:
/// `"Meta+K"` → (`"K"`, `["Meta"]`), `"Enter"` → (`"Enter"`, `[]`),
/// `"Shift++"` → (`"+"`, `["Shift"]`). The app resolves the names
/// (`ControlOrMeta` included) and refuses unknown ones.
pub fn parse_key(spec: &str) -> (String, Vec<String>) {
    if spec.len() <= 1 {
        return (spec.to_string(), Vec::new());
    }
    let (mods, key) = if let Some(head) = spec.strip_suffix("++") {
        (head, "+")
    } else {
        match spec.rsplit_once('+') {
            Some((head, key)) => (head, key),
            None => return (spec.to_string(), Vec::new()),
        }
    };
    (key.to_string(), mods.split('+').filter(|m| !m.is_empty()).map(str::to_string).collect())
}

fn key_args(spec: &str) -> Value {
    let (key, modifiers) = parse_key(spec);
    json!({ "key": key, "modifiers": modifiers })
}

fn mechanism(v: &Value) -> String {
    v["mechanism"].as_str().unwrap_or("native").to_string()
}

impl App {
    /// Press a key, as a user at the keyboard would, to whatever holds
    /// keyboard focus — the window when nothing does (Playwright's
    /// `page.keyboard.press`). `key` is Playwright's syntax: `"Enter"`,
    /// `"Escape"`, `"Tab"`, `"ArrowDown"`, `"a"`, `"Meta+K"`,
    /// `"ControlOrMeta+K"` (Command on Apple platforms, Control elsewhere),
    /// `"Shift+Tab"`.
    ///
    /// Returns how it was delivered: `"native"` (real platform key events,
    /// through the toolkit) or `"dispatch"` (the framework's own key paths,
    /// where the platform has no key injection — iOS, Linux). An action, so
    /// it is sent once: a busy app may still perform it.
    pub fn press_key(&mut self, key: &str) -> String {
        let desc = format!("press_key({key:?})");
        let args = key_args(key);
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.client.call("key", args.clone()) {
                Ok(v) => return mechanism(&v),
                // The relay had no app: the key never reached one.
                Err(e) if matches!(CallError::of(&e), Some(CallError::NoApp(_))) && Instant::now() < deadline => {
                    std::thread::sleep(POLL)
                }
                Err(e) => panic!("{desc}: {e}"),
            }
        }
    }
}

impl Locator<'_> {
    /// Focus the element, then press `key` (Playwright's `locator.press`):
    /// `app.test_id("password").press_key("Enter")` submits a sign-in form
    /// the way a user's Enter does. Waits for the element as every action
    /// does; a disabled field refuses. Key syntax and the returned mechanism
    /// as for [`App::press_key`].
    pub fn press_key(mut self, key: &str) -> String {
        let desc = format!("{}.press_key({key:?})", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let attempt = self.resolve().and_then(|el| Self::id_of(&el)).and_then(|id| {
                let mut args = key_args(key);
                args["element_id"] = json!(id);
                self.send_action("key", args).map_err(|e| match e {
                    // A key name the app doesn't know won't become one.
                    Failure::NotYet(m) if m.contains("unknown key") || m.contains("unknown modifier") => {
                        Failure::Fatal(m)
                    }
                    other => other,
                })
            });
            match attempt {
                Ok(v) => return mechanism(&v),
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {}", self.app.timeout, e.message())
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_key;

    #[test]
    fn key_strings_split_into_key_and_modifiers() {
        assert_eq!(parse_key("Enter"), ("Enter".into(), vec![]));
        assert_eq!(parse_key("Meta+K"), ("K".into(), vec!["Meta".to_string()]));
        assert_eq!(parse_key("ControlOrMeta+Shift+p"), ("p".into(), vec!["ControlOrMeta".to_string(), "Shift".to_string()]));
        assert_eq!(parse_key("Shift++"), ("+".into(), vec!["Shift".to_string()]));
        assert_eq!(parse_key("+"), ("+".into(), vec![]));
        assert_eq!(parse_key("a"), ("a".into(), vec![]));
    }
}
