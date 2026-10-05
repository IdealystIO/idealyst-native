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
    host_fns: host_fns(),                             // what bundles may call
    built_in: vec![("screens", include_bytes!(…))],   // optional: runs on first launch, offline
    ..Default::default()
})?;
```

Call it once at startup, inside the app's world (it creates a signal).
`ota::config!()` reads the settings from `Cargo.toml` when the app compiles.
To point a staging build elsewhere, set `IDEALYST_OTA_URL` when building it.

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

`ota.check()` checks again, for a pull-to-refresh or a timer.

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

## What's in the bucket

```text
index.json                          ← rewritten on each publish; served revalidated
bundles/<name>/<sha256>.wasm        ← never change; cached forever
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

## Your own server

None of this is required. The framework's side is the loader
(`remote_host::remote`) and the checks:

- `RemoteApp::set(name, bytes)` runs a bundle; `remove` drops one;
  `on_missing` reports a component no bundle provides yet.
- `remote_host::remote::provides(&host_fns)` is what the app has, and
  `remote_bundle::check(&requires, codec, &provides)` says whether a bundle
  fits.
- `remote_bundle::requires(&wasm)` reads a bundle's list; `ota_index` has the
  index format and `choose`.

A server can pick releases per app instead (the app sends its `provides`), or
deliver bundles any other way.
