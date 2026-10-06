# Over-the-air updates

Ship new versions of an app's [remote components](remote-components.md)
without an app store release. Publish a bundle, and apps pick it up at their
next check:

- **One large bundle:** an over-the-air update of the app's screens.
- **Many small ones:** server-driven UI, each component downloaded the first
  time it's shown.

There's no server to run. Releases are static files in an S3 bucket (or any
directory), served through a CDN. Each app reads them and decides for itself
what it can run.

## Set up

From the app's directory:

```sh
idealyst ota init --url https://ota.example.com/my-app --bucket s3://my-app-ota/my-app
```

This writes the settings into the app's `Cargo.toml`:

```toml
[package.metadata.idealyst.ota]
url = "https://ota.example.com/my-app"     # where apps read releases (a CDN in front of the bucket)
bucket = "s3://my-app-ota/my-app"          # where publishing writes them
public_keys = ["3d2804ee…"]                # the app only runs bundles signed with this key
```

It also creates the signing key, `ota-signing.key`, and adds it to
`.gitignore`. Keep it secret: whoever has it can publish bundles your app will
run. In CI, put its contents in `IDEALYST_REMOTE_SIGNING_KEY` instead.

The bundles are the ones the app already declares for
`idealyst build --remote` (`[package.metadata.idealyst.remote]`).

## In the app

```rust
let ota = ota::start(ota::config!(), ota::Options {
    built_in: vec![("screens", include_bytes!(…))],   // optional: runs on first launch, offline
    ..Default::default()
})?;
```

```toml
[package.metadata.idealyst.ota]
host_fns = "app::host_fns"   # what bundles may call: a public fn() -> Vec<HostFnDef>, from the library's root
```

Call it once at startup, inside the app's world (it creates a signal).
`ota::config!()` reads the settings from `Cargo.toml` when the app compiles.
To point a staging build elsewhere, set `IDEALYST_OTA_URL` when building it.

Declaring `host_fns` in `Cargo.toml` means the app runs with the same list
`idealyst ota manifest` registers (below). `Options::host_fns` still works for
an app that doesn't register its builds; giving both is an error.

After that, `#[component(remote)]` components just work:

- **At launch**, the app runs the bundles it ran last time, from its cache.
  On first launch, and right after an app update, it runs the built-in ones.
- **In the background**, it downloads the newest release of each bundle it
  can run. The release runs from the next launch, so nothing on screen
  changes under the user. `apply: ota::Apply::Now` swaps it in at once.
- **A component no bundle provides yet** (no built-in copy, first time
  shown) holds an empty place while its bundle downloads, then mounts.
- **Offline**, it keeps running what it has.

To tell users an app update would bring something new:

```rust
if ota.status().get() == ota::Status::AppUpdateRequired {
    // "Update the app to get the latest"
}
```

| `Status` | Meaning |
|---|---|
| `Checking` | Running what it has, and looking for updates |
| `UpToDate` | Running the newest release of every bundle it can |
| `UpdateReady` | A newer release is downloaded and runs from the next launch |
| `AppUpdateRequired` | A newer release exists that this app version can't run |
| `Failed(reason)` | The last check failed (offline, say); running what it has |

`ota.check()` checks again, for a pull-to-refresh. `check_every:
Some(Duration::from_secs(300))` checks on a schedule.

For a settings screen or a debug panel, `ota.bundles()` is each bundle's
state:
- what runs (version, content hash, and whether it came built in, from the
  cache or from a download);
- the newest release it can run, and whether a newer one needs an app update;
- what it's doing (downloading, ready for the next launch, failed).

`ota.checks()` gives when the last check ended and when the next starts.
`crates/ota/demo` shows all of it live, against a local MinIO.

## Publish

```sh
idealyst ota publish                  # every bundle
idealyst ota publish --bundle shop    # one
```

This builds each bundle as a release, signs it, and publishes it. A bundle
whose bytes didn't change isn't published again. When a release starts
requiring something its current release didn't, you're told before anything
goes live:

```text
  shop 1.3.0: new release
    it requires what the current release didn't — apps that lack it keep the current release:
      - prop `idea_ui::components::card::Card.elevation`
      - host function `shop::checkout`
Publish? [y/N]
```

Publishing it anyway is safe. Apps built without `Card.elevation` keep
running the release they have, and report `AppUpdateRequired`. Apps that have
it get the new one. Pass `--yes` to skip the question in CI.

```sh
idealyst ota status           # what's published: each bundle's newest release, its components
idealyst ota rollback shop    # take back shop's newest release; apps return to the one before
```

## Answers per app build

