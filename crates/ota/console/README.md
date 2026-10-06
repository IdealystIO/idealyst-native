# OTA console

A self-hosted web page for managing a release location: the bucket that
`idealyst ota publish` writes to and apps read from.

- **See** each bundle's releases, in the order apps choose them (the first is
  live), with when they were published, their size and signer, the remote
  components they provide, and what each newly requires.
- **Take one down**: any release, not just the newest. Apps go to the
  newest remaining release they can run, when they'd apply any update.
- **Kill switch**: take it down *urgently*. Apps running it replace it at
  once, even apps set to apply updates at the next launch. If no other
  release is left, they go back to their built-in copy, or stop running the
  bundle.
- **Restore** a taken-down release. **Pin** a release ahead of newer ones,
  including ones published later; **unpin** to serve the newest again.
- **App builds**: every registered build of the app (`idealyst ota manifest`
  registers one; builds that report themselves to the
  [resolver](../resolver/README.md) appear too, marked "seen in the field"), what each runs, and why it can't run anything newer.
  Before a take-down, the dialog names each build running that release and
  where it goes, or that it's left with nothing.
- **Compatibility grid**, under each bundle: every registered build against
  every live release. A cell is marked "runs" for the release the build
  chooses, ✓ for one it could run, and ✗ for one it can't. Hover over a
  cell, or tap it, for the reason: the missing prop, the changed
  host-function signature, the struct that now crosses differently. The
  grid comes from the manifests the builds generated and the requirements
  the releases recorded, so nothing is maintained by hand.
- **Audit log**: every action, and every publish from the CLI.

The page edits `index.json` and `audit.json` in the bucket, and every change
rewrites the registered builds' answers (`resolved/<id>.json`). Apps never
talk to the console: they read the bucket, or ask the
[resolver](../resolver/README.md), a separate service. So if the console is
down, nothing changes for users. Publishing stays in
the CLI and CI: signing keys never reach the console, so it can rearrange
signed releases but never ship code. There's no sign-in yet: run it where only
your team can reach it.

## Run it

```sh
idealyst build --web crates/ota/console

export OTA_CONSOLE_LOCATION=s3://my-ota-bucket/my-app    # or a directory
export AWS_ACCESS_KEY_ID=… AWS_SECRET_ACCESS_KEY=… AWS_REGION=us-east-1
# For MinIO or another S3-compatible store:
export AWS_ENDPOINT_URL=http://localhost:9000

cargo run -p ota-console --features server --bin server
# → http://127.0.0.1:3100/
```

| Variable | Default | |
|---|---|---|
| `OTA_CONSOLE_LOCATION` | — | `s3://bucket/prefix`, or a directory |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` | — | Keys that can read and write the location |
| `AWS_SESSION_TOKEN` | — | For temporary credentials |
| `AWS_REGION` / `AWS_DEFAULT_REGION` | `us-east-1` | |
| `AWS_ENDPOINT_URL` | AWS for the region | MinIO, R2, … |
| `PORT` | `3100` | |
| `OTA_CONSOLE_BIND` | `127.0.0.1` | Set `0.0.0.0` to serve beyond this machine (in a container, say) |

The keys need `s3:GetObject`, `s3:PutObject` and `s3:ListBucket` on the
location (listing finds the builds reported from the field). The console
reads the location at startup and logs a warning if it can't.

**Against the demo's MinIO** (`crates/ota/demo/setup.sh`):

```sh
. crates/ota/demo/minio.env
OTA_CONSOLE_LOCATION=s3://ota-demo/demo cargo run -p ota-console --features server --bin server
```

With the demo window open, take its live release down with the kill switch and
watch the window switch to the release before it within 5 seconds.

**A fleet to look at:** `IDEALYST_FLEET_OUT=/tmp/fleet cargo test -p
remote-showcase --test fleet` writes a location with four app versions and
six releases (`tests/fleet.rs`). Then run
`OTA_CONSOLE_LOCATION=/tmp/fleet crates/ota/console/dev.sh` to see a grid
with every kind of mismatch.

**Developing the console** (it rebuilds the page and the server on save):

```sh
crates/ota/console/dev.sh                 # against the demo's MinIO, on :3100
crates/ota/console/dev.sh --port 3001     # any `idealyst dev` arguments pass through
```

`dev.sh` sets the demo's MinIO variables. Plain `idealyst dev --web` works too,
with the variables above exported first.

If the location isn't set, or the store can't be read (MinIO stopped, wrong
keys), the console still starts. The page says what's wrong, and **Refresh**
retries.
