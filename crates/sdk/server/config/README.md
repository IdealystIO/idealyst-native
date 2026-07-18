# `idealyst-config` — unified SDK configuration (server tier)

Structured configuration for the server-tier SDKs (`jobs`, `pubsub`, `email`),
replacing per-SDK flat `IDEALYST_<SDK>_*` env vars as the **default authoring
surface**. Built around **named connection profiles**: a credential/endpoint is
defined once and referenced by name, so two tools can share one account — or
not — which a flat env namespace can't express.

```toml
# idealyst.toml — the shared base
[connections.aws-main]        # define an AWS account ONCE
kind = "aws"
region = "us-east-1"
profile = "prod"

[connections.cache]
kind = "redis"
url = "redis://127.0.0.1:6379"

[jobs]
backend = "sqs"
connection = "aws-main"       # jobs + email share this account…
queue_url = "https://sqs.us-east-1.amazonaws.com/123/jobs"

[email]
provider = "ses"
connection = "aws-main"       # …same account

[pubsub]
backend = "redis"
connection = "cache"          # …but pubsub is separate infra
```

```rust
// One call at startup wires every enabled SDK from the files on disk.
idealyst_config::configure_all().await?;
```

## Why named connections

A flat `.env` gives each tool its own `IDEALYST_JOBS_URL`, `IDEALYST_EMAIL_*`,
etc. — there's no way to say "these two share this AWS account, that third one
doesn't." A named `[connections.<name>]` is defined once and referenced by
`connection = "<name>"`; sharing is `same name`, separation is `different name`.

## Files compose

Every file deserializes into the same `Config`; the loader merges them:

- **`idealyst.toml`** — the base (`[connections.*]` + `[jobs]`/`[pubsub]`/`[email]`).
- **`jobs.toml` / `pubsub.toml` / `email.toml`** (aka `mail.toml`) — per-tool
  overrides, merged after the base. Each may define its own connections.
- **`extends = "path"`** (a string or a list) — literal inheritance: the named
  parent file(s) merge first, then this file overlays. So `mail.toml` can
  `extends = "idealyst.toml"` to inherit its connections.
- **Environment variables** — still honored, but applied **last** as an
  override layer (secret injection), not the primary surface.

Merge is field-level (a per-tool file's set fields win; unset fields keep the
base's); connections union across files.

## Surface

- **`configure_all().await`** — load + merge from the current dir, then
  configure every enabled SDK. **`configure_from(&Config)`** does the same from
  an already-loaded `Config`.
- **`load()` / `load_from(dir)`** — the merged `Config` (loader only; no
  wiring). Used by the CLI to decide worker orchestration.
- **`Config::aws_for` / `url_for`** — resolve a `connection` reference (or
  inline `region`/`profile`/`url`) to an `AwsConnection` / URL string.
- **`aws_sdk_config(&AwsConnection)`** (feature `aws`) — build a resolved
  `aws_config::SdkConfig` shared by the SES (email) and SQS (jobs) wiring.

## Features

Wiring is opt-in per SDK, so an app pulls only what it uses.

| Feature      | Effect |
| ------------ | ------ |
| `jobs` / `pubsub` / `email` | Wire that SDK from its section (pulls the SDK crate). |
| `redis` / `postgres`        | Forward to the tool crates so a `connection`/`url` can select that broker. |
| `sqs`                        | Jobs over AWS SQS (implies `aws`). |
| `ses`                        | Email over AWS SES (implies `aws`). |
| `aws`                        | Build the resolved `SdkConfig` from an AWS connection. |

## Error behavior

`configure_all()` fails fast on **structural** mistakes, with an error naming
the section: a backend selected without its required `connection`/`url`/
`queue_url`, a `connection` that isn't defined or is the wrong kind, an unknown
backend name, or a backend whose cargo feature isn't compiled in
(`ConfigError::FeatureOff`). A section that is **absent** is skipped (the SDK
errors `NotConfigured` at first use). AWS **credentials/region** resolve lazily
through the AWS provider chain, so a missing credential surfaces on the first
`send`/`enqueue`, not at configure time.

## Server-tier only

Runs in the server / worker binary. Apps depend on it as an **optional** dep
enabled by their `server` feature, so the wasm client never compiles it. See
[`crates/sdk/server/email/examples/email-demo`](../email/examples/email-demo) (standalone) and the
`server`-gated bins in [`crates/sdk/server/jobs/examples/jobs-demo`](../jobs/examples/jobs-demo) /
[`crates/sdk/server/pubsub/examples/pubsub-demo`](../pubsub/examples/pubsub-demo).

## Testing checklist

`cargo test -p idealyst-config` runs the pure loader/merge suite (connection
resolution, share-vs-separate, per-tool overlay, `extends` inheritance, clear
resolution errors). The end-to-end wiring test runs under the SDK features.

```sh
cargo test -p idealyst-config
cargo test -p idealyst-config --features "email jobs pubsub"   # + configure_from wiring
cargo check -p idealyst-config --features "ses sqs"            # AWS paths compile
```

| Path | Tests | Verification |
| ---- | ----- | ------------ |
| loader / merge / `extends` / resolution | 🧪 unit | ✅ host-verified |
| `configure_from` (memory backends)      | 🧪 unit (under `email jobs pubsub`) | ✅ host-verified |
| `ses` / `sqs` AWS wiring                 | — (compiles under the features) | ⚠️ **compile-checked only** — no live AWS run |
