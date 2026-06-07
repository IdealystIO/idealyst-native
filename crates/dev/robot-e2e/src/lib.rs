//! `robot-e2e` — a shared, **Playwright-flavoured E2E harness** built on the
//! framework's in-process [`runtime_core::robot`] API.
//!
//! One harness, every backend: the same `locate → act → assert` vocabulary
//! Playwright gives you for the web, but driving *our* introspection
//! registry instead of a browser — so an identical suite runs on web, iOS,
//! Android, macOS, and the terminal with no per-platform runner.
//!
//! | Playwright                 | here                          | Robot call          |
//! |----------------------------|-------------------------------|---------------------|
//! | `page.getByTestId`         | [`Page::get_by_test_id`]      | `find(TestId)`      |
//! | `page.getByText`           | [`Page::get_by_text`]         | `find(LabelContains)`|
//! | `page.getByRole`           | [`Page::get_by_role`]         | `find(Kind)`        |
//! | `locator.click()`          | [`Locator::click`]            | `Robot::click`      |
//! | `locator.fill()`           | [`Locator::fill`]             | `Robot::type_text`  |
//! | `locator.setChecked()`     | [`Locator::set_toggle`]       | `Robot::set_toggle` |
//! | (slider drag)              | [`Locator::set_slider`]       | `Robot::set_slider` |
//! | `locator.focus()/.blur()`  | [`Locator::focus`]/[`blur`]   | `Robot::focus`/`blur`|
//! | `expect(l).toBeVisible()`  | [`Expect::to_be_visible`]     | `find().is_some()`  |
//! | `expect(l).toHaveText()`   | [`Expect::to_have_text`]      | live `label`        |
//! | `expect(l).toHaveCount()`  | [`Expect::to_have_count`]     | `find_all().len()`  |
//!
//! ## Output
//!
//! Every action and assertion logs an `[e2e]` line to the platform console
//! (`console.*` on web, `__android_log_print` on Android, `NSLog`/stderr on
//! Apple, stderr in the terminal). At the end of a [`run_suites`] run the
//! harness emits **one machine-readable line**:
//!
//! ```text
//! [E2E-RESULT] {"suites":3,"tests":42,"pass":42,"fail":0,"failures":[]}
//! ```
//!
//! A cross-platform orchestrator launches the app on each target, scrapes
//! that target's log channel for `[E2E-RESULT]`, and aggregates the verdict
//! — no per-platform driver needed.
//!
//! Assertions read the **live** registry label ([`Robot`] recomputes
//! reactive text on read), so a `toHaveText` immediately after a `click`
//! sees the post-update value with no explicit wait.
//!
//! [`blur`]: Locator::blur

use runtime_core::robot::{Element, ElementKind, Query, Robot};

/// Pacing between tests, in ms. Purely cosmetic: it lets the on-screen UI
/// visibly change and the console stream one test at a time, like watching
/// a real test run. Actions *within* a test are synchronous — the registry
/// recomputes reactive labels on read, so no intra-test wait is needed.
const STEP_PACING_MS: i32 = 180;

/// Flush any work the last action *deferred* to a microtask — navigator
/// screen builds, reactive effects bound through `schedule_microtask`, etc.
/// — so the next assertion sees the settled tree.
///
/// This is what makes the synchronous `act → assert` chain robust against
/// the framework's deferral points: a `pop()` that unmounts its screen on a
/// microtask, a `when` branch whose effect rebuild is buffered, and so on.
/// Without it, an assertion immediately after such an action can race the
/// deferred work and read the pre-action tree (the stack-pop "detail screen
/// still present" flake). Drains run synchronously on the calling (UI)
/// thread, mirroring what the browser does between tasks.
fn settle() {
    // Drain repeatedly: a deferred unit of work can schedule *another*
    // microtask (e.g. a stack pop's `release_screen` drops a scope, whose
    // `on_cleanup` deregisters robot entries on a follow-up tick). A single
    // drain flushes one wave; loop until the queue stays empty so cascading
    // teardown fully settles before the next assertion. Bounded so a
    // pathological self-rescheduling task can't spin forever.
    for _ in 0..SETTLE_MAX_DRAINS {
        runtime_core::drain_buffered_microtasks();
    }
}

