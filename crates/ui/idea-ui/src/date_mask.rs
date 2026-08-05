//! Smart-typing mask for the typed date/time inputs (`DateInput`,
//! `TimeInput`, `DateTimeInput`): delimiters insert themselves and the
//! caret "jumps" between numeric segments as fast as the digits allow.
//!
//! The mask is derived from the SAME token stream the parser/formatter
//! use ([`crate::date::lex`]), so it can never disagree with what
//! `parse_date`/`format_date` accept. It only ever engages on text
//! **appended at the end** of the field — deletions, mid-string edits,
//! and text that doesn't align with the format pass through untouched,
//! and the lenient parser handles them as before. That containment is
//! what makes the mask safe: it can only ever rewrite text it fully
//! understands.
//!
//! ## Behavior (the "as quick as possible" rules)
//!
//! With format `MM/DD/YYYY`:
//! - Typing `2` in the month completes it immediately — no month
//!   continues with a second digit after `2` (2×10 > 12) — so the text
//!   becomes `02/` in one keystroke. Same for day `4`–`9` (4×10 > 31).
//! - Typing `1` waits: the month could be 1, 10, 11 or 12. `Tab`
//!   commits the ambiguous `1` as `01/` and moves to the day.
//! - Typing `0` waits for the second digit, and `Tab` on a bare `0` is
//!   swallowed — there is no month 0 to commit.
//! - A second digit that would overflow the segment closes it and
//!   flows into the next one: `1` then `5` → `01/5` (no month 15, so
//!   the 5 must be a day).
//! - Typing the delimiter yourself (`1` then `/`) commits the segment
//!   like `Tab` does. Any non-alphanumeric key counts — `-` advances a
//!   `/`-delimited format too, so numeric-keypad typing works.
//! - Two-digit-token segments (`MM`, `DD`, `HH`, `mm`, …) zero-pad when
//!   they close early; one-digit tokens (`M`, `D`) stay as typed and the
//!   blur canonicalization settles the final text.
//! - A meridiem token (`A`/`a`) completes from its first letter: `p` →
//!   `PM`.
//!
//! `Tab` interception rules (see [`Mask::tab`]): a valid partial
//! segment completes and focus STAYS (the jump the user asked for); an
//! invalid partial (`0` month) swallows the Tab; an empty field, a
//! complete field, an empty segment, or the LAST segment let Tab do its
//! normal focus move — the mask must never trap focus.

use crate::date::{lex, Token};

/// One numeric segment's shape: the valid value range, how many digits
/// it can hold, and whether closing early zero-pads to `max_len`
/// (two-digit tokens do; one-digit tokens render unpadded and leave
/// canonicalization to blur).
#[derive(Copy, Clone, Debug)]
struct SegSpec {
    min: u32,
    max: u32,
    max_len: usize,
    pad: bool,
}

/// One mask item: a numeric segment, a meridiem, or a literal run
/// (consecutive `Token::Literal`s merged, so `", "` inserts as one
/// unit).
#[derive(Clone, Debug)]
enum Item {
    Num(SegSpec),
    Meridiem { upper: bool },
    Lit(String),
}

/// Where the end of the current text sits relative to the format.
enum Pos {
    /// Inside numeric item `i` with `digits` typed so far (may be 0).
    InNum { item: usize, digits: String },
    /// The next expected characters are `item` (a literal run), of
    /// which `taken` chars are already present.
    AtLit { item: usize, taken: usize },
    /// At meridiem item `i`; `partial` holds a first letter (`A`/`P`
    /// in either case) if one was typed.
    AtMeridiem { item: usize, partial: Option<char> },
    /// Every item is fully consumed.
    Complete,
    /// The text does not align with the format (manual lenient typing,
    /// pasted garbage, …) — the mask must not touch it.
    Misaligned,
}

