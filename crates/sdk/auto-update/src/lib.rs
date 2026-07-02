//! # `auto-update` — desktop self-update for directly-distributed apps
//!
//! Keeps a *directly distributed* desktop app current — a macOS `.dmg`
//! (Developer ID), a Windows `.msix`, a Linux `.AppImage` — without the user
//! hunting down a new download. It does **not** apply to store builds: the Mac
//! App Store / Microsoft Store own updates for those, and Apple forbids an app
//! from updating itself. On iOS / Android / web this SDK is a deliberate no-op.
//!
//! ## Shape
//!
//! One reactive handle, [`Updater`], exposing a [`Signal<UpdateState>`] your UI
//! binds to. The lifecycle —
//!
//! ```text
//! Idle → Checking → UpToDate
//!                 ↘ Available → Downloading{progress} → ReadyToRelaunch
//!                 ↘ Failed
//!    (Store / web builds resolve straight to Unsupported)
//! ```
//!
//! — is authored once and renders identically on every backend; only the
//! platform *apply* step diverges (see below). A minimal banner:
//!
//! ```ignore
//! use auto_update::{Updater, UpdateConfig, UpdateState};
//!
//! // One process-global updater, checked at launch — the standardized hook:
//! let updater = auto_update::install(UpdateConfig::new(
//!     "https://releases.example.com/stable.json",
//!     "stable",
//!     keys::UPDATE_PUBLIC_KEY,               // baked in at build time
//!     env!("CARGO_PKG_VERSION"),
//!     42,
//! ));
//!
//! // In your render tree, react to `updater.state()` (or `auto_update::updater()`):
//! match updater.state().get() {
//!     UpdateState::Available(info) => { /* show "Update to {info.version}" */ }
//!     UpdateState::ReadyToRelaunch(_) => { /* show "Relaunch to finish" */ }
//!     _ => {}
//! }
//!
//! // Kick a check (e.g. on launch / on an interval / from a menu item):
//! # async fn go(updater: Updater) {
//! let _ = updater.check().await;
//! // Later, when the user accepts:
//! let _ = updater.download_and_install().await;
//! # }
//! ```
//!
//! ## Architecture
//!
//! - **Portable core** ([`manifest`]): the signed release-manifest format,
//!   platform/arch selection, version comparison, and Ed25519 signature
//!   verification. Pure Rust, fully unit-tested — the trust decisions live
//!   here, not in a platform backend.
//! - **Fetch** is cross-platform (via the `net` client), so it is *not* a
//!   per-platform seam.
//! - **Apply** *is* the seam — download, verify, swap the running app, relaunch
//!   — and each target supplies its own `imp`:
//!   - **macOS** — Sparkle.framework (atomic bundle swap + EdDSA + privileged
//!     relaunch), driven through objc2.
//!   - **Windows** — MSIX App Installer auto-update / a Squirrel-style installer.
//!   - **Linux** — AppImageUpdate (zsync delta) against the published image.
//!   - **iOS / web / other** — no-op, reporting [`InstallKind`] so the UI can
//!     hide itself.
//!
//! ## Security
//!
//! TLS is not the trust anchor. Every manifest entry is Ed25519-signed over
//! its `(version|build|url|sha256)` tuple against a key baked into the app at
//! build time, and the downloaded artifact's SHA-256 must match the signed
//! digest before anything is installed. A compromised CDN cannot ship a
//! release the app will accept.

#![deny(missing_docs)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use runtime_core::after_ms_detached;
use runtime_core::driver::spawn_async;
use runtime_core::Signal;

pub mod manifest;
mod error;

// Verified download of the artifact — shared by the native apply backends.
// Compiled only where an apply backend actually downloads (the three desktop
// targets); nowhere else references it, so gating here avoids dead code.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
mod download;

pub use error::UpdateError;
pub use manifest::{Arch, Platform, Release, ReleaseManifest};

// ---------------------------------------------------------------------------
// Backend selector. Exactly one compiles per target; each supplies an `imp`
// module with `install_kind()` and `async fn apply(&PreparedUpdate)`.
// ---------------------------------------------------------------------------

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(target_arch = "wasm32")))]
#[path = "apple.rs"]
mod imp;

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
#[path = "windows.rs"]
mod imp;

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
#[path = "linux.rs"]
mod imp;

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(not(any(
    target_arch = "wasm32",
    all(any(target_os = "macos", target_os = "ios"), not(target_arch = "wasm32")),
    all(target_os = "windows", not(target_arch = "wasm32")),
    all(target_os = "linux", not(target_arch = "wasm32")),
)))]
#[path = "stub.rs"]
mod imp;

// ---------------------------------------------------------------------------
// Public, platform-agnostic surface.
// ---------------------------------------------------------------------------

