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

Each framework crate is pinned to its OWN version — they release independently,
so `backend-web` may be on 2.x while `runtime-world` is on 1.8. The CLI carries
the framework's `[workspace.dependencies]` from when it was built and writes
each crate's major.minor from there (`backend-web = { version = "2.5", … }`).
For an existing registry project, the wrappers the CLI generates pin every
framework crate the app already resolves to the version it resolved, so they
unify with the app's copies; only crates the app doesn't use yet come from the
CLI's table. One exception: a requirement is never pinned *below* the CLI's own
within the same major, because the generated wrapper source is written against
the CLI's framework (see "Generated wrappers" below).

## Generated wrappers

Every native platform build goes through a generated wrapper crate at
`<app>/target/idealyst/<app>/<platform>/…` — its own cargo workspace, with its
own `Cargo.lock`, regenerated on every `idealyst dev` / `build` / `run`. Every
wrapper (iOS, Android, macOS, Linux, Windows, terminal, sim, SSR) works the
same way; only Roku still re-resolves per run:

- **The wrapper builds exactly what the app locked.** Its `Cargo.lock` is
  copied from the app's (the workspace root's, for a member) and re-copied only
  when the app's lock *content* changes — `.idealyst-lock-seed` next to it
  records which lock it came from and what cargo made of it. A run with
  nothing changed resolves nothing and contacts no index. A `cargo update` in
  the app reaches the wrapper on its next build.
- **Except where the CLI needs newer.** A wrapper requirement is the app's
  resolved major.minor, raised to the CLI's own within the same major (never
  across majors, which would mean two copies of the crate). If the app's lock
  is behind the CLI — `backend-ios-mobile` 1.13 against a CLI whose template
  calls 1.14's `deliver_inbound_link` — cargo bumps just that crate in the
  wrapper. Keep the app's lock and the CLI in step (`[workspace.metadata.idealyst]`)
  and the wrapper matches the app exactly.
- **Files are written only when their content changes**, so an unchanged
  regeneration does not dirty the wrapper crate (which for the iOS staticlib
  meant re-archiving a ~1 GB debug `.a`).
- **The target dir is private and pruned.** `<wrapper>/target` belongs to the
  wrapper alone (SSR keeps a second, `<wrapper>/target-premint`, for its
  premint posture; the CLI pins the dir with `CARGO_TARGET_DIR`). Each build
  records its units per variant in the profile dir it compiled into —
  `<target>/<triple>/<profile>/.idealyst-live/` for a cross build,
  `<target>/<profile>/.idealyst-live/` for a desktop host build — as
  `dev.json` for `idealyst dev`'s `--features dev` and `default.json` for
  `build` / `run`, and then deletes every unit, root binary/staticlib/cdylib
  and incremental cache no recorded variant uses (`build_ios::target_gc`).
  Disk use stays at one current build per variant (measured: ~2.8 GB for a
  small app's dev + non-dev iOS simulator builds, flat across releases;
  1.93 GB for `examples/inspector`'s macOS build, unchanged by a no-op
  rebuild), instead of one more generation per framework release.

The dev and non-dev variants share every unit below the framework
(third-party crates, `runtime-world`, `runtime-scene`); they differ from
`runtime-shared` up, because `dev` turns on `runtime-shared/robot` + `catalog`,
the vocabulary's robot registry, and the macros' catalog emission — which
reaches every `#[component]` crate including the app. Narrowing it further
would mean shipping the robot/catalog hooks in production builds.

### Why the desktop wrappers no longer share the project's `target/`

The macOS, terminal, sim and SSR wrappers used to build into the project's own
`target/` to reuse its compiled dependencies. Measured for
`examples/inspector` on macOS (2026-10-08, cargo 1.97, M3 Max): a cold wrapper
build into its own dir took 32.4 s; into a target the project's
`cargo build -p inspector` had just filled, 17.8 s — 39 of the wrapper's 222
units were reusable, every one a third-party crate. Path crates (the framework
in a checkout, the app itself) never match: cargo hashes a path package by its
path relative to the workspace root, and the wrapper is its own workspace; nor
does the workspace's `[profile.dev]` apply inside it. And the saving is paid
once per graph change, since the wrapper's own rebuilds are incremental either
way.