Each app build has a **manifest**: what it offers bundles (its components and
their props, its host functions, the remote components it mounts, its context
types), each with its type's shape. The manifest's **id** is a hash of it, so
two builds that offer the same things share an id. `ota.manifest_id()` gives
it.

A check gets its answer (what each bundle should run) from the first of these
that has one:

1. **A resolution service**, when the settings name one (`resolver =
   "https://…"`, or `IDEALYST_OTA_RESOLVER` at build time). The app sends its
   id. Only if the service has never seen it does the app send the whole
   manifest, once. `ota-resolver` is that service, run as a server or on
   AWS Lambda ([its README](../crates/ota/resolver/README.md)).
2. **The build's precomputed answer** at the release location,
   `resolved/<id>.json`. It is there once the build is registered, and every
   publish, take-down, restore and pin rewrites it.
3. **The index**, deciding on the device, as before.

All three make the same decision (`ota_index::resolve`). An answer made for
another manifest, or under another version of the compatibility rule, is
ignored. The app still checks each bundle's signature and requirements before
running it. A wrong answer can delay an update, but can't make the app run a
bundle it can't. `ota.checks().get().answered_by` says which one answered.

**Register each app release:**

```sh
idealyst ota manifest     # capture this build's manifest and register it
```

This builds a small program that links the app's library for this machine and
prints `ota::manifest(&host_fns())`, the same call the app makes at startup.
It then stores the manifest at the location and writes the build's answer.
Code compiled only on some platforms (`cfg(target_os)`) can make the shipped
build's manifest differ from this one. Such a build simply has no precomputed
answer: it reads the index, or reports its own manifest to the service.

Once builds are registered:
- `idealyst ota status` lists each build and what it runs;
- `idealyst ota publish` names each registered build a new release won't
  reach, and why;
- the console shows the same, and names the builds a take-down would move.

If a change to the index can't finish rewriting the answers (the store
failed partway), the change still stands, and the command says so;
`idealyst ota resolve` rewrites them. An older answer never overwrites a newer
one: each records the index `generation` it came from.

**Upgrade every CLI that publishes to a location before you register builds
there.** An older `idealyst` (one whose `ota-publish` predates registered
builds) still publishes and rolls back, but it rewrites the index without what
it doesn't know: the `generation`, the pin, and how each release was taken
down, including the kill switch. It also leaves every precomputed answer as
it was. Apps of registered builds read their answer before the index, so they
don't see that publish or rollback at all. A newer CLI or the console notices
an index whose generation was dropped and counts on from the newest stored
answer, so the answers recover with the next change made through it.

To recover after an older CLI wrote to the location:

1. Upgrade that CLI.
2. Run `idealyst ota resolve` with the new one. It rewrites every registered
   build's answer from the current index.
3. In the console, pin again what was pinned, and take down again with the
   kill switch what had it. The audit log (`audit.json`) lists both.

**The id, for another language.** It is the SHA-256, in lowercase hex, of the
compact JSON `{"rule":<RULE>,"provides":<manifest>}`. The manifest's fields
come in this order: `codec`, `components`, `host_fns`, `remote`, `contexts`.
Map keys are sorted, and there's no whitespace. `RULE` is
`remote_bundle::RULE`, the compatibility rule's version. It changes whenever
the rule could decide differently, so ids never span two rules.

## Managing releases: the console

`crates/ota/console` is a self-hosted web page for the release location
([its README](../crates/ota/console/README.md)). With it you can:

- **Take down** any release. Apps move to the newest remaining release they
  can run, when they'd apply any update.
- **Kill switch:** a take-down marked urgent. Apps *running* that release
  replace it at once, whatever their `apply` setting. If none is left, they
  return to their built-in copy, or stop running the bundle.
- **Restore** a taken-down release, to its place by publish time.
- **Pin** a release ahead of newer ones, including ones published later.
  Unpin to serve the newest again.
- **App builds:** each registered build and what it runs. Before a
  take-down, see which builds run that release and where each would go.
- **Compatibility grid:** per bundle, every registered build against every
  live release, with the reason behind each ✗.
- **Audit log:** every action, and every CLI publish, in `audit.json` beside
  the index.

