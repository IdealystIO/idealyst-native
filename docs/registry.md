# The idealyst cargo registry

Framework crates are published to a **sparse cargo registry** at
`https://crates.idealyst.io`, not pinned by git tag.

## Why this exists

A git dependency's `SourceId` includes the resolved commit. Bump the tag and
every package from that source gets a new `PackageId`, which invalidates every
fingerprint — so a consumer rebuilds its entire framework graph even when one
leaf crate changed. Cargo also materializes a full worktree of the repo per
rev; one developer machine held 28 checkouts of this repo at ~75 MB each.

Replaying the last 25 releases against a typical app's 38-crate framework
graph:

| | git tag | per-crate versions on the registry |
|---|---|---|
| median release | 38/38 rebuilt | **2/38** |
| mean | 100% | 24% |
| releases costing the consumer nothing | 0 of 25 | **7 of 25** |

Seven of those releases touched only `idealyst-cli` / `mcp-server` — crates no
consumer compiles — and still forced a full rebuild everywhere.

Two things are required for that, and either alone buys nothing:

1. **A registry.** Per-crate tarballs keyed by version, so cargo reuses the
   compiled artifact of a crate whose version did not move.
2. **Per-crate versions.** If all 133 publishable crates still bumped in
   lockstep, every version would move on every release and the registry would
   reuse nothing.

## Starting a new project

`idealyst new` scaffolds registry deps and writes the `.cargo/config.toml` that
defines the registry. Nothing else to set up.

Existing projects pinned by git keep working — the CLI mirrors whatever the
project already resolves `runtime-core` to, so a git-pinned project still gets
git-pinned wrappers. Only the fallback for a project with no framework dep yet
changed, from git to the registry.

## Using it (consumers)

Add the registry to `.cargo/config.toml`:

```toml
[registries.idealyst]
index = "sparse+https://crates.idealyst.io/index/"
```

Then depend on crates by version instead of by git. The `registry` key is
**required**, not decoration — see the warning below:

```toml
[workspace.dependencies]
idealyst       = { version = "1.5", registry = "idealyst" }
runtime-core   = { version = "1.5", registry = "idealyst" }
idea-ui        = { version = "1.5", registry = "idealyst" }
```

> **Always name the registry.** Most of these crates have bare names that are
> already taken on crates.io by unrelated packages — `css`, `wire`, `net`,
> `table`, `form`, `menu`, `video`, `canvas`, `charts`, `wasm-splitter` and
> more. A dependency without `registry = "idealyst"` resolves against
> crates.io and silently picks up a stranger's crate. This repo's own
> `.cargo/config.toml` and `[workspace.dependencies]` are set up this way for
> the same reason; the workspace will not even load without them.

Requirements are carets on `major.minor`, so a patch release is picked up by
`cargo update -p <crate>` without touching anything else.

To test a local framework change against a consumer, patch the registry the
same way you used to patch the git URL:

```toml
[patch.idealyst]
runtime-core = { path = "../idealyst-native/crates/runtime/core" }
```

As before, patch **every** crate that resolves from the registry or cargo will
report two instances of the same type ("expected `Element`, found `Element`").
Enumerate them from `Cargo.lock` rather than by hand.

## How a release works (maintainers)

`crates/tools/registry` does what `cargo publish` would. It cannot use
`cargo publish` itself: a sparse registry served from static files has no
`api` endpoint, and `cargo publish` requires one.

```sh
registry plan                     # what would be released, and at what version
registry build --out DIR          # package + lay out a registry locally
registry publish --execute        # build, upload to S3, invalidate CloudFront
```

Versions come from **conventional commits**, per crate, over the commits that
touched that crate's directory since its last release:

| commit | bump |
|---|---|
| `feat(mcp): …` | minor |
| `fix(table): …`, `chore: …`, anything unrecognised | patch |
| `feat!: …`, or `BREAKING CHANGE:` in the body | major |

An unrecognised subject earns a patch rather than nothing — a commit that
changed a crate's files still changed it, and skipping it would publish a
registry that disagrees with the source.

**One commit is skipped: a release's own version bump.** A release is cut from
a clean tree, so the tool records HEAD *before* writing the new versions, and
the bumps are committed afterwards — the recorded commit therefore always
predates the bump commit, and the next plan would see `<crate>/Cargo.toml`
changed for every crate the last release touched. A commit is ignored only
when its entire footprint in a crate is the `version` key of that crate's own
manifest; a version bump alongside any other edit is still a real change.