Against that, a shared dir rules out both halves of keeping the wrapper
bounded. The prune deletes every unit no wrapper variant recorded — in a
shared dir, the app's own build. And a seeded lock makes the wrapper resolve
the app's versions, so its units sit beside the app's differing only in that
workspace root: the Linux wrapper, sharing the project's target, poisoned the
project's next build that way ("multiple different versions of crate
`runtime_scene`", one source file named as both types) until it moved to a
private dir. Deleting the lock instead, which the shared-target wrappers did,
re-resolved against the registry every run — a new generation of the tree on
every framework release, piling up with nothing to remove it.

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
idealyst       = { version = "1.5.2", registry = "idealyst" }
runtime-core   = { version = "1.5.2", registry = "idealyst" }
idea-ui        = { version = "1.8.0", registry = "idealyst" }
```

> **Always name the registry.** Most of these crates have bare names that are
> already taken on crates.io by unrelated packages — `css`, `wire`, `net`,
> `table`, `form`, `menu`, `video`, `canvas`, `charts`, `wasm-splitter` and
> more. A dependency without `registry = "idealyst"` resolves against
> crates.io and silently picks up a stranger's crate. This repo's own
> `.cargo/config.toml` and `[workspace.dependencies]` are set up this way for
> the same reason; the workspace will not even load without them.

Requirements are carets on the crate's full version. A caret is a floor, not a
pin — `^1.7.1` still resolves 1.8.0 — so a later minor is picked up by
`cargo update -p <crate>` without touching anything else. The floor names the
full version rather than `major.minor` because a patch release here can add
API: the bump level is classified from the commit subject, and a `fix:` commit
that lands a new method is a patch. `runtime-shared 1.7.1` added
`PointerButton::is_primary` and `Recognizer::drive` that way, and a
`major.minor` floor let `gesture` ship calling both while declaring `1.5`.

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

"Dependency order" means everything `cargo package` resolves from the
registry, which is more than `[dependencies]`: normal and build dependencies,
plus every internal **dev-dependency that carries a version** (usually
inherited through `x = { workspace = true }`, including ones under
`[target.'cfg(..)'.dev-dependencies]`). Cargo strips a path-only dev-dep from
the packaged manifest but resolves a versioned one like any other. The
2026-09-24 release ordered by `[dependencies]` alone, packaged `wire` (versioned
dev-dep on `runtime-macros`) before the new `runtime-macros` was uploaded, and
died mid-publish. A dev edge only orders the publish when its target is part
of the same release — an untouched crate is already in the registry at the
version the floor names. Dev edges never decide *what* is released: a
sibling's major bump drags in its normal/build dependents only.

A **major** bump republishes dependents too, because their requirement has to
be rewritten. Minor and patch bumps deliberately do not, and that is exactly
the reuse the migration buys.

Separately from *who* gets republished, every bump — patch included — moves
that crate's floor in `[workspace.dependencies]`, before anything is packaged.
Moving the floor republishes nobody (the root manifest is not published); it
decides what a dependent packaged in this release, or in any later one,
records as its requirement. Skipping it is how `gesture 1.5.3` shipped
`runtime-shared = "1.5"` while calling an API that only landed in 1.7.1 — a
consumer whose lock already held 1.7.0 kept it, since the requirement was
satisfied, and the build failed inside `gesture`.

`--force <CRATE>` puts a crate in the plan at a patch bump although nothing in
its directory changed. It is for a published manifest that is wrong while the
source is right — an under-declared internal requirement, most likely — which
the diff-driven plan can never reach, because correcting recorded metadata does
not change any crate's source. It never downgrades a crate that earned a bigger
bump on its own.

**A crate built from files outside its directory** declares them in
`[package.metadata.registry]`. The CLI is the case it exists for: the binary
embeds the `idealyst new` scaffold (`examples/welcome`) and the Inspector's web
build (from `examples/inspector`), and `cargo package` carries only the crate's
own directory.

```toml
[package.metadata.registry]
also-watch = ["examples/welcome", "examples/inspector"]
prepackage = ["cargo", "build", "-p", "idealyst-cli", "--config", "env.IDEALYST_CLI_EXPORT_PACKAGE_ASSETS='1'"]
```

- `also-watch` paths count as the crate's own when the plan asks "what changed
  since the last release?", so a scaffold edit releases a new CLI.
- `prepackage` runs from the workspace root just before `cargo package` for
  that crate, and a failure stops the release. The CLI's command makes its
  build script copy both into `crates/tools/cli/package-assets/` — gitignored,
  listed in the crate's `include` — and the published build script reads them
  from there when no workspace surrounds it. In the workspace it always reads
  the workspace, so a stale `package-assets/` never leaks into a dev build.
  Unknown keys in the table are an error, so a misspelling cannot silently
  publish a crate without its files.

`--bump <CRATE>=<LEVEL>` raises a crate's bump level. The level is classified
from commit SUBJECTS, and a subject is a sentence someone wrote rather than a
contract — additive public API lands under a free-form subject and reads as a
patch. The version is the only thing a consumer sees, so it should say that API
arrived. Raise-only: asking for less than a crate earned is refused, because
silently under-publishing API is the failure the flag exists to prevent. A
crate that is not already in the plan is refused too — use `--force` for that.

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

The full procedure around those two commands — what to review before
publishing, which packages and targets to verify (four `newcore::tests`
fail on any host that has not linked AppKit), and how to confirm the result
from outside the workspace — is the `release` skill in
[`.claude/skills/release/`](../.claude/skills/release/SKILL.md).

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

- **Prefer internal `[dev-dependencies]` without a version.** Cargo strips a
  path-only dev-dep when packaging; one with a version requirement is kept and
  resolved from the registry instead. The publish order accounts for that (see
  above), but a versioned dev-dep can close a cycle that no order solves —
  `wire` dev-depends on `dev-client`, which depends on `wire`; with a version
  on that dev-dep, and both crates in one release, each needs the other's new
  version in the registry first. `build`/`publish` refuse such a release
  before packaging anything and name the dev edge; the fix is to make it
  path-only (`{ path = "…" }`, no `version`, no `workspace = true`).
  `registry migrate` de-linked all 73 that existed then; `wire`'s
  `runtime-macros`/`runtime-template` dev-deps were added later with
  `workspace = true` and are versioned.
- **Workspace-internal crates are `publish = false`**: the runnable examples
  under `*/examples/`, smoke tests, benchmarks, and the release tooling itself.
  Consumers never name them. The CLI (`idealyst-cli`) and the 35 crates it
  builds on — `build-*`, `run-*`, `dev-*`, `wasm-carve`, `mcp-server`, `lint`,
  `configure`, `robot-*`, `inspector-*`, … — ARE published, so the CLI installs
  from the registry (`cargo install idealyst-cli --index …`, or `idealyst
  update`). Their versions move like any other crate's; consumers' app builds
  never depend on them, so their releases cost no one a rebuild.
- **A missing crate must answer 404, not 403.** S3 reports an absent key as
  AccessDenied to any caller that may not list the bucket, and cargo treats a
  403 from a sparse index as a hard error rather than "no such crate" — a
  typo'd crate name reads as `AccessDenied`, and a multi-crate `cargo package`
  cannot overlay not-yet-published siblings. `scripts/provision-registry.sh`
  therefore grants the distribution `s3:ListBucket` as well as `s3:GetObject`.
  That exposes no listing: the root is served as `index.html` and the cache
  policy forwards no query string. Re-apply after editing the policy with
  `AWS_PROFILE=idealyst ./scripts/provision-registry.sh distribution`.
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
