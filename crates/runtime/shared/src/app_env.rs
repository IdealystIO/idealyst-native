//! Variables the dev tooling hands the app (`idealyst dev --app-env K=V`,
//! `idealyst test --env K=V`), read the same way on every platform.
//!
//! Native apps receive them as process environment: desktop apps inherit
//! it, the iOS simulator gets them as `SIMCTL_CHILD_*`, and the Android
//! bridge sets them from the `IdealystEnv` manifest metadata before the app
//! starts. A browser page has no environment, so the dev server injects a
//! `window.IDEALYST_APP_ENV` object and the web boot installs a reader for
//! it with [`set_app_env_source`] before mounting.

runtime_tls::thread_local! {
    static SOURCE: std::cell::Cell<Option<fn(&str) -> Option<String>>> =
        const { std::cell::Cell::new(None) };
}

/// The value the dev tooling passed the app for `key`, if any.
///
/// Reads the installed source (web: the page's `IDEALYST_APP_ENV`), then
/// the process environment (native). `None` in a production build that
/// nobody passed the variable to.
pub fn app_env(key: &str) -> Option<String> {
    if let Some(read) = SOURCE.with(|s| s.get()) {
        if let Some(v) = read(key) {
            return Some(v);
        }
    }
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    std::env::var(key).ok()
}

/// Install where [`app_env`] looks before the process environment. Called
/// by a platform boot whose app has no environment (web).
pub fn set_app_env_source(read: fn(&str) -> Option<String>) {
    SOURCE.with(|s| s.set(Some(read)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(key: &str) -> Option<String> {
        (key == "FROM_PAGE").then(|| "page".to_string())
    }

    #[test]
    fn reads_the_process_environment_natively() {
        std::env::set_var("IDEALYST_APP_ENV_TEST_KEY", "native");
        assert_eq!(app_env("IDEALYST_APP_ENV_TEST_KEY").as_deref(), Some("native"));
        assert_eq!(app_env("IDEALYST_APP_ENV_TEST_MISSING"), None);
    }

    #[test]
    fn an_installed_source_wins_and_falls_through_to_the_environment() {
        std::env::set_var("IDEALYST_APP_ENV_TEST_FALLBACK", "env");
        set_app_env_source(page);
        assert_eq!(app_env("FROM_PAGE").as_deref(), Some("page"));
        assert_eq!(app_env("IDEALYST_APP_ENV_TEST_FALLBACK").as_deref(), Some("env"));
    }
}