/// What a Tab press should do (see module docs).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TabAction {
    /// Replace the text with this and keep focus (PreventDefault).
    Complete(String),
    /// Swallow the Tab — the segment can't commit (e.g. month `0`).
    Swallow,
    /// Let the platform's normal Tab focus move run.
    Pass,
}

pub(crate) struct Mask {
    items: Vec<Item>,
}

impl Mask {
    pub(crate) fn new(fmt: &str) -> Self {
        let mut items: Vec<Item> = Vec::new();
        for tok in lex(fmt) {
            match tok {
                Token::Literal(c) => match items.last_mut() {
                    Some(Item::Lit(s)) => s.push(c),
                    _ => items.push(Item::Lit(c.to_string())),
                },
                Token::Meridiem { upper } => items.push(Item::Meridiem { upper }),
                t => items.push(Item::Num(seg_spec(t))),
            }
        }
        Mask { items }
    }

    /// Apply the mask to an input transition `prev` → `next`. Engages
    /// only when `next` is `prev` plus appended characters (each
    /// appended char is fed through the segment machine — so a paste of
    /// `07031994` masks the same as eight keystrokes); anything else
    /// returns `next` unchanged.
    pub(crate) fn feed(&self, prev: &str, next: &str) -> String {
        if next.len() <= prev.len() || !next.starts_with(prev) {
            return next.to_string();
        }
        let mut text = prev.to_string();
        for ch in next[prev.len()..].chars() {
            match self.feed_char(&text, ch) {
                Some(t) => text = t,
                // Misaligned: hand the raw text back untouched — the
                // mask must not mangle input it doesn't understand.
                None => return next.to_string(),
            }
        }
        text
    }

    /// Feed one appended char. `Some(new_text)` = handled (which may
    /// mean "rejected": the text comes back unchanged and the char is
    /// dropped). `None` = the current text doesn't align with the
    /// format; the caller falls back to the raw input.
    fn feed_char(&self, text: &str, ch: char) -> Option<String> {
        match self.position(text) {
            Pos::Misaligned => None,
            // Field full: extra characters are dropped, like any mask.
            Pos::Complete => Some(text.to_string()),
            Pos::AtLit { item, taken } => {
                let lit = match &self.items[item] {
                    Item::Lit(s) => s,
                    _ => unreachable!("AtLit points at a literal"),
                };
                let rest: String = lit.chars().skip(taken).collect();
                if ch.is_ascii_digit() {
                    // Digit typed where a delimiter belongs: insert the
                    // delimiter for the user and put the digit in the
                    // next segment.
                    let mut t = text.to_string();
                    t.push_str(&rest);
                    self.feed_char(&t, ch)
                } else if !ch.is_alphanumeric() {
                    // Any delimiter-ish key completes the canonical
                    // literal ("-" works in a "/"-delimited format).
                    let mut t = text.to_string();
                    t.push_str(&rest);
                    Some(t)
                } else {
                    // A letter could only be a meridiem start; let it
                    // through to the meridiem arm if the literal is
                    // done and the next item is one.
                    let mut t = text.to_string();
                    t.push_str(&rest);
                    if matches!(self.items.get(item + 1), Some(Item::Meridiem { .. })) {
                        self.feed_char(&t, ch)
                    } else {
                        Some(text.to_string())
                    }
                }
            }
            Pos::AtMeridiem { item, partial } => {
                let upper = match &self.items[item] {
                    Item::Meridiem { upper } => *upper,
                    _ => unreachable!("AtMeridiem points at a meridiem"),
                };
                let pm = match (partial, ch.to_ascii_lowercase()) {
                    (None, 'a') => Some(false),
                    (None, 'p') => Some(true),
                    (Some(p), 'm') => Some(p.to_ascii_lowercase() == 'p'),
                    _ => None,
                };
                match pm {
                    Some(pm) => {
                        let base: &str = &text[..text.len() - partial.map_or(0, |c| c.len_utf8())];
                        let word = match (pm, upper) {
                            (false, true) => "AM",
                            (true, true) => "PM",
                            (false, false) => "am",
                            (true, false) => "pm",
                        };
                        Some(format!("{base}{word}"))
                    }
                    None => Some(text.to_string()),
                }
            }
            Pos::InNum { item, digits } => {
                let spec = match &self.items[item] {
                    Item::Num(s) => *s,
                    _ => unreachable!("InNum points at a numeric segment"),
                };
                if !ch.is_ascii_digit() {
                    // Delimiter (or Tab-alike char): commit the segment
                    // if it holds a valid value; otherwise drop the key.
                    if !ch.is_alphanumeric() {
                        if let Some(t) = self.close_segment(text, item, &digits, spec) {
                            return Some(t);
                        }
                    }
                    return Some(text.to_string());
                }
                let candidate = format!("{digits}{ch}");
                let val: u32 = candidate.parse().ok()?;
                let fits = candidate.len() <= spec.max_len
                    && val <= spec.max
                    && !(candidate.len() == spec.max_len && val < spec.min);
                if fits {
                    let base = &text[..text.len() - digits.len()];
                    // Segment done when it can't take another digit:
                    // full width, or any further digit would overflow
                    // (month 2 → 2×10 > 12 → jump).
                    let done = candidate.len() == spec.max_len || val * 10 > spec.max;
                    if done && item + 1 < self.items.len() {
                        let closed = render_segment(&candidate, spec);
                        let lit = self.following_literal(item);
                        Some(format!("{base}{closed}{lit}"))
                    } else {
                        Some(format!("{base}{candidate}"))
                    }
                } else {
                    // The digit overflows this segment (month "1" +
                    // "5"): close the segment on its current digits and
                    // flow the new digit into the next one. If the
                    // current digits can't commit ("0" + "0"), drop the
                    // key.
                    match self.close_segment(text, item, &digits, spec) {
                        Some(t) => self.feed_char(&t, ch),
                        None => Some(text.to_string()),
                    }
                }
            }
        }
    }

