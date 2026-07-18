# `email` — transactional email SDK (server tier)

Send email with a pluggable provider, and render the message body from an
**idealyst template** — the same primitives and `ui!` your app uses, rendered
to email-safe HTML with no browser and no WASM ("SSG for emails").

```rust
use email::{Email, Mailbox};

#[server]
async fn welcome(addr: String) -> Result<(), ServerError> {
    email::send(
        Email::to(addr)
            .subject("Welcome!")
            .template(|| WelcomeEmail(&WelcomeEmailProps { name: "Sam".into() })),
    )
    .await
    .map_err(ServerError::failed)?;
    Ok(())
}
```

`.template(...)` renders the component through
[`backend-email`](../../../backend/email): styles are inlined on every node,
theme tokens are resolved to literal values (email clients have no CSS
variables), and interaction / `@media` overlays are dropped. It also derives a
`text/plain` alternative and, if the template set a page-metadata title, the
subject.

## Surface

- **`Email`** — the message + a fluent builder: `Email::to(addr)`, then
  `.from` / `.cc` / `.bcc` / `.reply_to` / `.subject`, and a body via
  `.html(str)` / `.text(str)` or `.template(|| Component())` /
  `.template_with(setup, || …)`.
- **`Mailbox`** — one address, optionally named. `From<&str>` / `From<String>`
  / `From<(name, addr)>`; `encoded()` renders `Name <addr>`.
- **`send(Email)`** — validates (recipient present, sender resolvable, a body
  part exists) and dispatches through the configured provider.
- **`configure(provider)`** installs the process-wide provider;
  **`configure_from_env()`** reads `IDEALYST_EMAIL_PROVIDER` /
  `IDEALYST_EMAIL_FROM`. For the file-based, connection-profile surface use
  [`idealyst-config`](../config)'s `configure_all()`.
- **`EmailProvider`** trait — implement your own delivery.
- Re-exports the rendering surface (`render_email` / `RenderedEmail` /
  `EmailBackend`) from `backend-email`.

## Providers (feature-gated)

| Feature   | Provider         | Delivery |
| --------- | ---------------- | -------- |
| `memory`  | `MemoryProvider` | In-process **capture** — records every send, never hits the network (default). The dev + test substrate: inspect `.sent()`, preview the HTML. |
| `ses`     | `SesProvider`    | AWS SESv2 (`SendEmail`). Credentials + region via the standard AWS provider chain; `SesProvider::from_aws(&SdkConfig)` takes a resolved config so [`idealyst-config`](../config) can share one AWS account with the `jobs` SDK's SQS. |

## Email-safe rendering

The template is rendered by a dedicated backend, not the web backend, because
email clients break the browser's assumptions:

- **Gmail strips `<style>`/`<head>` CSS** → every style is inline on its node.
- **No CSS custom properties** → theme tokens are resolved to literals at
  render time (never `var(--…)`).
- **No interaction, unreliable `@media`** → hover/press/focus, breakpoint, and
  container overlays are dropped; only the resolved base style ships.

The backend stays layout-neutral (`view`→`div`, `text`→`span`, `link`→`a`).
Email-safe, opinionated building blocks (centered column, sections, CTA button,
divider) live in [`idea-ui-mail`](../../../ui/idea-ui-mail), the same way
`idea-ui` is the opinion layer over the web backend.

## Server-tier only

`email` runs in the server binary. Apps depend on it as an **optional** dep
enabled by their `server` feature, so the wasm client never compiles it. See
[`examples/email-demo`](./examples/email-demo).

## Testing checklist

`cargo test -p email` runs the memory-provider suite (send/capture, validation,
mailbox encoding, template render + subject derivation).
`cargo test -p backend-email` covers the renderer (inline styles, token
resolution, overlay dropping, document wrapping).

| Provider  | Tests | Verification |
| --------- | ----- | ------------ |
| `memory`  | 🧪 unit (send/capture, validation, template render) | ✅ host-verified; `crates/sdk/server/email/examples/email-demo` writes an HTML preview |
| `ses`     | — (compiles under `--features ses`) | ⚠️ **compile-checked only** — no live AWS run |

```sh
cargo test -p email
cargo check -p email --features ses
cargo run  -p email-demo -- you@example.com      # memory: renders + previews
```
