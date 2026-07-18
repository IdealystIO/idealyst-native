//! A tiny **Playwright-flavoured E2E harness** built on the framework's
//! in-process Robot API.
//!
//! This is the seed of what a real cross-platform E2E framework would be:
//! the same `locate → act → assert` vocabulary Playwright gives you for
//! the web, but driving *our* introspection registry instead of a
//! browser — so the identical suite runs on web, iOS, Android, macOS, and
//! the terminal with no per-platform runner.
//!
//! Mapping to the underlying [`runtime_core::robot`] surface:
//!
//! | Playwright            | here                          | Robot call          |
//! |-----------------------|-------------------------------|---------------------|
//! | `page.getByTestId`    | [`Page::get_by_test_id`]      | `find(TestId)`      |
//! | `page.getByText`      | [`Page::get_by_text`]         | `find(LabelContains)`|
//! | `locator.click()`     | [`Locator::click`]            | `Robot::click`      |
//! | `locator.fill()`      | [`Locator::fill`]             | `Robot::type_text`  |
//! | `expect(l).toBeVisible()` | [`Expect::to_be_visible`] | `find().is_some()`  |
//! | `expect(l).toHaveText()`  | [`Expect::to_have_text`]  | live `label`        |
//!
//! Every action and assertion logs a line to the platform console via
//! [`runtime_core::log_info`] (`console.*` on web, `__android_log_print`
//! on Android, `NSLog`/stderr on Apple, stderr in the terminal). Run the
//! demo and watch the suite narrate itself — that's the "see what the E2E
//! is doing" part.
//!
//! Assertions read the **live** registry label ([`Robot`] recomputes
//! reactive text on read), so a `toHaveText` immediately after a `click`
//! sees the post-update value with no explicit wait.

use runtime_core::robot::{Element, ElementKind, Query, Robot};

/// Pacing between tests, in ms. Purely cosmetic: it lets the on-screen UI
/// visibly change and the console stream one test at a time, like watching
/// a real test run. Actions within a test are synchronous.
const STEP_PACING_MS: i32 = 750;

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

    /// Click / press the element. `Result::Err` if it isn't present.
    pub fn click(&self) -> Result<(), String> {
        log(format!("  ▸ {}.click()", self.desc));
        let el = self.resolve()?;
        Robot::new()
            .click(&el)
            .map_err(|e| format!("{}.click() failed: {e}", self.desc))
    }

    /// Type text into a text input (clears + sets, like Playwright `fill`).
    pub fn fill(&self, text: &str) -> Result<(), String> {
        log(format!("  ▸ {}.fill({text:?})", self.desc));
        let el = self.resolve()?;
        Robot::new()
            .type_text(&el, text)
            .map_err(|e| format!("{}.fill() failed: {e}", self.desc))
    }

    /// Number of elements this locator currently matches.
    fn count(&self) -> usize {
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
}

// ---------------------------------------------------------------------------
// Test + runner
// ---------------------------------------------------------------------------

/// One named test case — Playwright's `test('name', async ({page}) => …)`.
pub struct Test {
    name: String,
    body: Box<dyn Fn(&Page) -> Result<(), String>>,
}

/// Define a test. The body uses `?` to bail on the first failed
/// action/assertion — same as `await expect(...)` throwing in Playwright.
pub fn test(
    name: &str,
    body: impl Fn(&Page) -> Result<(), String> + 'static,
) -> Test {
    Test {
        name: name.to_string(),
        body: Box::new(body),
    }
}

struct RunState {
    suite: String,
    tests: Vec<Test>,
    passed: std::cell::Cell<usize>,
    failed: std::cell::Cell<usize>,
}

/// Run a suite, **paced** so each test's UI changes are visible on screen
/// and its log lines stream one test at a time. Returns immediately; the
/// run advances on the framework scheduler.
///
/// Must be called on the UI thread, after the first render (so the
/// registry is populated) — see the `after_ms` deferral in `lib.rs`.
pub fn run(suite: &str, tests: Vec<Test>) {
    log(format!("▶ suite: {suite}  ({} tests)", tests.len()));
    let state = std::rc::Rc::new(RunState {
        suite: suite.to_string(),
        tests,
        passed: std::cell::Cell::new(0),
        failed: std::cell::Cell::new(0),
    });
    run_one(state, 0);
}

fn run_one(state: std::rc::Rc<RunState>, i: usize) {
    if i >= state.tests.len() {
        let (p, f) = (state.passed.get(), state.failed.get());
        let verdict = if f == 0 { "✅ all green" } else { "❌ failures" };
        log(format!(
            "■ suite: {} — {verdict}: {p} passed, {f} failed",
            state.suite
        ));
        return;
    }

    let t = &state.tests[i];
    log(format!("• test ({}/{}): {}", i + 1, state.tests.len(), t.name));
    let page = Page::new();
    match (t.body)(&page) {
        Ok(()) => {
            state.passed.set(state.passed.get() + 1);
            log(format!("  ✅ PASS: {}", t.name));
        }
        Err(e) => {
            state.failed.set(state.failed.get() + 1);
            log_fail(format!("  ❌ FAIL: {} — {e}", t.name));
        }
    }

    // Advance to the next test on the scheduler. `after_ms_detached`
    // (vs `after_ms`) is deliberate: the returned handle of `after_ms`
    // would cancel on drop at the end of this function, killing the loop
    // before the next test fires — the classic scheduled-handle pitfall.
    let next = state.clone();
    runtime_core::after_ms_detached(STEP_PACING_MS, move || run_one(next, i + 1));
}
