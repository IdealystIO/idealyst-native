# email-demo

Renders an idealyst **email template** to email-safe HTML and sends it through
the `email` SDK.

Two ideas in one small example:

1. **SSG-for-emails.** [`src/template.rs`](src/template.rs) is an ordinary
   idealyst `#[component]` — the same primitives and `ui!` macro your app uses —
   built from the `idea-ui-mail` component set and parameterized by input props
   (`name`, `cta_url`). `Email::template(...)` renders it through
   `backend-email` into a self-contained HTML document with **all styles inline
   and theme tokens resolved to literals** (no `<style>` classes, no
   `var(--…)`), which is what email clients need. No browser, no WASM.

2. **Pluggable sending.** The `email` SDK abstracts the provider behind
   `EmailProvider`. This demo defaults to the in-memory **capture** provider
   (records the message, never hits the network) and can switch to **AWS SES**
   via env / `dev.toml`.

## Run

```sh
# In-memory capture: prints the rendered HTML + plaintext and writes an
# HTML preview to your temp dir.
cargo run -p email-demo -- sam@example.com
```

Open the printed `idealyst-email-demo.html` path in a browser to preview the
rendered email.

## Send for real (AWS SES)

Configure the provider in [`idealyst.toml`](idealyst.toml) using a **named
connection profile** (uncomment the `[connections.aws-main]` + `[email]`
blocks), then:

```sh
cargo run -p email-demo --features ses -- sam@example.com
```

The non-memory path calls `idealyst_config::configure_all()`, which reads
`idealyst.toml` (+ an optional `mail.toml`), resolves the `connection`
reference, and installs the SES provider. Because the AWS account is a *named
connection*, the jobs SDK (SQS) can reference the same `aws-main` to share one
account, or a different one to stay separate — something a flat `.env` can't
express.

Credentials + region resolve through the standard AWS provider chain
(`profile`/`region` in the connection, then `AWS_*` env, shared config, or an
IAM role). The `from` address must be a verified SES identity. Env vars
(`IDEALYST_EMAIL_PROVIDER`/`IDEALYST_EMAIL_FROM`) still work as an override.
