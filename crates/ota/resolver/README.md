# OTA resolver

A small service that answers an app's update check for its build: what each
bundle should run. Apps use it when their settings name it:

```toml
[package.metadata.idealyst.ota]
resolver = "https://ota-api.example.com"
```

It is optional. Without it, an app reads its build's precomputed answer from
the release location, or decides from the index itself (see `docs/ota.md`,
"Answers per app build"). Apps that can't reach the resolver fall back to
those, so if it's down, users still get updates.

## What it answers

`POST /v1/resolve`, with `{"manifest": "<id>"}`:

| Reply | When |
|---|---|
| `200`, the answer (`ota_index::Resolution`) | the location knows this build: registered from its build (`idealyst ota manifest`), or reported before |
| `404` | it doesn't: the app sends `{"manifest": "<id>", "provides": {…}}` once |
| `400` | the id isn't one (64 lowercase hex characters, what `Provides::id()` gives), or the manifest sent isn't the one its id names. A malformed id is refused before anything is read |
| `413` | a body over 1 MB |
| `502` | the location couldn't be read |

A manifest sent this way is answered and stored as **reported from the
field**. The resolver writes `manifests/<id>.json` and a marker,
`reported/<id>.json`, so the next request needs only the id, and the release
console lists the build. Each report writes files of its own. Many instances
reporting at once, such as a new app release reaching many installs, never
wait on or overwrite each other.

The answer is `ota_index::resolve`, the same decision the app makes from the
index. The app still checks every bundle's signature and requirements before
running it. The resolver can delay an update, but can't make an app run a
bundle it shouldn't. It changes nothing at the location except the reports,
which are capped.

`GET /health` answers `ok`.

## Run it

```sh
export OTA_LOCATION=s3://my-app-ota/my-app      # the bucket `idealyst ota publish` writes
export AWS_ACCESS_KEY_ID=… AWS_SECRET_ACCESS_KEY=… AWS_REGION=us-east-1
cargo run -p ota-resolver --release             # → http://127.0.0.1:3200/v1/resolve
```

| Variable | Default | |
|---|---|---|
| `OTA_LOCATION` | — | `s3://bucket/prefix`, or a directory |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` | — | Read the location; write `manifests/` and `reported/` |
| `AWS_REGION` / `AWS_DEFAULT_REGION` | `us-east-1` | |
| `AWS_ENDPOINT_URL` | AWS for the region | MinIO, R2, … |
| `OTA_RESOLVE_REPORTS` | `keep` | `ignore`: answer manifests apps send, but don't store them. The app then sends its manifest again on every check |
| `OTA_RESOLVE_MAX_REPORTED` | `500` | How many reported builds to store |
| `OTA_RESOLVE_INDEX_TTL_SECS` | `5` | How long an instance reuses the index: a publish or take-down reaches apps asking here at most this much later |
| `PORT` | `3200` | The server binary |
| `OTA_RESOLVE_BIND` | `127.0.0.1` | `0.0.0.0` in a container |

The keys need `s3:GetObject` and `s3:ListBucket` on the location, and
`s3:PutObject` on `manifests/*` and `reported/*` to store reports.

## On AWS Lambda

The same router runs as a Lambda (`src/bin/lambda.rs`, via `server-aws`):

```sh
cargo lambda build -p ota-resolver --features lambda --bin lambda --release --arm64
```

Put it behind a Function URL or API Gateway, set `OTA_LOCATION`, and give the
function's role the bucket permissions above. Lambda supplies the
credentials. An instance keeps the manifests it has read, and the index for
`OTA_RESOLVE_INDEX_TTL_SECS`, across warm invocations. A cold instance reads
them again.

An instance only keeps manifests the location stores: registered builds, and
reports it stored. A manifest it answered but didn't store (reports ignored,
or past `OTA_RESOLVE_MAX_REPORTED`) is forgotten, so clients posting many
manifests can't grow its memory.

## Locally, with the demo's MinIO

```sh
. crates/ota/demo/minio.env
OTA_LOCATION=s3://ota-demo/demo cargo run -p ota-resolver
IDEALYST_OTA_RESOLVER=http://127.0.0.1:3200 cargo run -p ota-demo
```

The demo window then says it was "answered by the service".
