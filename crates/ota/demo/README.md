# Over-the-air demo

A macOS window in two halves:

- **Top, native:** what the updater is doing, live: checking, up to date,
  downloading, failed. Also each bundle's running version and content hash
  (and whether it came from the cache or a download), the newest release in
  the bucket, and when the next automatic check runs (every 5 seconds).
- **Bottom, remote:** `Panel` in `src/lib.rs`, downloaded from a local MinIO
  bucket and swapped in as soon as a new release is published.

## Run it

Needs Docker, the AWS CLI (2.22+) and the `idealyst` CLI.

```sh
crates/ota/demo/setup.sh          # MinIO on :9000, bucket `ota-demo` (safe to re-run)
crates/ota/demo/publish.sh        # build `Panel` and publish it
cargo run -p ota-demo             # the window
```

Then edit `Panel` in `src/lib.rs` (text, a badge, a new button) and run
`publish.sh` again. Within 5 seconds the top half shows the download and the
new hash, and the bottom half shows your change. Leave the window running
throughout; only the bundle is rebuilt.

Things to try:

- **Stop MinIO** (`docker stop idealyst-ota-minio`): the top half reports the
  failed check, and the panel keeps running. Restart MinIO and it recovers.
- **Restart the window offline:** the panel runs from the cache at once
  ("from the cache").
- **Bump `version` in Cargo.toml** before publishing, to see it change in the
  top half (otherwise only the hash changes).
- **Publish something this build can't run** (e.g. a prop idea-ui's `Button`
  doesn't have): the window keeps the current release and reports that a
  newer one needs an app update.
- **Register this build** (`. crates/ota/demo/minio.env && idealyst ota
  manifest crates/ota/demo`): the top half's last line changes from "decided
  here from the index" to "answered by this build's precomputed file". The
  manifest id there is the one the command printed. It is the same build,
  identified the same way on both sides.
- **Ask the resolver:** run `OTA_LOCATION=s3://ota-demo/demo cargo run -p
  ota-resolver` (with `minio.env` sourced), and build the window with
  `IDEALYST_OTA_RESOLVER=http://127.0.0.1:3200 cargo run -p ota-demo`. The
  window then says "answered by the service". An unregistered build reports
  itself, and the console lists it.
- **Watch the bucket** at http://localhost:9001 (otatest / otatest-secret).

`IDEALYST_OTA_DEMO_LIVE=1 cargo test -p ota-demo` renders the window against
the mock backend with the live release, as a check that the whole loop works.
