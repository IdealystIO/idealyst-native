//! A test's hand on the app's HTTP. See the crate's `Cargo.toml` for why
//! this sits below `net`.
//!
//! A test adds [`Rule`]s. Each request `net` sends is matched against them
//! ([`intercept`]) before it reaches the platform: the newest matching rule
//! decides. A **hold** parks the request (it is listed by [`held`]) until
//! [`release`] lets it go on to the network, or fails it; a **fail**
//! answers it at once with a status or a transport error, and it never
//! reaches the network. A delay is a hold the caller releases on a timer,
//! which this crate (no threads, no clock: it runs on the web too) leaves
//! to the robot.
//!
//! Process-wide: requests are sent from any thread.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// How a matched request is answered without the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// An HTTP response with this status and body.
    Status { code: u16, body: String },
    /// A transport error before any response.
    Network(String),
    /// The request timed out.
    Timeout,
    /// The device is offline.
    Offline,
}

/// What a rule does to the requests it matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Park the request until released.
    Hold,
    /// Answer it at once.
    Fail(Failure),
}

/// Which requests a rule applies to, and what it does.
#[derive(Clone, Debug)]
pub struct Rule {
    /// Matches when the request URL contains this; `*` matches any run of
    /// characters (`/_srv/*search*`).
    pub url: String,
    /// Only this method (`GET`, `POST`, …), any when `None`.
    pub method: Option<String>,
    pub action: Action,
    /// Apply to this many requests, then retire; `None`: until removed.
    pub times: Option<u32>,
}

/// What [`intercept`] decided for one request.
pub enum Decision {
    /// Send it.
    Pass,
    /// Answer it with this failure; do not send it.
    Fail(Failure),
    /// Await this, then act on what it resolves to.
    Hold(HoldFuture),
}

/// A held request, as [`held`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub id: u64,
    pub rule: u64,
    pub method: String,
    pub url: String,
}

#[derive(Default)]
struct Gate {
    outcome: Option<Option<Failure>>,
    waker: Option<Waker>,
}

/// Resolves when the hold is released: `None` sends the request, `Some`
/// fails it.
pub struct HoldFuture {
    gate: Arc<Mutex<Gate>>,
}

