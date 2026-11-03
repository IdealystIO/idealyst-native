//! Keys the robot presses: the request ([`KeyChord`], parsed from a
//! Playwright-style string like `"Enter"` or `"ControlOrMeta+K"`) and what
//! a backend receives ([`KeyPress`], modifiers resolved for the platform).
//!
//! Key names follow Playwright's vocabulary, which is Web
//! `KeyboardEvent.key` for named keys plus a few aliases: `Enter`,
//! `Escape`, `Tab`, `Backspace`, `Delete`, `Insert`, `Home`, `End`,
//! `PageUp`, `PageDown`, `ArrowUp`/`Down`/`Left`/`Right`, `Space`, `F1`…`F24`,
//! the modifier keys themselves (`Shift`, `Control`, `Alt`, `Meta`), the
//! physical names `KeyA`…`KeyZ` / `Digit0`…`Digit9`, and any single
//! character. Every resolved press carries the `key` and `code` a real US
//! keyboard reports, so a backend can build the platform event without its
//! own name table, and app code matching `e.key` / `e.code` sees what a
//! user's press produces.

/// A key press as a test asks for it, before the platform is known:
/// [`control_or_meta`](Self::control_or_meta) is still unresolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyChord {
    /// Web `KeyboardEvent.key` the press produces (`"Enter"`, `"a"`, `"K"`,
    /// `" "`).
    pub key: String,
    /// Web `KeyboardEvent.code` on a US layout (`"Enter"`, `"KeyA"`,
    /// `"Space"`); empty for a character no US key types.
    pub code: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
    /// `ControlOrMeta` was asked for: Meta where the platform's shortcut
    /// modifier is Command (Apple), Control elsewhere.
    pub control_or_meta: bool,
}

/// A key press resolved for the platform — what
/// [`IntrospectionOps::native_key`](super::IntrospectionOps::native_key)
/// delivers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPress {
    /// Web `KeyboardEvent.key` (see [`KeyChord::key`]).
    pub key: String,
    /// Web `KeyboardEvent.code` on a US layout; empty when no US key types
    /// [`key`](Self::key).
    pub code: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl KeyPress {
    /// The character the press types into a focused text field, as the
    /// platform's text path delivers it: a single-character key with no
    /// Control or Meta held. `None` for named keys (Enter, Tab, arrows…)
    /// and shortcuts.
    pub fn text(&self) -> Option<char> {
        if self.ctrl || self.meta {
            return None;
        }
        let mut chars = self.key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    }
}

/// Named keys a test may press, by their Web `KeyboardEvent.key` (which is
/// also their `code`).
const NAMED: &[&str] = &[
    "Enter", "Escape", "Tab", "Backspace", "Delete", "Insert", "Home", "End", "PageUp", "PageDown",
    "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "CapsLock", "ContextMenu",
];

impl KeyChord {
    /// Parse a Playwright key string: modifiers then the key, joined by
    /// `+` (`"Enter"`, `"Shift+Tab"`, `"Meta+K"`, `"ControlOrMeta+a"`,
    /// `"Shift++"`). Modifier names: `Shift`, `Control` (`Ctrl`), `Alt`
    /// (`Option`), `Meta` (`Command`, `Cmd`), `ControlOrMeta`.
    pub fn parse(spec: &str) -> Result<KeyChord, String> {
        let (mods, key) = split_combo(spec);
        Self::from_parts(key, &mods)
    }