/// Upper bound on cascading microtask waves a single `settle()` flushes.
/// Teardown chains in practice are 2–3 deep; 8 is comfortable headroom
/// without risking an unbounded spin on a self-rescheduling task.
const SETTLE_MAX_DRAINS: usize = 8;

/// `[e2e]`-prefixed info line to the platform console.
fn log(line: impl AsRef<str>) {
    runtime_core::log_info!("[e2e] {}", line.as_ref());
}

/// `[e2e]`-prefixed error line (failed assertions / actions).
fn log_fail(line: impl AsRef<str>) {
    runtime_core::log_error!("[e2e] {}", line.as_ref());
}

// ---------------------------------------------------------------------------
// Page + Locator
// ---------------------------------------------------------------------------

/// The entry point — Playwright's `page`. A zero-cost handle to the
/// current process's Robot registry.
pub struct Page;

impl Page {
    pub fn new() -> Self {
        Page
    }

    /// Locate by `test_id` (the durable, refactor-proof hook).
    pub fn get_by_test_id(&self, id: &str) -> Locator {
        Locator::new(Sel::TestId(id.to_string()), format!("getByTestId({id:?})"))
    }

    /// Locate by visible text (substring match against the live label).
    pub fn get_by_text(&self, text: &str) -> Locator {
        Locator::new(Sel::Text(text.to_string()), format!("getByText({text:?})"))
    }

    /// Locate by primitive kind, e.g. every `Button`.
    pub fn get_by_role(&self, kind: ElementKind) -> Locator {
        Locator::new(Sel::Role(kind), format!("getByRole({kind:?})"))
    }

    /// Flush deferred work (microtask-buffered navigator builds, reactive
    /// effects) so a following assertion sees the settled tree. Actions
    /// already settle automatically; call this only when a *non-action*
    /// triggered deferred work (e.g. a signal you wrote directly).
    pub fn settle(&self) {
        settle();
    }
}

impl Default for Page {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
enum Sel {
    TestId(String),
    Text(String),
    Role(ElementKind),
}

/// A located (or to-be-located) element — Playwright's `Locator`. Lazy:
/// it re-resolves against the live registry on each action/assertion, so
/// it survives re-renders just like a Playwright locator survives DOM
/// churn.
#[derive(Clone)]
pub struct Locator {
    sel: Sel,
    /// Human-readable form for the console, e.g. `getByTestId('inc')`.
    desc: String,
}

impl Locator {
    fn new(sel: Sel, desc: String) -> Self {
        Locator { sel, desc }
    }

    fn query(&self) -> Query {
        match &self.sel {
            Sel::TestId(s) => Query::test_id(s.clone()),
            Sel::Text(s) => Query::label_contains(s.clone()),
            Sel::Role(k) => Query::kind(*k),
        }
    }

    fn resolve(&self) -> Result<Element, String> {
        Robot::new()
            .find(self.query())
            .ok_or_else(|| format!("{} resolved to no element", self.desc))
    }