/// How this build is distributed — determines whether self-update is even
/// possible. Reported by the platform backend at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Directly distributed (Developer ID `.dmg`, `.msix`, `.AppImage`).
    /// Self-update applies.
    Direct,
    /// Installed from an app store — the store owns updates; self-update is
    /// forbidden (Apple) or unnecessary. The updater no-ops.
    Store,
    /// A web build — a page reload is the update. The updater no-ops.
    Web,
    /// The distribution channel couldn't be determined; treated like `Store`
    /// (no self-update) to fail safe.
    Unknown,
}

impl InstallKind {
    /// Whether self-update should run for this distribution.
    pub fn can_self_update(self) -> bool {
        matches!(self, InstallKind::Direct)
    }
}

/// Immutable configuration for an [`Updater`]. Cheap to build; the version /
/// build should come from the app's own compiled-in metadata.
#[derive(Clone)]
pub struct UpdateConfig {
    /// URL of the signed release manifest for this channel.
    pub manifest_url: String,
    /// The channel this app is subscribed to (`"stable"`, `"beta"`, …). Must
    /// match the manifest's `channel`.
    pub channel: String,
    /// The Ed25519 public key (32 bytes) that release signatures are verified
    /// against. Bake this into the app at build time; it must pair with the
    /// private key `idealyst publish` signs the manifest with.
    pub public_key: [u8; 32],
    /// The version this app is currently running (`CFBundleShortVersionString`
    /// on macOS) — typically `env!("CARGO_PKG_VERSION")`.
    pub current_version: String,
    /// The build number this app is currently running (`CFBundleVersion`).
    pub current_build: u64,
    /// When [`install`]ed, run a [`check`](Updater::check) once at startup.
    pub check_on_launch: bool,
    /// When [`install`]ed, re-check on this interval (in addition to
    /// `check_on_launch`). `None` disables periodic checks. Only armed on
    /// builds that can actually self-update.
    pub check_interval: Option<Duration>,
}

impl UpdateConfig {
    /// A config with `check_on_launch = true` and no periodic re-check — the
    /// common starting point. Fill in the remaining fields.
    pub fn new(
        manifest_url: impl Into<String>,
        channel: impl Into<String>,
        public_key: [u8; 32],
        current_version: impl Into<String>,
        current_build: u64,
    ) -> Self {
        Self {
            manifest_url: manifest_url.into(),
            channel: channel.into(),
            public_key,
            current_version: current_version.into(),
            current_build,
            check_on_launch: true,
            check_interval: None,
        }
    }

    /// Re-check on the given interval as well as at launch.
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.check_interval = Some(interval);
        self
    }
}

/// Details of an update the [`Updater`] found or is installing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    /// The version being offered / installed.
    pub version: String,
    /// Its build number.
    pub build: u64,
    /// Release-notes URL to surface, if the manifest supplied one.
    pub notes_url: Option<String>,
    /// Whether the manifest marked this update mandatory (forced).
    pub mandatory: bool,
}

/// The reactive state of the update lifecycle. Bind [`Updater::state`] and
/// match on this to render your UI. Cloneable so it can live in a [`Signal`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum UpdateState {
    /// Nothing has happened yet (initial state).
    Idle,
    /// A check is in flight.
    Checking,
    /// The running build is the latest — no update available.
    UpToDate,
    /// An update is available and verified, awaiting the user's go-ahead.
    Available(UpdateInfo),
    /// The update artifact is downloading; `progress` is `0.0..=1.0`, or `None`
    /// while the total size is unknown.
    Downloading {
        /// Which update is downloading.
        info: UpdateInfo,
        /// Fractional progress in `0.0..=1.0`, or `None` if indeterminate.
        progress: Option<f32>,
    },
    /// The update is staged and verified; relaunching applies it.
    ReadyToRelaunch(UpdateInfo),
    /// This build can't self-update (store / web / unsupported target). The UI
    /// should hide update affordances (or point at the store).
    Unsupported,
    /// The last operation failed; carries a human-readable reason. A
    /// subsequent [`check`](Updater::check) can recover from here.
    Failed(String),
}

/// A verified, ready-to-install update handed to the platform apply step.
/// Internal to the fetch→apply hand-off, but public so backends can consume it.
#[derive(Clone, Debug)]
pub struct PreparedUpdate {
    /// User-facing details.
    pub info: UpdateInfo,
    /// Where to download the artifact from.
    pub url: String,
    /// The signed lowercase-hex SHA-256 the download must match.
    pub sha256: String,
}

/// The update handle. Cheap to clone — it's a bundle of `Rc`s and a `Signal`
/// sharing one backing state, so every clone drives the same lifecycle.
#[derive(Clone)]
pub struct Updater {
    state: Signal<UpdateState>,
    config: Rc<UpdateConfig>,
    /// The verified update from the most recent successful [`check`], awaiting
    /// [`download_and_install`].
    pending: Rc<RefCell<Option<PreparedUpdate>>>,
}