    /// The Tab decision for the current text — see the module docs for
    /// the rules table.
    pub(crate) fn tab(&self, text: &str) -> TabAction {
        if text.is_empty() {
            return TabAction::Pass;
        }
        match self.position(text) {
            Pos::Misaligned | Pos::Complete => TabAction::Pass,
            Pos::AtLit { item, taken } => {
                // "01" with the "/" not yet typed: Tab inserts it.
                let lit = match &self.items[item] {
                    Item::Lit(s) => s,
                    _ => unreachable!(),
                };
                let rest: String = lit.chars().skip(taken).collect();
                TabAction::Complete(format!("{text}{rest}"))
            }
            Pos::AtMeridiem { partial, .. } => match partial {
                // A lone "P": unambiguous, finish it.
                Some(_) => match self.feed_char(text, 'm') {
                    Some(t) if t != text => TabAction::Complete(t),
                    _ => TabAction::Swallow,
                },
                None => TabAction::Pass,
            },
            Pos::InNum { item, digits } => {
                if digits.is_empty() {
                    // Nothing typed in this segment — don't trap focus.
                    return TabAction::Pass;
                }
                if item + 1 >= self.items.len() {
                    // Last segment: blur canonicalization finishes the
                    // job; let Tab leave the field.
                    return TabAction::Pass;
                }
                let spec = match &self.items[item] {
                    Item::Num(s) => *s,
                    _ => unreachable!(),
                };
                match self.close_segment(text, item, &digits, spec) {
                    Some(t) => TabAction::Complete(t),
                    None => TabAction::Swallow,
                }
            }
        }
    }

    /// `true` while `text` is a valid but INCOMPLETE prefix of the
    /// format — the state every keystroke passes through on the way to
    /// a full value. The typed-field wiring suppresses the parse error
    /// for these (flashing "Invalid date" after the first digit — and
    /// resizing the field to fit it — punishes normal typing); text
    /// that is complete, or that doesn't align with the format at all,
    /// reports `false` and errors as usual.
    pub(crate) fn is_incomplete_prefix(&self, text: &str) -> bool {
        matches!(
            self.position(text),
            Pos::InNum { .. } | Pos::AtLit { .. } | Pos::AtMeridiem { .. }
        )
    }