The console edits the index; apps never talk to it. The service apps can ask,
`ota-resolver`, runs separately (see [Answers per app build](#answers-per-app-build)). Signing keys stay with the
CLI, so the console can rearrange signed releases but not ship code.

**How older apps see these.** A pin is expressed as the *order* of
`releases`, which every client follows, so apps built before pinning existed
serve the pinned release too. The kill switch needs `ota` 0.2 or later: older
clients treat an urgent take-down as a plain one.

## How an app picks a release

Every release bundle lists what it needs from an app: the app components and
props it uses, the host functions it calls, with their types
([details](remote-components.md#which-apps-a-bundle-runs-on)). The index lists
each bundle's recent releases with those lists. An app compares each list
against what it has itself, and runs the newest release that fits. So:

- an app never downloads a bundle it can't run;
- one release location serves every app version in the field, with no
  per-version files or servers;
- the loader checks the same list again before running a bundle, so even a
  wrongly published release is refused, not misrun.

`crates/streaming/showcase/app/tests/fleet.rs` shows it across a fleet. Four
app versions, real code, change a prop, a host function, an argument type and
a struct field. They are checked against six releases: which release each
version can run, why it can't run the others, and which it chooses.

## What's in the bucket

```text
index.json                          ← rewritten on each publish; served revalidated
bundles/<name>/<sha256>.wasm        ← never change; cached forever
manifests.json                      ← app builds registered from their build
reported/<id>.json                  ← app builds reported from the field, one marker each
manifests/<id>.json                 ← each build's manifest; never changes
resolved/<id>.json                  ← each registered build's answer; rewritten with the index
audit.json                          ← who changed what
```

Bundles are uploaded before the index names them, so an app never sees an index
pointing at a missing file. The index is written only if nobody changed it
since this publish read it, so two people publishing at once can't lose each
other's release. Publishing uses the `aws` CLI with your own credentials. The
index keeps each bundle's last 20 releases.

Put a CDN (CloudFront) in front of the bucket and use its address as `url`. The
cache headers are set on upload: the index is revalidated on each read, and the
bundles are immutable. `bucket` can also be a local directory, with
`url = "file://…"`, for development and tests.

## Testing against S3

MinIO speaks the S3 API, including the conditional writes publishing relies
on, so a local container stands in for S3 and the CDN:

```sh
docker run -d --name ota-minio -p 9000:9000 -p 9001:9001 \
  -e MINIO_ROOT_USER=otatest -e MINIO_ROOT_PASSWORD=otatest-secret \
  minio/minio server /data --console-address :9001

export AWS_ACCESS_KEY_ID=otatest AWS_SECRET_ACCESS_KEY=otatest-secret \
       AWS_DEFAULT_REGION=us-east-1 AWS_ENDPOINT_URL=http://localhost:9000
aws s3api create-bucket --bucket ota-test
# Anyone may read, as through a CDN:
aws s3api put-bucket-policy --bucket ota-test --policy \
  '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["*"]},"Action":["s3:GetObject"],"Resource":["arn:aws:s3:::ota-test/*"]}]}'
```

Then publish to it, and point an app (or a test) at it over HTTP:

```sh
IDEALYST_OTA_BUCKET=s3://ota-test/my-app idealyst ota publish
IDEALYST_OTA_URL=http://localhost:9000/ota-test/my-app cargo run   # the app reads it
```

The framework's own tests run against it when asked. The `ota-publish` ones
are `#[ignore]`d, so a plain `cargo test` lists them as ignored; `--ignored`
runs them:

```sh
# Eight publishes racing to one index all land; publish, republish, roll back;
# registered answers and reports, through the aws CLI and signed HTTP:
IDEALYST_OTA_TEST_S3=s3://ota-test/race \
  cargo test -p ota-publish --features s3-http --test s3 -- --ignored
# The showcase published there, downloaded over HTTP by the app's client:
IDEALYST_OTA_BUCKET=s3://ota-test/showcase idealyst ota publish crates/streaming/showcase/app
IDEALYST_OTA_TEST_URL=http://localhost:9000/ota-test/showcase \
  cargo test -p remote-showcase --test ota over_http -- --ignored
```

`AWS_ENDPOINT_URL` needs AWS CLI 2.13 or later; the conditional writes need
2.22 or later.

## Your own server

None of this is required. The framework's side is the loader
(`remote_host::remote`) and the checks:

- `RemoteApp::set(name, bytes)` runs a bundle; `remove` drops one;
  `on_missing` reports a component no bundle provides yet.
- `remote_host::remote::provides(&host_fns)` is what the app has, and
  `remote_bundle::check(&requires, codec, &provides)` says whether a bundle
  fits.
- `remote_bundle::requires(&wasm)` reads a bundle's list; `ota_index` has the
  index format, `resolve` (one app's whole answer) and `choose`.

`ota-resolver` is one server that picks releases per app. Any
other can speak the same protocol (`POST /v1/resolve`, the app's id, then its
manifest on a 404), or deliver bundles another way.