    /// The key and its modifiers given apart (the bridge's
    /// `{key, modifiers}`). `key` may itself be a combo; its modifiers add
    /// to `modifiers`.
    pub fn from_parts(key: &str, modifiers: &[&str]) -> Result<KeyChord, String> {
        let (inner, key) = split_combo(key);
        let mut chord = KeyChord {
            key: String::new(),
            code: String::new(),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
            control_or_meta: false,
        };
        for m in modifiers.iter().chain(inner.iter()) {
            match m.to_ascii_lowercase().as_str() {
                "shift" => chord.shift = true,
                "control" | "ctrl" => chord.ctrl = true,
                "alt" | "option" => chord.alt = true,
                "meta" | "command" | "cmd" => chord.meta = true,
                "controlormeta" => chord.control_or_meta = true,
                _ => return Err(format!("unknown modifier '{m}' (Shift, Control, Alt, Meta, ControlOrMeta)")),
            }
        }
        let (key, code, shifted) = canonical_key(key)?;
        // A capital asked for with a shortcut modifier but no Shift
        // (`"Meta+K"`) means the key, not the capital: Cmd+K reports "k".
        // Otherwise a capital or a shifted symbol takes Shift, as a real
        // keyboard types it, and a letter with Shift held is its capital.
        let shortcut = chord.ctrl || chord.alt || chord.meta || chord.control_or_meta;
        chord.shift |= shifted && !(shortcut && single_letter(&key).is_some());
        let key = match single_letter(&key) {
            Some(c) if chord.shift => c.to_ascii_uppercase().to_string(),
            Some(c) => c.to_ascii_lowercase().to_string(),
            None => key,
        };
        chord.key = key;
        chord.code = code;
        Ok(chord)
    }

    /// Resolve `ControlOrMeta`: Meta when `shortcut_is_meta` (the
    /// platform's shortcut modifier is Command), Control otherwise.
    pub fn resolve(&self, shortcut_is_meta: bool) -> KeyPress {
        KeyPress {
            key: self.key.clone(),
            code: self.code.clone(),
            shift: self.shift,
            ctrl: self.ctrl || (self.control_or_meta && !shortcut_is_meta),
            alt: self.alt,
            meta: self.meta || (self.control_or_meta && shortcut_is_meta),
        }
    }
}

/// `"Meta+Shift+K"` → (`["Meta", "Shift"]`, `"K"`). A `+` key is the one
/// after the last separator (`"Shift++"`, `"+"`).
fn split_combo(spec: &str) -> (Vec<&str>, &str) {
    if spec.len() <= 1 {
        return (Vec::new(), spec);
    }
    let (mods, key) = if let Some(head) = spec.strip_suffix("++") {
        (head, "+")
    } else {
        match spec.rsplit_once('+') {
            Some((head, key)) => (head, key),
            None => return (Vec::new(), spec),
        }
    };
    (mods.split('+').filter(|m| !m.is_empty()).collect(), key)
}

fn single_letter(key: &str) -> Option<char> {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c),
        _ => None,
    }
}

/// `(key, code, shift)` for a key name: the value a US keyboard reports
/// and whether Shift is part of typing it.
fn canonical_key(name: &str) -> Result<(String, String, bool), String> {
    if name.is_empty() {
        return Err("empty key".into());
    }
    if let Some(&named) = NAMED.iter().find(|n| **n == name) {
        return Ok((named.to_string(), named.to_string(), false));
    }
    match name {
        "Space" | " " => return Ok((" ".into(), "Space".into(), false)),
        "Shift" | "Control" | "Alt" | "Meta" => return Ok((name.into(), format!("{name}Left"), false)),
        _ => {}
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=24).contains(&n) {
            return Ok((name.into(), name.into(), false));
        }
    }
    if let Some(l) = name.strip_prefix("Key").and_then(single_letter) {
        return Ok((l.to_ascii_lowercase().to_string(), name.into(), false));
    }
    if let Some(d) = name.strip_prefix("Digit").filter(|d| d.len() == 1 && d.as_bytes()[0].is_ascii_digit()) {
        return Ok((d.into(), name.into(), false));
    }
    let mut chars = name.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Err(format!(
            "unknown key '{name}' (a single character, or Enter, Escape, Tab, Backspace, Delete, \
             Insert, Home, End, PageUp, PageDown, ArrowUp/Down/Left/Right, Space, F1–F24, Shift, \
             Control, Alt, Meta, KeyA–KeyZ, Digit0–Digit9)"
        ));
    };
    let (code, shift) = us_code(c);
    Ok((c.to_string(), code, shift))
}

