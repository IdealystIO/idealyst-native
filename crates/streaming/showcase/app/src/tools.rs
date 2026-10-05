//! Native code the bundle calls: every kind of `#[host_fn]`, and a remote
//! screen calling them as it would call any Rust function.
//!
//! The functions compile into both builds. In the app each is the function
//! as written; in the bundle, a stub that asks the app to run it. The
//! `ToolsScreen` body runs in the bundle, with key and value types of its
//! own that the app never compiles — the generic functions still work,
//! because the app runs them on those values' bytes.
//!
//! | Function | Shows |
//! |---|---|
//! | [`word_count`] | sync, plain values |
//! | [`invoice_total`] | structs both sides compile (`#[derive(Remote)]`), `Result` |
//! | [`histogram`] | a list of numbers (crosses as one byte run) |
//! | [`digest`] | `async`: the app runs it on its executor |
//! | [`sort_order`] | `K: Key`: the order that sorts any key — a bundle-only struct here |
//! | [`distinct`] | `K: Key` in and out: the keys come back as the bundle's type |
//! | [`group_by`] | `K: Key` with a value carried unread (`V`, an `Opaque`) |
//! | [`join`] | two lists matched by key |
//! | [`stats`] / [`sorted`] | `N: Numeric`: compiled for every number type |
//! | [`top`] | an async generic function |

use std::collections::{BTreeMap, HashMap, HashSet};

use runtime_core::{component, host_fn, signal, ui, Element, Key, Numeric, Remote, Signal};

use super::{column, Body, Heading, Muted, Page};

// ---------------------------------------------------------------------------
// Plain host functions
// ---------------------------------------------------------------------------

/// Sync, plain values in and out.
#[host_fn]
pub fn word_count(text: String) -> u32 {
    text.split_whitespace().count() as u32
}

/// A value type both builds compile: it crosses field by field.
#[derive(Clone, Debug, PartialEq, Remote)]
pub struct Invoice {
    pub lines: Vec<InvoiceLine>,
    pub tax_percent: u32,
}

#[derive(Clone, Debug, PartialEq, Remote)]
pub struct InvoiceLine {
    pub item: String,
    pub cents: u64,
    pub qty: u32,
}

#[derive(Clone, Debug, PartialEq, Remote)]
pub enum InvoiceError {
    Empty,
    OverLimit { cents: u64 },
}

/// Structs in, a `Result` out.
#[host_fn]
pub fn invoice_total(invoice: Invoice) -> Result<u64, InvoiceError> {
    if invoice.lines.is_empty() {
        return Err(InvoiceError::Empty);
    }
    let net: u64 = invoice.lines.iter().map(|l| l.cents * l.qty as u64).sum();
    let total = net + net * invoice.tax_percent as u64 / 100;
    if total > 1_000_000 {
        return Err(InvoiceError::OverLimit { cents: total });
    }
    Ok(total)
}

/// Counts of `samples` in `buckets` equal ranges over 0.0–1.0. A list of
/// numbers crosses as one byte run, so this costs the bundle a memcpy.
#[host_fn]
pub fn histogram(samples: Vec<f32>, buckets: u32) -> Vec<u32> {
    let mut counts = vec![0u32; buckets as usize];
    for s in samples {
        let i = ((s.clamp(0.0, 1.0) * buckets as f32) as usize).min(buckets as usize - 1);
        counts[i] += 1;
    }
    counts
}

/// Async: the app runs it on its own executor (a delay stands in for disk
/// or network) and the bundle's `spawn_then` gets the result.
#[host_fn]
pub async fn digest(bytes: Vec<u8>) -> String {
    super::app::delay(DIGEST_DELAY_MS).await;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// How long [`digest`] takes (the flow test waits it out).
pub const DIGEST_DELAY_MS: i32 = 300;

// ---------------------------------------------------------------------------
// Generic host functions
// ---------------------------------------------------------------------------

/// The order that sorts `keys` (stable): `keys[order[0]]` is the
/// smallest. Any `Key` — the bundle's own types included.
#[host_fn]
pub fn sort_order<K: Key>(keys: Vec<K>) -> Vec<u32> {
    let mut order: Vec<u32> = (0..keys.len() as u32).collect();
    order.sort_by(|a, b| keys[*a as usize].cmp(&keys[*b as usize]));
    order
}

/// Each key once, in first-seen order. The keys come back as the caller's
/// type.
#[host_fn]
pub fn distinct<K: Key>(items: Vec<K>) -> Vec<K> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|k| seen.insert(k.clone())).collect()
}

/// Rows grouped by key, groups in key order. The values (`V`) are carried
/// along unread: the app never needs their type.
#[host_fn]
pub fn group_by<K: Key, V>(rows: Vec<(K, V)>) -> Vec<(K, Vec<V>)> {
    let mut groups: BTreeMap<K, Vec<V>> = BTreeMap::new();
    for (k, v) in rows {
        groups.entry(k).or_default().push(v);
    }
    groups.into_iter().collect()
}