Publishing is **incremental**, one crate at a time in dependency order, with
each crate's tarball and index entry uploaded before the next is packaged.
That is not an optimisation: `cargo package` resolves a crate's dependencies
as if it were already published, so a crate with internal deps cannot be
packaged until those deps are actually retrievable from the registry. Staging
all of them first and uploading at the end fails on the third crate.

A **major** bump republishes dependents too, because their requirement has to
be rewritten. Minor and patch bumps deliberately do not: `1.5` already admits
`1.5.3`, and that is exactly the reuse the migration buys.

`releases.json` in the bucket records the version and commit each crate was
last cut from. It is our bookkeeping, not part of cargo's schema — an index
entry records a version but not the commit that produced it, and "what changed
since?" needs the commit.

Releases are cut **by hand**:

```sh
AWS_PROFILE=idealyst \
IDEALYST_REGISTRY_BUCKET=idealyst-crates \
IDEALYST_REGISTRY_DISTRIBUTION=EWTO387ZA9GEV \
  cargo run -p registry -- publish --execute
```

Commit the version bumps it writes; the next release compares against that
commit. Run `plan` first to see what it would do — it touches nothing.

`.github/workflows/release.yml` holds the same steps for when there is CI. It
is `workflow_dispatch` only and not wired up: a `push` trigger would fire on
every merge and fail on the missing `AWS_RELEASE_ROLE` secret.

## Layout in the bucket

```
index/config.json                    { "dl": "…/crates/{crate}/{version}/download" }
index/wi/re/wire                     JSON-lines, one line per published version
index/ru/nt/runtime-world
crates/wire/1.5.2/download           the `cargo package` tarball
releases.json                        our own version+commit bookkeeping
```

`config.json` sits at the root of the **index**, not of the bucket — cargo
fetches `<index-url>/config.json`. Putting it at the bucket root produces a
404 that cargo reports as "no matching package named X found" for whichever
crate it was resolving, which points nowhere near the actual fault.

Cache headers are set per object at upload: index files carry
`max-age=0, must-revalidate` (a consumer resolving against a cached index
cannot see a version published a minute ago; S3 answers the revalidation with
a cheap 304), while `.crate` tarballs are immutable by construction and carry
a one-year `immutable`.

Tarballs upload **before** the index. The index is what tells cargo a version
exists, so announcing it first would leave a window where a resolve finds the
entry and 404s on the download.

## Rules that are easy to trip over

- **Internal `[dev-dependencies]` must not carry a version.** Cargo strips a
  path-only dev-dep when packaging; one with a version requirement is kept and
  resolved from the registry instead. That turns a legal dev-dependency cycle
  into an unpublishable workspace — `wire` dev-depends on `dev-client`, which
  depends on `wire`, and neither could be packaged first. `registry migrate`
  de-links all 73 of them.
- **Workspace-internal crates are `publish = false`** — 125 of them: tooling,
  the 31 runnable examples under `*/examples/`, smoke tests, benchmarks.
  Consumers never name them.
- **A renamed dependency is keyed by its alias.**
  `wasm-split = { path = "…", package = "wasm-splitter" }` lives under the key
  `wasm-split`, so looking it up by package name misses it — and appending a
  second entry leaves the real one unversioned, which `cargo package` then
  rejects. `registry migrate` resolves aliases.
- **`denoise` cannot be published** and is marked `publish = false`. It
  depends on `deep_filter` by git rev, cargo refuses to package a crate with a
  git dependency, and the `deep_filter` on crates.io is the old
  dataset/training library with no DF3 runtime. Publishing the SDK means first
  mirroring `deep_filter` v0.5.6 into this registry and depending on that.
- **A published version is immutable.** Re-publishing the same version is a
  no-op when the bytes match, and a hard error when they do not.
- **Resume is scoped to one commit.** Cargo embeds `.cargo_vcs_info.json` —
  the commit SHA — inside every `.crate`, so an interrupted publish can only be
  resumed from the same commit. That covers what resume is for (a network
  blip, an expired credential). It does NOT survive committing a fix and
  retrying: every tarball's checksum changes, and every already-uploaded crate
  then reports a content mismatch. In that situation the versions were never
  consumed by anyone, so clear them and republish:

  ```sh
  aws s3 rm s3://idealyst-crates/ --recursive     # only while nothing consumes them
  ```