impl Updater {
    /// Create an updater for the given configuration. Starts in
    /// [`UpdateState::Idle`]; nothing happens until you [`check`](Self::check).
    pub fn new(config: UpdateConfig) -> Self {
        Self {
            state: Signal::new(UpdateState::Idle),
            config: Rc::new(config),
            pending: Rc::new(RefCell::new(None)),
        }
    }

    /// The reactive state signal — bind this in your render tree.
    pub fn state(&self) -> Signal<UpdateState> {
        self.state
    }

    /// How this build is distributed. When this isn't [`InstallKind::Direct`],
    /// [`check`](Self::check) resolves straight to [`UpdateState::Unsupported`].
    pub fn install_kind() -> InstallKind {
        imp::install_kind()
    }

    /// Check the release manifest for a newer, verified build.
    ///
    /// Transitions the [`state`](Self::state) to [`UpdateState::Checking`] and
    /// then to [`Available`](UpdateState::Available), [`UpToDate`](UpdateState::UpToDate),
    /// [`Unsupported`](UpdateState::Unsupported), or [`Failed`](UpdateState::Failed).
    /// Returns `Ok(())` for every *expected* outcome (including "up to date"
    /// and "unsupported"); only a genuine fetch/parse/verify failure is `Err`.
    pub async fn check(&self) -> Result<(), UpdateError> {
        // Store / web / unsupported target: nothing to do but say so.
        if !imp::install_kind().can_self_update() {
            self.state.set(UpdateState::Unsupported);
            return Ok(());
        }
        let Some(platform) = Platform::current() else {
            self.state.set(UpdateState::Unsupported);
            return Ok(());
        };

        self.state.set(UpdateState::Checking);

        let manifest = match self.fetch_manifest().await {
            Ok(m) => m,
            Err(e) => {
                self.state.set(UpdateState::Failed(e.to_string()));
                return Err(e);
            }
        };

        let Some(release) = manifest.select(platform, Arch::current()) else {
            self.state.set(UpdateState::UpToDate);
            return Ok(());
        };

        // Signature is the gate: refuse anything we can't cryptographically
        // attribute to the publisher, regardless of how it reached us.
        if let Err(e) = release.verify(&self.config.public_key) {
            let err = UpdateError::from(e);
            self.state.set(UpdateState::Failed(err.to_string()));
            return Err(err);
        }

        let newer = release.is_newer_than(&self.config.current_version, self.config.current_build);
        if !newer {
            self.pending.borrow_mut().take();
            self.state.set(UpdateState::UpToDate);
            return Ok(());
        }

        let info = UpdateInfo {
            version: release.version.clone(),
            build: release.build,
            notes_url: release.notes_url.clone(),
            mandatory: release.mandatory,
        };
        *self.pending.borrow_mut() = Some(PreparedUpdate {
            info: info.clone(),
            url: release.url.clone(),
            sha256: release.sha256.clone(),
        });
        self.state.set(UpdateState::Available(info));
        Ok(())
    }

    /// Download the pending update, verify its digest, and hand it to the
    /// platform installer (which stages it and relaunches). Call after a
    /// [`check`](Self::check) has surfaced [`UpdateState::Available`].
    ///
    /// On success the app is either relaunched by the backend or left in
    /// [`UpdateState::ReadyToRelaunch`] for the user to trigger. Returns
    /// [`UpdateError::NothingToInstall`] if no update is pending.
    pub async fn download_and_install(&self) -> Result<(), UpdateError> {
        let prepared = self
            .pending
            .borrow()
            .clone()
            .ok_or(UpdateError::NothingToInstall)?;

        self.state.set(UpdateState::Downloading {
            info: prepared.info.clone(),
            progress: None,
        });

        match imp::apply(&prepared).await {
            Ok(()) => {
                self.state.set(UpdateState::ReadyToRelaunch(prepared.info));
                Ok(())
            }
            Err(e) => {
                self.state.set(UpdateState::Failed(e.to_string()));
                Err(e)
            }
        }
    }

    /// Kick a [`check`](Self::check) in the background (fire-and-forget) and
    /// let it drive [`state`](Self::state). Convenient for a "Check for
    /// Updates" menu item or a launch hook where you don't want to await.
    pub fn check_now(&self) {
        let this = self.clone();
        spawn_async(async move {
            let _ = this.check().await;
        });
    }

    /// Apply the staged update and relaunch into the new version. Valid once
    /// [`state`](Self::state) is [`UpdateState::ReadyToRelaunch`]. On the
    /// desktop backends this **replaces the app and exits the process** (the
    /// platform swapper reopens it), so it does not return on success; it
    /// returns [`UpdateError`] only if there's nothing staged or the target
    /// can't self-update.
    pub fn relaunch(&self) -> Result<(), UpdateError> {
        imp::relaunch()
    }