/// The US-layout `code` for a character and whether it takes Shift.
fn us_code(c: char) -> (String, bool) {
    if c.is_ascii_alphabetic() {
        return (format!("Key{}", c.to_ascii_uppercase()), c.is_ascii_uppercase());
    }
    if c.is_ascii_digit() {
        return (format!("Digit{c}"), false);
    }
    const SHIFTED_DIGITS: &str = ")!@#$%^&*(";
    if let Some(i) = SHIFTED_DIGITS.find(c) {
        return (format!("Digit{i}"), true);
    }
    let (code, shift) = match c {
        '`' => ("Backquote", false),
        '~' => ("Backquote", true),
        '-' => ("Minus", false),
        '_' => ("Minus", true),
        '=' => ("Equal", false),
        '+' => ("Equal", true),
        '[' => ("BracketLeft", false),
        '{' => ("BracketLeft", true),
        ']' => ("BracketRight", false),
        '}' => ("BracketRight", true),
        '\\' => ("Backslash", false),
        '|' => ("Backslash", true),
        ';' => ("Semicolon", false),
        ':' => ("Semicolon", true),
        '\'' => ("Quote", false),
        '"' => ("Quote", true),
        ',' => ("Comma", false),
        '<' => ("Comma", true),
        '.' => ("Period", false),
        '>' => ("Period", true),
        '/' => ("Slash", false),
        '?' => ("Slash", true),
        _ => ("", false),
    };
    (code.into(), shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(spec: &str, meta: bool) -> KeyPress {
        KeyChord::parse(spec).unwrap().resolve(meta)
    }

    #[test]
    fn named_keys_carry_their_web_key_and_code() {
        let p = press("Enter", false);
        assert_eq!((p.key.as_str(), p.code.as_str(), p.shift), ("Enter", "Enter", false));
        let p = press("Space", false);
        assert_eq!((p.key.as_str(), p.code.as_str()), (" ", "Space"));
        assert_eq!(press("F5", false).code, "F5");
        assert_eq!(press("Meta", false).code, "MetaLeft");
        assert_eq!(press("KeyQ", false).key, "q");
        assert_eq!(press("Digit7", false).code, "Digit7");
        assert!(KeyChord::parse("F25").is_err());
        assert!(KeyChord::parse("Return").unwrap_err().contains("unknown key 'Return'"));
    }

    #[test]
    fn characters_carry_the_us_code_and_shift_a_real_keyboard_reports() {
        let p = press("a", false);
        assert_eq!((p.key.as_str(), p.code.as_str(), p.shift), ("a", "KeyA", false));
        let p = press("A", false);
        assert_eq!((p.key.as_str(), p.code.as_str(), p.shift), ("A", "KeyA", true));
        let p = press("!", false);
        assert_eq!((p.code.as_str(), p.shift), ("Digit1", true));
        assert_eq!(press("/", false).code, "Slash");
        assert_eq!(press("é", false).code, "");
        assert_eq!(press("a", false).text(), Some('a'));
        assert_eq!(press("Enter", false).text(), None);
    }

    #[test]
    fn combos_split_modifiers_from_the_key() {
        let p = press("Meta+K", false);
        assert_eq!((p.key.as_str(), p.code.as_str(), p.meta, p.shift), ("k", "KeyK", true, false));
        assert_eq!(p.text(), None, "a shortcut types nothing");
        let p = press("Shift+k", false);
        assert_eq!((p.key.as_str(), p.shift), ("K", true));
        let p = press("Control+Shift+ArrowLeft", false);
        assert!(p.ctrl && p.shift && !p.meta);
        assert_eq!(p.key, "ArrowLeft");
        let p = press("Shift++", false);
        assert_eq!((p.key.as_str(), p.code.as_str(), p.shift), ("+", "Equal", true));
        assert_eq!(press("+", false).key, "+");
        assert!(KeyChord::parse("Hyper+K").unwrap_err().contains("unknown modifier 'Hyper'"));
    }

    #[test]
    fn control_or_meta_resolves_per_platform() {
        let apple = press("ControlOrMeta+a", true);
        assert!(apple.meta && !apple.ctrl);
        let other = press("ControlOrMeta+a", false);
        assert!(other.ctrl && !other.meta);
        let parts = KeyChord::from_parts("k", &["ControlOrMeta", "shift"]).unwrap().resolve(true);
        assert!(parts.meta && parts.shift);
        assert_eq!(parts.key, "K");
    }
}