/// Every `(a, b)` whose keys match, in `left`'s order.
#[host_fn]
pub fn join<K: Key, A: Clone, B: Clone>(left: Vec<(K, A)>, right: Vec<(K, B)>) -> Vec<(A, B)> {
    let mut by_key: HashMap<K, Vec<B>> = HashMap::new();
    for (k, b) in right {
        by_key.entry(k).or_default().push(b);
    }
    let mut out = Vec::new();
    for (k, a) in left {
        for b in by_key.get(&k).into_iter().flatten() {
            out.push((a.clone(), b.clone()));
        }
    }
    out
}

/// A summary of a list of numbers, any number type.
#[derive(Clone, Debug, PartialEq, Remote)]
pub struct Stats {
    pub count: u32,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

#[host_fn]
pub fn stats<N: Numeric>(values: Vec<N>) -> Stats {
    let count = values.len() as u32;
    let min = values.iter().copied().min_by(N::total_cmp).map_or(0.0, N::to_f64);
    let max = values.iter().copied().max_by(N::total_cmp).map_or(0.0, N::to_f64);
    let mean = if count == 0 { 0.0 } else { values.iter().map(|v| v.to_f64()).sum::<f64>() / count as f64 };
    Stats { count, min, max, mean }
}

/// The numbers, sorted (floats too: `NaN`s last).
#[host_fn]
pub fn sorted<N: Numeric>(values: Vec<N>) -> Vec<N> {
    let mut values = values;
    values.sort_unstable_by(N::total_cmp);
    values
}

/// The `n` largest keys, largest first. Async and generic.
#[host_fn]
pub async fn top<K: Key>(keys: Vec<K>, n: u32) -> Vec<K> {
    let mut keys = keys;
    keys.sort_unstable_by(|a, b| b.cmp(a));
    keys.truncate(n as usize);
    keys
}

/// What remote code may call from here (the app's allowlist adds these).
#[cfg(not(idealyst_stream_guest))]
pub fn host_fns() -> Vec<runtime_vocabulary::remote::HostFnDef> {
    vec![
        word_count::export(),
        invoice_total::export(),
        histogram::export(),
        digest::export(),
        sort_order::export(),
        distinct::export(),
        group_by::export(),
        join::export(),
        stats::export(),
        sorted::export(),
        top::export(),
    ]
}

// ---------------------------------------------------------------------------
// The screen: bundle code calling them
// ---------------------------------------------------------------------------

/// Staff, as the bundle holds them: `(name, team, hired)`.
pub const STAFF: &[(&str, &str, u16)] = &[
    ("Ada", "eng", 2019),
    ("Lin", "ops", 2021),
    ("Sam", "eng", 2017),
    ("Kim", "sales", 2021),
    ("Ola", "ops", 2018),
    ("Bea", "eng", 2021),
];

/// Orders by staff name: `(name, product)`.
pub const ORDERS: &[(&str, &str)] = &[("Sam", "Lamp"), ("Ada", "Mug"), ("Sam", "Tent"), ("Zed", "Rope")];

/// The tools tab: every host function above, called from bundle code.
#[component(remote)]
pub fn ToolsScreen() -> Element {
    // The bundle's own types: the app never compiles these.
    #[derive(Clone, Debug, Key)]
    struct Seniority {
        team: String,
        hired: u16,
    }
    #[derive(Clone, Debug, Remote)]
    struct Person {
        name: String,
        hired: u16,
    }
    let person = |&(name, _, hired): &(&str, &str, u16)| Person { name: name.to_string(), hired };

    // Plain calls.
    let words = word_count("the bundle asks the app to count these words".to_string());
    let invoice = Invoice {
        lines: vec![
            InvoiceLine { item: "Mug".into(), cents: 1800, qty: 2 },
            InvoiceLine { item: "Lamp".into(), cents: 1500, qty: 1 },
        ],
        tax_percent: 10,
    };
    let total = match invoice_total(invoice) {
        Ok(cents) => format!("${}.{:02}", cents / 100, cents % 100),
        Err(e) => format!("{e:?}"),
    };
    let empty = invoice_total(Invoice { lines: vec![], tax_percent: 0 });
    let hist = histogram(vec![0.1, 0.15, 0.5, 0.9, 0.95, 0.99], 3);

    // Generic calls, on the bundle's own types.
    let keys: Vec<Seniority> = STAFF.iter().map(|&(_, team, hired)| Seniority { team: team.to_string(), hired }).collect();
    let by_team: Vec<&str> = sort_order(keys).into_iter().map(|i| STAFF[i as usize].0).collect();
    let teams = distinct(STAFF.iter().map(|s| s.1.to_string()).collect());
    let groups = group_by(STAFF.iter().map(|s| (s.1.to_string(), person(s))).collect());
    let groups: Vec<String> = groups
        .into_iter()
        .map(|(team, people)| format!("{team}: {}", people.iter().map(|p| format!("{} ({})", p.name, p.hired)).collect::<Vec<_>>().join(", ")))
        .collect();
    let joined = join(
        STAFF.iter().map(|s| (s.0.to_string(), s.0.to_string())).collect(),
        ORDERS.iter().map(|o| (o.0.to_string(), o.1.to_string())).collect(),
    );
    let joined: Vec<String> = joined.into_iter().map(|(who, what)| format!("{who}→{what}")).collect();
    let s = stats(vec![2.5f64, 9.0, 1.5, 4.0]);
    let small = sorted(vec![7i16, -3, 0, 12, -40]);

    // Async calls: the results arrive later.
    let latest: Signal<Option<Vec<u16>>> = signal(None);
    runtime_core::spawn_then(top(STAFF.iter().map(|s| s.2).collect(), 2), move |t| latest.set(Some(t)));
    let hash: Signal<Option<String>> = signal(None);
    runtime_core::spawn_then(digest(b"remote".to_vec()), move |h| hash.set(Some(h)));

    ui! {
        scroll_view(style = Page()) {
            view(style = column(6.0)) {
                text(style = Heading()) { "Tools — the bundle calling the app" }
                text(style = Body()) { format!("words: {words}") }
                text(style = Body()) { format!("invoice: {total}") }
                text(style = Body()) { format!("empty invoice: {empty:?}") }
                text(style = Body()) { format!("histogram: {hist:?}") }
                text(style = Body()) { format!("by team, then hired: {}", by_team.join(", ")) }
                text(style = Body()) { format!("teams: {}", teams.join(", ")) }
                for g in groups {
                    text(style = Body()) { g }
                }
                text(style = Body()) { format!("orders: {}", joined.join(", ")) }
                text(style = Body()) { format!("stats: n={} min={} max={} mean={}", s.count, s.min, s.max, s.mean) }
                text(style = Body()) { format!("sorted: {small:?}") }
                text(style = Muted()) { move || match latest.get() {
                    None => "latest hires: …".to_string(),
                    Some(t) => format!("latest hires: {t:?}"),
                } }
                text(style = Muted()) { move || match hash.get() {
                    None => "digest: …".to_string(),
                    Some(h) => format!("digest: {h}"),
                } }
            }
        }
    }
}

/// The app's side of a bundle's call, driven with raw bytes: what arrives
/// is untrusted, so a bad call is an `Err` (the loader stops that bundle),
/// never a panic in the app.
#[cfg(all(test, not(idealyst_stream_guest)))]
mod tests {
    use runtime_vocabulary::host_types::{KeyBytes, Numeric};
    use runtime_vocabulary::remote::host_fn::{encode_keys, HostFnKind};
    use runtime_vocabulary::remote::RemoteValue;