    /// Fetch + parse the manifest. Cross-platform (via `net`), so it lives in
    /// the portable layer rather than the `imp` seam.
    async fn fetch_manifest(&self) -> Result<ReleaseManifest, UpdateError> {
        let body = net::Client::new()
            .get(&self.config.manifest_url)
            .send()
            .await
            .map_err(|e| UpdateError::Fetch(e.to_string()))?
            .text()
            .await
            .map_err(|e| UpdateError::Fetch(e.to_string()))?;
        Ok(ReleaseManifest::parse(body.as_bytes())?)
    }
}

// ---------------------------------------------------------------------------
// Process-global hook — the standardized way an app opts in.
//
// Most apps want one updater for the whole process, reachable from anywhere in
// the UI. `install` builds it, stores it, and (per config) checks at launch and
// on an interval; `updater()` hands it back to any render site that binds the
// state.
// ---------------------------------------------------------------------------

thread_local! {
    static INSTALLED: RefCell<Option<Updater>> = const { RefCell::new(None) };
}

/// Install the process-global [`Updater`] from `config`. Call once during app
/// startup. Returns the handle (also retrievable anywhere via [`updater`]).
///
/// If the build can self-update (direct distribution), this honors
/// [`UpdateConfig::check_on_launch`] and [`UpdateConfig::check_interval`];
/// on store / web / unsupported builds it installs an inert updater that
/// resolves to [`UpdateState::Unsupported`] and schedules nothing.
///
/// ```ignore
/// use auto_update::{install, UpdateConfig};
/// use std::time::Duration;
///
/// let updater = install(
///     UpdateConfig::new(
///         "https://releases.example.com/stable.json",
///         "stable",
///         keys::UPDATE_PUBLIC_KEY,           // baked in at build time
///         env!("CARGO_PKG_VERSION"),
///         42,
///     )
///     .with_interval(Duration::from_secs(6 * 60 * 60)), // re-check every 6h
/// );
/// // Bind `updater.state()` in your UI; call `updater.relaunch()` when ready.
/// ```
pub fn install(config: UpdateConfig) -> Updater {
    let check_on_launch = config.check_on_launch;
    let interval = config.check_interval;

    let updater = Updater::new(config);
    INSTALLED.with(|c| *c.borrow_mut() = Some(updater.clone()));

    // Only schedule work where it can actually lead somewhere — a store / web
    // build would just re-derive `Unsupported` on a timer.
    if Updater::install_kind().can_self_update() {
        if check_on_launch {
            updater.check_now();
        }
        if let Some(interval) = interval {
            arm_interval(updater.clone(), interval);
        }
    }
    updater
}

/// The process-global [`Updater`] installed by [`install`], or `None` if
/// [`install`] hasn't run yet.
pub fn updater() -> Option<Updater> {
    INSTALLED.with(|c| c.borrow().clone())
}

/// Schedule one check after `interval`, then re-arm — a self-perpetuating
/// timer built on the framework scheduler (`after_ms_detached`), matching the
/// `sync` SDK's polling pattern. No thread, no `mem::forget`.
fn arm_interval(updater: Updater, interval: Duration) {
    let ms = interval.as_millis().min(i32::MAX as u128) as i32;
    after_ms_detached(ms, move || {
        updater.check_now();
        arm_interval(updater.clone(), interval);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_kind_gates_self_update() {
        assert!(InstallKind::Direct.can_self_update());
        assert!(!InstallKind::Store.can_self_update());
        assert!(!InstallKind::Web.can_self_update());
        assert!(!InstallKind::Unknown.can_self_update());
    }

    #[test]
    fn updater_starts_idle() {
        let updater = Updater::new(UpdateConfig::new(
            "https://example.com/stable.json",
            "stable",
            [0u8; 32],
            "1.0.0",
            1,
        ));
        assert_eq!(updater.state().get(), UpdateState::Idle);
    }

    #[test]
    fn install_before_check_is_nothing_to_install() {
        let updater = Updater::new(UpdateConfig::new(
            "https://example.com/stable.json",
            "stable",
            [0u8; 32],
            "1.0.0",
            1,
        ));
        // Drive the future to completion synchronously — it never awaits before
        // the early return.
        let fut = updater.download_and_install();
        let result = futures_lite_block(fut);
        assert!(matches!(result, Err(UpdateError::NothingToInstall)));
    }

    /// Minimal executor for a future that completes without ever yielding
    /// `Pending` (true for the early-return path under test). Avoids pulling a
    /// runtime dependency into a unit test.
    fn futures_lite_block<F: std::future::Future>(mut fut: F) -> F::Output {
        use std::pin::Pin;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        // Safety: `fut` is owned and not moved after pinning.
        let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => panic!("future yielded Pending in a test that expects immediate completion"),
            }
        }
    }
}
