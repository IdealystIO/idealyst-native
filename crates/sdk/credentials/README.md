# `credentials`

Secure storage for **secrets** — auth tokens, API keys, anything that must
not be readable by other code, other users, or a casual disk/backup
inspection. This is the `SecureStore` half of the split; the plaintext
[`storage`](../storage) crate is the `AsyncStorage` half. Put preferences
in `storage`; put secrets here.

Its defining principle: **it is backed only by each platform's real secure
facility, and it refuses — loudly — where real security isn't achievable,
rather than faking it.**

| Platform | Backend | Secure? | Verified |
| --- | --- | --- | --- |
| iOS / macOS | Keychain (Security framework) | yes — OS / Secure-Enclave protected | ✅ host-tested (macOS) |
| Android | AES-256-GCM keyed by an AndroidKeyStore key (TEE/StrongBox) | yes | ⚠️ compile-checked, **device-unverified** |
| Windows | Credential Manager (via `keyring`) | yes — OS vault | ⚠️ compile-checked |
| Linux | Secret Service — GNOME Keyring / KWallet (via `keyring`) | yes — OS vault | ⚠️ compile-checked |
| web | **errors** — see below | n/a | n/a |

Desktop uses the OS credential vault rather than a hardware enclave: there's no
browser-XSS surface on a desktop, so the vault (user-session-locked, encrypted
at rest) is the right bar. On Linux it needs a running Secret Service daemon (a
desktop login session); on a headless box with none, operations return
`CredError::Backend` — no secure store to use, and we don't pretend otherwise.

```rust
use credentials::{platform_credentials, CredError};

let creds = platform_credentials("myapp");       // Arc<dyn Credentials>
creds.set("token", "eyJhbGci…")?;                // → Keychain / Keystore
assert_eq!(creds.get("token")?, Some("eyJhbGci…".to_string()));
creds.remove("token")?;
```

The trait is **synchronous** on purpose: every real keystore is, and a
sync `get` drops straight into the server-fn bearer source (below).

## Why web errors, and what to do instead

A browser has **no secure client-side store.** Anything your code can read,
any script on your origin can read — so an XSS gets it, no matter how it's
"encrypted." Calling browser storage "secure" is the exact false pretense
this crate exists to avoid, so on web every operation returns
[`CredError::Unsupported`] with guidance.

The correct pattern for web secrets is **server-side, via an httpOnly
cookie** (the BFF pattern): a server function validates the login and sets
a cookie the browser sends automatically but JS can never read — so the
secret never enters the client at all.

```rust
// Server fn (runs server-side). `set_cookie` is from the `server` SDK.
#[server]
pub async fn login(email: String, password: String) -> Result<(), ServerError> {
    let session_id = authenticate(&email, &password).await?;   // your logic
    server::set_cookie(server::Cookie::new("session", session_id));  // httpOnly+Secure+SameSite=Lax by default
    Ok(())
}

// Logout clears it:
#[server]
pub async fn logout() -> Result<(), ServerError> {
    server::clear_cookie("session");
    Ok(())
}
```

A guard middleware reads the cookie on later requests (`Cookies` extractor)
and injects the principal for `Auth<Principal>` handlers. The browser sends
the cookie automatically.

**Deployment note:** this works out of the box when the server serves both
the app and `/_srv/*` from **one origin** (the standard BFF shape) — fetch's
default `same-origin` credentials mode stores and sends the cookie. A
*cross-origin* API additionally needs `credentials: 'include'` on the fetch
plus `Access-Control-Allow-Credentials` CORS on the server; that cross-origin
mode is a deliberate opt-in, not the default (forcing it would leak cookies
cross-site).

## One auth glue, every platform

A credential here *is* the server-fn auth token, and because `get` is sync
it plugs directly into the bearer source. The **same code works
everywhere**:

```rust
let creds = credentials::platform_credentials("myapp");
server::configure(
    server::ClientConfig::new("https://api.example.com")
        .with_credentials(server::bearer({
            let creds = creds.clone();
            move || creds.get("token").ok().flatten()
        })),
);
```

- **Native**: reads the token from Keychain/Keystore and sends
  `Authorization: Bearer …`.
- **Web**: `get` errors → `None` → no bearer header is sent, which is
  correct — the httpOnly session cookie carries auth there instead.

So you write the auth wiring once; each platform uses its right mechanism.

## Threat model (be precise, don't over-trust)

- **iOS/macOS Keychain, Android Keystore**: secrets are OS-protected at
  rest, often hardware-backed (Secure Enclave / TEE / StrongBox). Reading
  your *own* secret into app memory is the intended, secure way to use it —
  the OS gates access to the owning app. This protects against device theft,
  other apps, backups, and disk inspection. It does **not** protect against
  a compromised/rooted device running as your app.
- **web**: no secure client storage exists; secrets stay server-side
  (above). The httpOnly cookie protects the session secret from JS/XSS
  *reading* it, but an XSS can still *make authenticated requests* while the
  page is open — defense-in-depth (short sessions, CSRF protection,
  `SameSite`) still matters.
- **Windows/Linux**: the OS credential vault (Credential Manager / Secret
  Service). Encrypted at rest and unlocked with the user's login session — no
  hardware enclave, but there's no browser-XSS surface on a desktop, so the
  vault is the right bar. It does protect against other users and casual disk
  inspection; it does not protect against malware running as your user.

## Verification status

- **Apple Keychain** — round-trip tested live against the host macOS
  Keychain (`cargo test -p credentials --lib -- --ignored`): set, get,
  rotate, remove, idempotent-remove.
- **Android Keystore** — compiles for `aarch64-linux-android` but is **not
  device-verified** here (JNI method signatures resolve at runtime). Every
  failure returns [`CredError::Backend`] with the JNI message to make an
  on-device diagnosis quick. Test on a device before relying on it.
- **Windows / Linux** — the `keyring`-backed vault path compiles for
  `x86_64-pc-windows-gnu` and `x86_64-unknown-linux-gnu` (Linux uses keyring's
  async/`zbus` Secret Service, so no `libdbus` C dependency). Not run here
  (the host is macOS, which uses the Keychain backend) — run a quick
  set/get/remove on a real Windows/Linux desktop to confirm.
- **Web** — the error-with-guidance path is unit-tested.
- **Server `set_cookie`** — an end-to-end test boots the real router and
  asserts a handler's `set_cookie` surfaces a `Set-Cookie:
  session=…; HttpOnly; Secure; SameSite=Lax` response header.

[`CredError::Unsupported`]: src/lib.rs
[`CredError::Backend`]: src/lib.rs