    /// Close numeric segment `item` (currently holding `digits`) if the
    /// value is valid: pad per the spec and append the following
    /// literal run. `None` = the digits don't form a committable value.
    fn close_segment(
        &self,
        text: &str,
        item: usize,
        digits: &str,
        spec: SegSpec,
    ) -> Option<String> {
        if digits.is_empty() {
            return None;
        }
        let val: u32 = digits.parse().ok()?;
        if val < spec.min || val > spec.max {
            return None;
        }
        let base = &text[..text.len() - digits.len()];
        let closed = render_segment(digits, spec);
        let lit = self.following_literal(item);
        Some(format!("{base}{closed}{lit}"))
    }

    /// The literal run immediately after `item`, or `""` when the next
    /// item is another segment (compact formats like `YYYYMMDD`) or
    /// there is none.
    fn following_literal(&self, item: usize) -> &str {
        match self.items.get(item + 1) {
            Some(Item::Lit(s)) => s,
            _ => "",
        }
    }

    /// Walk the format over `text` and classify where its end sits.
    fn position(&self, text: &str) -> Pos {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        for (idx, item) in self.items.iter().enumerate() {
            match item {
                Item::Lit(lit) => {
                    for (j, expected) in lit.chars().enumerate() {
                        match chars.get(i) {
                            None => return Pos::AtLit { item: idx, taken: j },
                            Some(&c) if c == expected => i += 1,
                            Some(_) => return Pos::Misaligned,
                        }
                    }
                }
                Item::Num(spec) => {
                    let start = i;
                    while i < chars.len()
                        && i - start < spec.max_len
                        && chars[i].is_ascii_digit()
                    {
                        i += 1;
                    }
                    if i == chars.len() {
                        let digits: String = chars[start..i].iter().collect();
                        // A textually full segment is "between
                        // segments", not "in" this one — for the LAST
                        // item that means the whole field is complete.
                        // A shorter (lenient) segment stays active
                        // until a delimiter follows.
                        if digits.len() == spec.max_len {
                            continue;
                        }
                        return Pos::InNum { item: idx, digits };
                    }
                    if i == start {
                        // Segment expects a digit, text has something
                        // else (lenient manual text): not ours.
                        return Pos::Misaligned;
                    }
                    // Digits followed by more text: the delimiter check
                    // happens on the next (literal) item.
                }
                Item::Meridiem { .. } => {
                    // Prefix match (the meridiem may sit mid-format).
                    let two: String =
                        chars[i..].iter().take(2).collect::<String>().to_ascii_lowercase();
                    if two == "am" || two == "pm" {
                        i += 2;
                        continue;
                    }
                    match chars.get(i) {
                        None => return Pos::AtMeridiem { item: idx, partial: None },
                        Some(&c)
                            if i + 1 == chars.len()
                                && matches!(c.to_ascii_lowercase(), 'a' | 'p') =>
                        {
                            return Pos::AtMeridiem { item: idx, partial: Some(c) }
                        }
                        Some(_) => return Pos::Misaligned,
                    }
                }
            }
        }
        if i == chars.len() {
            Pos::Complete
        } else {
            // Trailing text past the format.
            Pos::Misaligned
        }
    }
}

/// A closed segment's text: zero-padded to the token width for
/// two-digit/`YYYY` tokens, as-typed for one-digit tokens.
fn render_segment(digits: &str, spec: SegSpec) -> String {
    if spec.pad {
        format!("{:0>width$}", digits, width = spec.max_len)
    } else {
        digits.to_string()
    }
}