    fn sync(def: runtime_vocabulary::remote::HostFnDef) -> fn(&[u8]) -> Result<Vec<u8>, String> {
        match def.kind {
            HostFnKind::Sync(f) => f,
            HostFnKind::Async(_) => panic!("sync"),
        }
    }

    /// A numeric call starts with its number type's tag; the app runs the
    /// instantiation for that type.
    #[test]
    fn a_numeric_call_runs_the_instantiation_its_tag_names() {
        let call = sync(super::sorted::export());
        let mut args = vec![<u32 as Numeric>::TAG];
        vec![3u32, 1, 2].encode(&mut args);
        assert_eq!(Vec::<u32>::decode(&mut &call(&args).unwrap()[..]).unwrap(), [1, 2, 3]);
        let mut args = vec![<f64 as Numeric>::TAG];
        vec![2.0f64, -1.0].encode(&mut args);
        assert_eq!(Vec::<f64>::decode(&mut &call(&args).unwrap()[..]).unwrap(), [-1.0, 2.0]);
    }

    #[test]
    fn a_numeric_call_without_a_known_tag_is_refused() {
        let call = sync(super::sorted::export());
        assert!(call(&[]).unwrap_err().contains("missing its number types"));
        assert!(call(&[200, 0]).unwrap_err().contains("no number type has tag 200"));
    }

    /// A key call reads the bundle's keys (here: strings, so each one
    /// framed); a malformed list is refused.
    #[test]
    fn a_key_call_sorts_framed_keys_and_refuses_malformed_ones() {
        let call = sync(super::sort_order::export());
        let keys = [(2u8, "b".to_string()), (1, "z".to_string()), (2, "a".to_string())];
        let mut args = Vec::new();
        encode_keys(&keys, &mut args);
        let reply = call(&args).unwrap();
        assert_eq!(Vec::<u32>::decode(&mut &reply[..]).unwrap(), [1, 2, 0]);
        // One framed key claiming 50 bytes, 1 there.
        assert!(call(&[1, 0, 50, 0, 0, 0, 1]).is_err(), "a frame longer than the call");
        // The app's stand-in is the same function a native caller runs.
        assert_eq!(super::sort_order(keys.iter().map(KeyBytes::of).collect()), [1, 2, 0]);
    }
}