    /// Click / press the element (buttons, pressables, links).
    pub fn click(&self) -> Result<(), String> {
        log(format!("  ▸ {}.click()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .click(&el)
            .map_err(|e| format!("{}.click() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Type text into a text input (clears + sets, like Playwright `fill`).
    pub fn fill(&self, text: &str) -> Result<(), String> {
        log(format!("  ▸ {}.fill({text:?})", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .type_text(&el, text)
            .map_err(|e| format!("{}.fill() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Set a toggle's checked state (Playwright `setChecked`).
    pub fn set_toggle(&self, value: bool) -> Result<(), String> {
        log(format!("  ▸ {}.set_toggle({value})", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .set_toggle(&el, value)
            .map_err(|e| format!("{}.set_toggle() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Set a slider's value (simulates a drag to `value`).
    pub fn set_slider(&self, value: f32) -> Result<(), String> {
        log(format!("  ▸ {}.set_slider({value})", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .set_slider(&el, value)
            .map_err(|e| format!("{}.set_slider() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Focus the element (text inputs).
    pub fn focus(&self) -> Result<(), String> {
        log(format!("  ▸ {}.focus()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .focus(&el)
            .map_err(|e| format!("{}.focus() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Blur the element.
    pub fn blur(&self) -> Result<(), String> {
        log(format!("  ▸ {}.blur()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .blur(&el)
            .map_err(|e| format!("{}.blur() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Number of elements this locator currently matches.
    pub fn count(&self) -> usize {
        Robot::new().find_all(self.query()).len()
    }
}

// ---------------------------------------------------------------------------
// Assertions — expect(...)
// ---------------------------------------------------------------------------

/// Wrap a locator in an assertion context — Playwright's `expect(...)`.
pub fn expect(locator: &Locator) -> Expect {
    Expect {
        loc: locator.clone(),
    }
}

pub struct Expect {
    loc: Locator,
}

impl Expect {
    /// `expect(locator).toBeVisible()` — the element is in the live tree.
    pub fn to_be_visible(self) -> Result<(), String> {
        match self.loc.resolve() {
            Ok(_) => {
                log(format!("  ✓ expect({}).toBeVisible()", self.loc.desc));
                Ok(())
            }
            Err(_) => Err(format!(
                "expect({}).toBeVisible() — element not found",
                self.loc.desc
            )),
        }
    }

    /// `expect(locator).not.toBeVisible()` — the element is absent.
    pub fn not_to_be_visible(self) -> Result<(), String> {
        match self.loc.resolve() {
            Err(_) => {
                log(format!("  ✓ expect({}).not.toBeVisible()", self.loc.desc));
                Ok(())
            }
            Ok(_) => Err(format!(
                "expect({}).not.toBeVisible() — element is present",
                self.loc.desc
            )),
        }
    }

    /// `expect(locator).toHaveText(expected)` — exact match against the
    /// element's live label.
    pub fn to_have_text(self, expected: &str) -> Result<(), String> {
        let el = self.loc.resolve()?;
        match el.label.as_deref() {
            Some(actual) if actual == expected => {
                log(format!(
                    "  ✓ expect({}).toHaveText({expected:?})",
                    self.loc.desc
                ));
                Ok(())
            }
            other => Err(format!(
                "expect({}).toHaveText({expected:?}) — actual text was {:?}",
                self.loc.desc, other
            )),
        }
    }

    /// `expect(locator).toContainText(substr)` — substring match against
    /// the element's live label.
    pub fn to_contain_text(self, substr: &str) -> Result<(), String> {
        let el = self.loc.resolve()?;
        match el.label.as_deref() {
            Some(actual) if actual.contains(substr) => {
                log(format!(
                    "  ✓ expect({}).toContainText({substr:?})",
                    self.loc.desc
                ));
                Ok(())
            }
            other => Err(format!(
                "expect({}).toContainText({substr:?}) — actual text was {:?}",
                self.loc.desc, other
            )),
        }
    }

    /// `expect(locator).toHaveCount(n)` — number of matches.
    pub fn to_have_count(self, n: usize) -> Result<(), String> {
        let actual = self.loc.count();
        if actual == n {
            log(format!("  ✓ expect({}).toHaveCount({n})", self.loc.desc));
            Ok(())
        } else {
            Err(format!(
                "expect({}).toHaveCount({n}) — actual count was {actual}",
                self.loc.desc
            ))
        }
    }

    /// `expect(locator).toHaveMinCount(n)` — at least `n` matches. Useful
    /// when a screen renders an unknown-but-bounded number of a kind.
    pub fn to_have_min_count(self, n: usize) -> Result<(), String> {
        let actual = self.loc.count();
        if actual >= n {
            log(format!(
                "  ✓ expect({}).toHaveMinCount({n}) (actual {actual})",
                self.loc.desc
            ));
            Ok(())
        } else {
            Err(format!(
                "expect({}).toHaveMinCount({n}) — actual count was {actual}",
                self.loc.desc
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Test + Suite + runner
// ---------------------------------------------------------------------------

/// One named test case — Playwright's `test('name', async ({page}) => …)`.
///
/// Two shapes:
/// - **Sync** ([`test`]): one closure run synchronously, `?`-chaining
///   actions and assertions. Right for purely-synchronous UI (reactive
///   signals, `when` branches — they settle by the next line).
/// - **Flow** ([`flow`]): an ordered list of steps the runner advances
///   **one per real scheduler tick**, so macrotask-/event-driven work (a
///   stack `pop` that completes on the browser's async `popstate`) settles
///   between an action and the assertion that checks it. `poll` steps retry
///   across ticks until they pass or time out (Playwright's auto-waiting).
pub struct Test {
    name: String,
    body: Body,
}

enum Body {
    Sync(Box<dyn Fn(&Page) -> Result<(), String>>),
    Flow(Vec<Step>),
}

struct Step {
    kind: StepKind,
    run: Box<dyn Fn(&Page) -> Result<(), String>>,
}

enum StepKind {
    /// Runs once; an `Err` fails the test immediately. Use for actions and
    /// synchronous assertions.
    Once,
    /// Re-runs across real ticks until it returns `Ok` or the attempt budget
    /// is exhausted (then the last `Err` fails the test). Use ONLY for
    /// side-effect-free assertions — a `poll` body re-executes, so it must
    /// not click/fill.
    Poll,
}

/// Define a synchronous test. The body uses `?` to bail on the first failed
/// action/assertion — same as `await expect(...)` throwing in Playwright.
pub fn test(
    name: &str,
    body: impl Fn(&Page) -> Result<(), String> + 'static,
) -> Test {
    Test {
        name: name.to_string(),
        body: Body::Sync(Box::new(body)),
    }
}

/// Begin a **flow** test — a sequence of steps advanced one per real
/// scheduler tick, so asynchronous (event-/timer-driven) UI changes settle
/// between steps. Chain [`Flow::act`] (run-once action/assertion) and
/// [`Flow::poll`] (assertion retried until it passes), then [`Flow::build`].
///
/// ```ignore
/// flow("push then pop")
///     .act(|p| p.get_by_test_id("push-detail").click())
///     .act(|p| expect(&p.get_by_test_id("detail-marker")).to_be_visible())
///     .act(|p| p.get_by_test_id("back").click())
///     // pop completes on async popstate — poll until the screen is gone:
///     .poll(|p| expect(&p.get_by_test_id("detail-marker")).not_to_be_visible())
///     .build()
/// ```
pub fn flow(name: &str) -> Flow {
    Flow {
        name: name.to_string(),
        steps: Vec::new(),
    }
}

/// Builder for a [`Body::Flow`] test — see [`flow`].
pub struct Flow {
    name: String,
    steps: Vec<Step>,
}

impl Flow {
    /// A run-once step: an action (`click`/`fill`/…) or a synchronous
    /// assertion. An `Err` fails the test immediately.
    pub fn act(mut self, body: impl Fn(&Page) -> Result<(), String> + 'static) -> Self {
        self.steps.push(Step {
            kind: StepKind::Once,
            run: Box::new(body),
        });
        self
    }

    /// A retried assertion: re-run across real ticks until it returns `Ok`
    /// or the attempt budget runs out. MUST be side-effect-free (no
    /// `click`/`fill` — the body re-executes).
    pub fn poll(mut self, body: impl Fn(&Page) -> Result<(), String> + 'static) -> Self {
        self.steps.push(Step {
            kind: StepKind::Poll,
            run: Box::new(body),
        });
        self
    }

    /// Finish the flow into a [`Test`] for a [`suite`].
    pub fn build(self) -> Test {
        Test {
            name: self.name,
            body: Body::Flow(self.steps),
        }
    }
}

/// A named group of tests — Playwright's `describe`.
pub struct Suite {
    name: String,
    tests: Vec<Test>,
}

/// Group tests under a suite name.
pub fn suite(name: &str, tests: Vec<Test>) -> Suite {
    Suite {
        name: name.to_string(),
        tests,
    }
}

struct Queued {
    suite: String,
    test: Test,
}

struct RunState {
    queue: Vec<Queued>,
    suite_count: usize,
    passed: std::cell::Cell<usize>,
    failed: std::cell::Cell<usize>,
    failures: std::cell::RefCell<Vec<String>>,
    /// Last suite name we logged a header for, so we print one header per
    /// suite as the flat queue crosses suite boundaries.
    last_suite: std::cell::RefCell<Option<String>>,
}

/// Run a single suite (back-compat convenience). Equivalent to
/// `run_suites(vec![suite(name, tests)])`.
pub fn run(name: &str, tests: Vec<Test>) {
    run_suites(vec![suite(name, tests)]);
}

/// Run several suites **sequentially**, paced so each test's UI changes are
/// visible on screen and its log lines stream one test at a time. Returns
/// immediately; the run advances on the framework scheduler and emits a
/// final `[E2E-RESULT]` machine-readable line when every suite is done.
///
/// Must be called on the UI thread, after the first render (so the registry
/// is populated) — see the `after_ms_detached` deferral in the app's entry.
pub fn run_suites(suites: Vec<Suite>) {
    let suite_count = suites.len();
    let mut queue = Vec::new();
    for s in suites {
        for t in s.tests {
            queue.push(Queued {
                suite: s.name.clone(),
                test: t,
            });
        }
    }
    log(format!(
        "▶ running {suite_count} suite(s), {} test(s) total",
        queue.len()
    ));
    let state = std::rc::Rc::new(RunState {
        queue,
        suite_count,
        passed: std::cell::Cell::new(0),
        failed: std::cell::Cell::new(0),
        failures: std::cell::RefCell::new(Vec::new()),
        last_suite: std::cell::RefCell::new(None),
    });
    run_one(state, 0);
}

/// Between-step delay for flow tests, ms. Any real-timer delay yields a
/// macrotask to the event loop, which is what lets async UI work (browser
/// `popstate` after a stack pop, a deferred layout pass) complete before the
/// next step's assertion. Kept small so flows stay snappy.
const FLOW_STEP_MS: i32 = 60;
/// Poll retry interval for `poll` steps, ms.
const POLL_INTERVAL_MS: i32 = 60;
/// Max `poll` attempts before the step's last `Err` fails the test
/// (≈ `POLL_INTERVAL_MS * POLL_MAX_ATTEMPTS` of real wall-clock headroom).
const POLL_MAX_ATTEMPTS: usize = 25;

fn run_one(state: std::rc::Rc<RunState>, i: usize) {
    if i >= state.queue.len() {
        emit_summary(&state);
        return;
    }

    let q = &state.queue[i];

    // Print a suite header when we cross into a new suite.
    {
        let mut last = state.last_suite.borrow_mut();
        if last.as_deref() != Some(q.suite.as_str()) {
            log(format!("▶ suite: {}", q.suite));
            *last = Some(q.suite.clone());
        }
    }

    log(format!(
        "• test ({}/{}): {}",
        i + 1,
        state.queue.len(),
        q.test.name
    ));

    match &q.test.body {
        Body::Sync(body) => {
            let page = Page::new();
            match body(&page) {
                Ok(()) => record_pass(&state, i),
                Err(e) => record_fail(&state, i, &e),
            }
            advance(state, i);
        }
        // Flow tests drive themselves across real ticks, then advance.
        Body::Flow(_) => drive_flow(state, i, 0, 0),
    }
}

/// Drive step `step_idx` of flow test `i` on its `attempt`-th try. Each
/// transition reschedules on the real scheduler (`after_ms_detached`), so the
/// event loop turns between steps — async UI work settles in the gap.
fn drive_flow(state: std::rc::Rc<RunState>, i: usize, step_idx: usize, attempt: usize) {
    let steps = match &state.queue[i].test.body {
        Body::Flow(s) => s,
        // Unreachable: `run_one` only routes `Body::Flow` here.
        Body::Sync(_) => {
            advance(state, i);
            return;
        }
    };

    if step_idx >= steps.len() {
        record_pass(&state, i);
        advance(state, i);
        return;
    }

    let step = &steps[step_idx];
    let page = Page::new();
    let res = (step.run)(&page);

    match (&step.kind, res) {
        // A passed step (action or assertion) → next step after a real tick.
        (_, Ok(())) => schedule_step(state, i, step_idx + 1, 0, FLOW_STEP_MS),
        // A once-step failure ends the test.
        (StepKind::Once, Err(e)) => {
            record_fail(&state, i, &e);
            advance(state, i);
        }
        // A poll-step failure retries until the budget is spent.
        (StepKind::Poll, Err(e)) => {
            if attempt + 1 < POLL_MAX_ATTEMPTS {
                schedule_step(state, i, step_idx, attempt + 1, POLL_INTERVAL_MS);
            } else {
                record_fail(&state, i, &format!("timed out waiting: {e}"));
                advance(state, i);
            }
        }
    }
}

fn schedule_step(
    state: std::rc::Rc<RunState>,
    i: usize,
    step_idx: usize,
    attempt: usize,
    delay_ms: i32,
) {
    runtime_core::after_ms_detached(delay_ms, move || drive_flow(state, i, step_idx, attempt));
}

fn record_pass(state: &RunState, i: usize) {
    state.passed.set(state.passed.get() + 1);
    log(format!("  ✅ PASS: {}", state.queue[i].test.name));
}

fn record_fail(state: &RunState, i: usize, e: &str) {
    let q = &state.queue[i];
    state.failed.set(state.failed.get() + 1);
    state
        .failures
        .borrow_mut()
        .push(format!("{}/{}: {}", q.suite, q.test.name, e));
    log_fail(format!("  ❌ FAIL: {} — {e}", q.test.name));
}

/// Advance to the next test on the scheduler. `after_ms_detached` (vs
/// `after_ms`) is deliberate: the handle of `after_ms` would cancel on drop
/// at the end of this function, killing the loop before the next test fires —
/// the classic scheduled-handle pitfall.
fn advance(state: std::rc::Rc<RunState>, i: usize) {
    runtime_core::after_ms_detached(STEP_PACING_MS, move || run_one(state, i + 1));
}

fn emit_summary(state: &RunState) {
    let (p, f) = (state.passed.get(), state.failed.get());
    let total = p + f;
    let verdict = if f == 0 { "✅ all green" } else { "❌ failures" };
    log(format!(
        "■ done — {verdict}: {p}/{total} passed across {} suite(s)",
        state.suite_count
    ));
    let failures = state.failures.borrow();
    for fail in failures.iter() {
        log_fail(format!("  • {fail}"));
    }
    // One machine-readable line for the cross-platform orchestrator to
    // scrape from each target's log channel.
    let failures_json = failures
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect::<Vec<_>>()
        .join(",");
    runtime_core::log_info!(
        "[E2E-RESULT] {{\"suites\":{},\"tests\":{},\"pass\":{},\"fail\":{},\"failures\":[{}]}}",
        state.suite_count,
        total,
        p,
        f,
        failures_json
    );
}

/// Minimal JSON string escaping — enough for failure messages, which are
/// developer-authored and never contain control bytes beyond newlines.
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ")
}