fn seg_spec(tok: Token) -> SegSpec {
    match tok {
        Token::Year4 => SegSpec { min: 0, max: 9999, max_len: 4, pad: true },
        Token::Month2 => SegSpec { min: 1, max: 12, max_len: 2, pad: true },
        Token::Month1 => SegSpec { min: 1, max: 12, max_len: 2, pad: false },
        Token::Day2 => SegSpec { min: 1, max: 31, max_len: 2, pad: true },
        Token::Day1 => SegSpec { min: 1, max: 31, max_len: 2, pad: false },
        Token::Hour24Two => SegSpec { min: 0, max: 23, max_len: 2, pad: true },
        Token::Hour24One => SegSpec { min: 0, max: 23, max_len: 2, pad: false },
        Token::Hour12Two => SegSpec { min: 1, max: 12, max_len: 2, pad: true },
        Token::Hour12One => SegSpec { min: 1, max: 12, max_len: 2, pad: false },
        Token::Minute2 | Token::Second2 => SegSpec { min: 0, max: 59, max_len: 2, pad: true },
        Token::Meridiem { .. } | Token::Literal(_) => {
            unreachable!("meridiem/literal are not numeric segments")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Type `keys` one char at a time from empty, mimicking the
    /// on_change flow (each step's output is the next step's `prev`).
    fn type_out(mask: &Mask, keys: &str) -> String {
        let mut text = String::new();
        for ch in keys.chars() {
            let next = format!("{text}{ch}");
            text = mask.feed(&text, &next);
        }
        text
    }

    #[test]
    fn unambiguous_month_digit_auto_jumps() {
        let m = Mask::new("MM/DD/YYYY");
        // No month starts with 2-9: one keystroke pads + advances.
        assert_eq!(type_out(&m, "2"), "02/");
        assert_eq!(type_out(&m, "9"), "09/");
        // 1 is ambiguous (1, 10, 11, 12): wait.
        assert_eq!(type_out(&m, "1"), "1");
        // 0 waits for the second digit.
        assert_eq!(type_out(&m, "0"), "0");
        assert_eq!(type_out(&m, "07"), "07/");
    }

    #[test]
    fn overflow_digit_flows_into_next_segment() {
        let m = Mask::new("MM/DD/YYYY");
        // Month 15 doesn't exist: the 5 must be a day — and day 5 is
        // itself unambiguous (no day starts with 5), so it advances too.
        assert_eq!(type_out(&m, "15"), "01/05/");
        assert_eq!(type_out(&m, "155"), "01/05/5");
    }

    #[test]
    fn rejected_digits_are_dropped() {
        let m = Mask::new("MM/DD/YYYY");
        // "00" is no month — the second 0 is swallowed.
        assert_eq!(type_out(&m, "00"), "0");
        // Day "00" likewise, after a committed month.
        assert_eq!(type_out(&m, "1200"), "12/0");
    }

    #[test]
    fn full_typing_run_inserts_all_delimiters() {
        let m = Mask::new("MM/DD/YYYY");
        assert_eq!(type_out(&m, "07031994"), "07/03/1994");
        // Digits past a complete field are dropped.
        assert_eq!(type_out(&m, "070319941"), "07/03/1994");
        let dt = Mask::new("YYYY-MM-DD HH:mm");
        assert_eq!(type_out(&dt, "202608031430"), "2026-08-03 14:30");
    }

    #[test]
    fn paste_masks_like_keystrokes() {
        let m = Mask::new("MM/DD/YYYY");
        // One feed call with many appended chars (a paste at the end).
        assert_eq!(m.feed("", "07031994"), "07/03/1994");
    }

    #[test]
    fn manual_delimiter_commits_like_tab() {
        let m = Mask::new("MM/DD/YYYY");
        assert_eq!(type_out(&m, "1/"), "01/");
        // Any non-alphanumeric key works as the delimiter.
        assert_eq!(type_out(&m, "1-"), "01/");
        // But an empty segment can't be committed by a delimiter.
        assert_eq!(type_out(&m, "/"), "");
        assert_eq!(type_out(&m, "0/"), "0");
    }

    #[test]
    fn single_digit_tokens_do_not_pad() {
        let m = Mask::new("D/M/YYYY");
        assert_eq!(type_out(&m, "7"), "7/");
        assert_eq!(type_out(&m, "73"), "7/3/");
        assert_eq!(type_out(&m, "731994"), "7/3/1994");
    }

    #[test]
    fn time_formats_auto_advance() {
        let m = Mask::new("HH:mm");
        // Hours 0-23: 3-9 are unambiguous.
        assert_eq!(type_out(&m, "7"), "07:");
        assert_eq!(type_out(&m, "730"), "07:30");
        assert_eq!(type_out(&m, "2"), "2");
        assert_eq!(type_out(&m, "23"), "23:");
        // Minutes are the LAST segment: nothing to advance to, no
        // mid-typing pad — blur canonicalization settles "23:6".
        assert_eq!(type_out(&m, "236"), "23:6");
        // Hour 0 is VALID in a 24h clock (00:xx) — it waits, and a
        // delimiter commits it.
        assert_eq!(type_out(&m, "0:"), "00:");
    }

    #[test]
    fn meridiem_completes_from_first_letter() {
        let m = Mask::new("h:mm A");
        assert_eq!(type_out(&m, "730p"), "7:30 PM");
        assert_eq!(type_out(&m, "730a"), "7:30 AM");
        // Lowercase token renders lowercase.
        let lower = Mask::new("h:mm a");
        assert_eq!(type_out(&lower, "730P"), "7:30 pm");
        // Stray letters elsewhere are dropped.
        assert_eq!(type_out(&m, "7x30p"), "7:30 PM");
    }

    #[test]
    fn deletions_and_misaligned_text_pass_through() {
        let m = Mask::new("MM/DD/YYYY");
        // Deletion (next shorter than prev): untouched.
        assert_eq!(m.feed("07/03", "07/0"), "07/0");
        // Mid-string edit (prefix mismatch): untouched.
        assert_eq!(m.feed("07/03", "08/03"), "08/03");
        // Appending to misaligned text: raw append, no mangling.
        assert_eq!(m.feed("garbage", "garbage1"), "garbage1");
        // Resuming after a delete re-engages cleanly.
        assert_eq!(m.feed("07/", "07/4"), "07/04/");
    }

    #[test]
    fn tab_commits_ambiguous_segment() {
        let m = Mask::new("MM/DD/YYYY");
        // The user's spec: month "1" + Tab → commit as 01, jump to day.
        assert_eq!(m.tab("1"), TabAction::Complete("01/".into()));
        assert_eq!(m.tab("07/3"), TabAction::Complete("07/03/".into()));
        // Month "0" can't commit: swallow.
        assert_eq!(m.tab("0"), TabAction::Swallow);
        // Empty field / complete field / empty segment: normal Tab.
        assert_eq!(m.tab(""), TabAction::Pass);
        assert_eq!(m.tab("07/03/1994"), TabAction::Pass);
        assert_eq!(m.tab("07/"), TabAction::Pass);
        // Last segment: let Tab leave; blur canonicalizes.
        assert_eq!(m.tab("07/03/199"), TabAction::Pass);
    }

    #[test]
    fn tab_completes_pending_delimiter_and_meridiem() {
        let m = Mask::new("MM/DD/YYYY");
        // "12" is textually full but the "/" isn't typed yet.
        assert_eq!(m.tab("12"), TabAction::Complete("12/".into()));
        let t = Mask::new("h:mm A");
        assert_eq!(t.tab("7:30 P"), TabAction::Complete("7:30 PM".into()));
        // Hour 0 is valid in 24h — Tab commits it.
        let h24 = Mask::new("HH:mm");
        assert_eq!(h24.tab("0"), TabAction::Complete("00:".into()));
    }
}