impl Future for HoldFuture {
    type Output = Option<Failure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut g = lock(&self.gate);
        match g.outcome.take() {
            Some(out) => Poll::Ready(out),
            None => {
                g.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

struct Parked {
    held: Held,
    gate: Arc<Mutex<Gate>>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    /// `(id, rule, uses left)`, oldest first.
    rules: Vec<(u64, Rule, Option<u32>)>,
    parked: Vec<Parked>,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<State>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut s = lock(&STATE);
    let state = s.get_or_insert_with(State::default);
    let r = f(state);
    ACTIVE.store(!state.rules.is_empty() || !state.parked.is_empty(), Ordering::Release);
    r
}

/// `*`-glob containment: `pattern` occurs in `text`, each `*` any run.
fn matches(pattern: &str, text: &str) -> bool {
    // A containment test, not a whole-URL match: each piece between the
    // `*`s must occur, in order, anywhere after the previous one.
    let mut at = 0;
    for part in pattern.split('*').filter(|p| !p.is_empty()) {
        match text[at..].find(part) {
            Some(pos) => at += pos + part.len(),
            None => return false,
        }
    }
    true
}

/// Add a rule; returns its id. The newest matching rule wins.
pub fn add_rule(rule: Rule) -> u64 {
    with_state(|s| {
        s.next_id += 1;
        let id = s.next_id;
        let left = rule.times;
        s.rules.push((id, rule, left));
        id
    })
}

/// Remove a rule (requests it already holds stay held).
pub fn remove_rule(id: u64) -> bool {
    with_state(|s| {
        let before = s.rules.len();
        s.rules.retain(|(r, _, _)| *r != id);
        s.rules.len() != before
    })
}

/// Decide what happens to a request about to be sent.
#[inline]
pub fn intercept(method: &str, url: &str) -> Decision {
    if !ACTIVE.load(Ordering::Acquire) {
        return Decision::Pass;
    }
    with_state(|s| {
        let Some(pos) = s.rules.iter().rposition(|(_, r, _)| {
            matches(&r.url, url) && r.method.as_deref().map_or(true, |m| m.eq_ignore_ascii_case(method))
        }) else {
            return Decision::Pass;
        };
        let (rule_id, action) = {
            let (id, rule, left) = &mut s.rules[pos];
            if let Some(n) = left {
                *n -= 1;
            }
            (*id, rule.action.clone())
        };
        if s.rules[pos].2 == Some(0) {
            s.rules.remove(pos);
        }
        match action {
            Action::Fail(f) => Decision::Fail(f),
            Action::Hold => {
                s.next_id += 1;
                let gate = Arc::new(Mutex::new(Gate::default()));
                let held = Held { id: s.next_id, rule: rule_id, method: method.to_ascii_uppercase(), url: url.to_string() };
                s.parked.push(Parked { held, gate: gate.clone() });
                Decision::Hold(HoldFuture { gate })
            }
        }
    })
}

/// The requests held now, oldest first.
pub fn held() -> Vec<Held> {
    if !ACTIVE.load(Ordering::Acquire) {
        return Vec::new();
    }
    with_state(|s| s.parked.iter().map(|p| p.held.clone()).collect())
}

/// Which held requests [`release`] lets go.
#[derive(Clone, Copy, Debug)]
pub enum Which {
    /// The held request with this id.
    Request(u64),
    /// Every request held by this rule.
    Rule(u64),
    All,
}

/// Let held requests go: on to the network (`outcome: None`) or answered
/// with a failure. Returns how many were released.
pub fn release(which: Which, outcome: Option<Failure>) -> usize {
    let gates: Vec<Arc<Mutex<Gate>>> = with_state(|s| {
        let (go, stay): (Vec<Parked>, Vec<Parked>) = s.parked.drain(..).partition(|p| match which {
            Which::Request(id) => p.held.id == id,
            Which::Rule(id) => p.held.rule == id,
            Which::All => true,
        });
        s.parked = stay;
        go.into_iter().map(|p| p.gate).collect()
    });
    let n = gates.len();
    for gate in gates {
        let waker = {
            let mut g = lock(&gate);
            g.outcome = Some(outcome.clone());
            g.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }
    n
}

/// Remove every rule and send every held request on.
pub fn clear() {
    with_state(|s| s.rules.clear());
    release(Which::All, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Waker;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn poll(f: &mut HoldFuture) -> Poll<Option<Failure>> {
        Pin::new(f).poll(&mut Context::from_waker(Waker::noop()))
    }

    fn rule(url: &str, action: Action) -> Rule {
        Rule { url: url.into(), method: None, action, times: None }
    }

    #[test]
    fn nothing_set_passes() {
        let _s = lock(&SERIAL);
        clear();
        assert!(matches!(intercept("GET", "https://x/a"), Decision::Pass));
    }

    #[test]
    fn a_hold_parks_until_released_then_sends_or_fails() {
        let _s = lock(&SERIAL);
        clear();
        let r = add_rule(rule("/_srv/*search*", Action::Hold));
        let Decision::Hold(mut a) = intercept("POST", "http://h/_srv/project_search") else { panic!("held") };
        let Decision::Hold(mut b) = intercept("post", "http://h/_srv/person_search") else { panic!("held") };
        assert!(matches!(intercept("POST", "http://h/_srv/save"), Decision::Pass), "no match passes");
        assert!(poll(&mut a).is_pending());
        let listed = held();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].method, "POST");
        assert_eq!(release(Which::Request(listed[0].id), None), 1);
        assert_eq!(poll(&mut a), Poll::Ready(None), "released: send it");
        assert!(poll(&mut b).is_pending());
        assert_eq!(release(Which::Rule(r), Some(Failure::Offline)), 1);
        assert_eq!(poll(&mut b), Poll::Ready(Some(Failure::Offline)));
        clear();
    }

    #[test]
    fn a_fail_answers_at_once_and_times_retire_a_rule() {
        let _s = lock(&SERIAL);
        clear();
        add_rule(Rule {
            url: "/api/".into(),
            method: Some("GET".into()),
            action: Action::Fail(Failure::Status { code: 503, body: "down".into() }),
            times: Some(1),
        });
        assert!(matches!(intercept("POST", "http://h/api/x"), Decision::Pass), "other method");
        match intercept("GET", "http://h/api/x") {
            Decision::Fail(Failure::Status { code: 503, .. }) => {}
            _ => panic!("failed with 503"),
        }
        assert!(matches!(intercept("GET", "http://h/api/x"), Decision::Pass), "used up");
    }

    #[test]
    fn the_newest_matching_rule_wins_and_clear_sends_held_on() {
        let _s = lock(&SERIAL);
        clear();
        add_rule(rule("/x", Action::Fail(Failure::Timeout)));
        add_rule(rule("/x", Action::Hold));
        let Decision::Hold(mut h) = intercept("GET", "http://h/x") else { panic!("newest: hold") };
        clear();
        assert_eq!(poll(&mut h), Poll::Ready(None));
        assert!(held().is_empty());
    }

    #[test]
    fn glob_matching() {
        assert!(matches("/_srv/*search", "http://h/_srv/project_search"));
        assert!(matches("search", "http://h/_srv/project_search?q=1"));
        assert!(!matches("/_srv/*search", "http://h/_srv/save"));
        assert!(matches("*", "anything"));
    }
}
